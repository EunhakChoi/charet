//! One Chromium for the process, a fresh browser context (storage, service workers) per render.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::log::{
    self, EventEntryAdded, LogEntryLevel, LogEntrySource,
};
use chromiumoxide::cdp::browser_protocol::network::{
    self, EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent, ResourceType,
};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams,
};
use chromiumoxide::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams, SetAutoAttachParams,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    ConsoleApiCalledType, EventConsoleApiCalled, EventExceptionThrown, ExceptionDetails,
    RemoteObject,
};
use chromiumoxide::error::CdpError;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

const COLLECT: &str = include_str!("collect.js");
/// No request for this long counts as network idle.
const IDLE: Duration = Duration::from_millis(500);
/// Longest wait for network idle after the page said it is ready.
const IDLE_MAX: Duration = Duration::from_secs(10);
const NAVIGATE_MAX: Duration = Duration::from_secs(60);

/// What to render.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    pub url: String,
    pub width: u32,
    pub height: u32,
    /// `jpeg`, `png`, or absent for a check without a picture.
    #[serde(default)]
    pub image: Option<ImageFormat>,
    #[serde(default)]
    pub quality: Option<i64>,
    /// Collect the element map and loaded resources.
    #[serde(default)]
    pub map: bool,
    /// Longest wait for `<html data-charet-ready>`.
    #[serde(default = "default_ready_ms")]
    pub ready_ms: u64,
    /// Extra wait after network idle, for late paint (transitions, fonts).
    #[serde(default = "default_settle_ms")]
    pub settle_ms: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Jpeg,
    Png,
}

const fn default_ready_ms() -> u64 {
    30_000
}

const fn default_settle_ms() -> u64 {
    300
}

/// What came out of one render.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shot {
    /// The value of `data-charet-ready` (`true`, `error`), or absent when it never appeared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready: Option<String>,
    /// Page errors, console errors and missing mocks, grouped by source, then in time order.
    pub issues: Vec<String>,
    /// Base64 image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elements: Option<Value>,
    /// The page URL and every resource it loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Vec<String>>,
}

/// Where an issue came from. Issues are reported grouped by source so output is stable even when
/// console and network events race.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    /// Uncaught exception.
    Page,
    /// `console.error`, charet notices.
    Console,
    /// Browser log (failed loads).
    Log,
    /// A `fetch` / XHR that went out on the network although mocks answer in the page.
    Network,
    /// The renderer itself (navigation failed, never ready).
    Render,
}

#[derive(Debug, Default)]
struct Watch {
    issues: Vec<(Source, f64, String)>,
    /// `fetch` / XHR requests that reached the network: time, `METHOD /path?query`, host if not
    /// the page's own.
    unmocked: Vec<(f64, String, Option<String>)>,
    /// Requests in flight, by id.
    inflight: HashMap<String, String>,
    last_activity: Option<Instant>,
}

type Shared = Arc<StdMutex<Watch>>;

fn lock(watch: &Shared) -> std::sync::MutexGuard<'_, Watch> {
    watch.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A page and the browser context that owns it; both go away together.
struct Tab {
    page: Page,
    browser: Arc<Browser>,
    context: BrowserContextId,
}

impl Tab {
    async fn close(self) {
        let _ = self.page.close().await;
        let _ = self.browser.dispose_browser_context(self.context).await;
    }
}

/// Chromium settings.
#[derive(Debug, Clone)]
pub struct ChromeOptions {
    pub executable: PathBuf,
}

#[derive(Debug)]
pub struct Chrome {
    options: ChromeOptions,
    live: Mutex<Option<Arc<Browser>>>,
}

impl Chrome {
    pub fn new(options: ChromeOptions) -> Self {
        Self {
            options,
            live: Mutex::new(None),
        }
    }

    async fn launch(&self) -> Result<Arc<Browser>, String> {
        let dir = std::env::temp_dir().join(format!("charet-render-{}", std::process::id()));
        let args = vec![
            "--disable-back-forward-cache".to_owned(),
            "--no-service-autorun".to_owned(),
            "--hide-scrollbars".to_owned(),
            // No hover styles stuck on whatever sits under the default mouse position.
            "--blink-settings=primaryHoverType=2,availableHoverTypes=2,primaryPointerType=4,availablePointerTypes=4".to_owned(),
        ];
        let config = BrowserConfig::builder()
            .chrome_executable(&self.options.executable)
            .user_data_dir(&dir)
            .no_sandbox()
            .viewport(None)
            .request_timeout(Duration::from_secs(120))
            .launch_timeout(Duration::from_secs(30))
            .args(args)
            .build()?;
        let (browser, mut handler) = Browser::launch(config).await.map_err(|e| {
            format!(
                "cannot start Chromium ({}): {e}",
                self.options.executable.display()
            )
        })?;
        tokio::spawn(async move { while handler.next().await.is_some() {} });
        Ok(Arc::new(browser))
    }

    async fn browser(&self, fresh: bool) -> Result<Arc<Browser>, String> {
        let mut live = self.live.lock().await;
        if fresh {
            *live = None;
        }
        if let Some(browser) = live.as_ref() {
            return Ok(browser.clone());
        }
        let browser = self.launch().await?;
        *live = Some(browser.clone());
        Ok(browser)
    }

    /// A new page in a new context; restarts Chromium once if it died.
    async fn tab(&self) -> Result<Tab, String> {
        match open(self.browser(false).await?).await {
            Ok(tab) => Ok(tab),
            Err(_) => open(self.browser(true).await?).await,
        }
    }

    pub async fn close(&self) {
        if let Some(browser) = self.live.lock().await.take()
            && let Ok(mut browser) = Arc::try_unwrap(browser)
        {
            let _ = browser.close().await;
            let _ = browser.wait().await;
        }
    }

    pub async fn render(&self, request: &RenderRequest) -> Shot {
        let mut shot = Shot::default();
        let tab = match self.tab().await {
            Ok(tab) => tab,
            Err(error) => {
                shot.issues.push(first(&error, 300));
                return shot;
            }
        };
        let watch = Shared::default();
        if let Err(error) = shoot(&tab.page, request, &watch, &mut shot).await {
            push(&watch, Source::Render, f64::MAX, first(&error, 300));
        }
        tab.close().await;
        let mut watch = lock(&watch);
        report_unmocked(&mut watch);
        watch
            .issues
            .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
        shot.issues = watch.issues.drain(..).map(|(_, _, issue)| issue).collect();
        shot
    }
}

async fn open(browser: Arc<Browser>) -> Result<Tab, String> {
    let context = browser
        .create_browser_context(CreateBrowserContextParams::default())
        .await
        .map_err(|e| e.to_string())?;
    let params = CreateTargetParams::builder()
        .url("about:blank")
        .browser_context_id(context.clone())
        .build()?;
    // chromiumoxide pauses new workers until a debugger resumes them; nothing here does, so the
    // MSW service worker would hang and the app never mount. Attach without waiting.
    let attach = SetAutoAttachParams::builder()
        .auto_attach(true)
        .wait_for_debugger_on_start(false)
        .flatten(true)
        .build()?;
    let page = match browser.new_page(params).await {
        Ok(page) => page.execute(attach).await.map(|_| page),
        Err(error) => Err(error),
    };
    match page {
        Ok(page) => Ok(Tab {
            page,
            browser,
            context,
        }),
        Err(error) => {
            let _ = browser.dispose_browser_context(context).await;
            Err(error.to_string())
        }
    }
}

async fn shoot(
    page: &Page,
    request: &RenderRequest,
    watch: &Shared,
    shot: &mut Shot,
) -> Result<(), String> {
    let cdp = |e: CdpError| e.to_string();
    let mut exceptions = page
        .event_listener::<EventExceptionThrown>()
        .await
        .map_err(cdp)?;
    let mut console = page
        .event_listener::<EventConsoleApiCalled>()
        .await
        .map_err(cdp)?;
    let mut logs = page
        .event_listener::<EventEntryAdded>()
        .await
        .map_err(cdp)?;
    let mut sent = page
        .event_listener::<EventRequestWillBeSent>()
        .await
        .map_err(cdp)?;
    let mut finished = page
        .event_listener::<EventLoadingFinished>()
        .await
        .map_err(cdp)?;
    let mut failed = page
        .event_listener::<EventLoadingFailed>()
        .await
        .map_err(cdp)?;
    page.execute(log::EnableParams::default())
        .await
        .map_err(cdp)?;
    page.execute(network::EnableParams::default())
        .await
        .map_err(cdp)?;
    page.execute(SetDeviceMetricsOverrideParams::new(
        i64::from(request.width),
        i64::from(request.height),
        1.0,
        false,
    ))
    .await
    .map_err(cdp)?;

    let w = watch.clone();
    let page_url = request.url.clone();
    let collector = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(event) = exceptions.next() => {
                    let message = exception_message(&event.exception_details);
                    push(&w, Source::Page, *event.timestamp.inner(), format!("pageerror: {}", first(&message, 300)));
                }
                Some(event) = console.next() => {
                    let text = event.args.iter().map(arg_text).collect::<Vec<_>>().join(" ");
                    if let Some(issue) = console_issue(event.r#type == ConsoleApiCalledType::Error, &text) {
                        push(&w, Source::Console, *event.timestamp.inner(), issue);
                    }
                }
                Some(event) = logs.next() => {
                    let entry = &event.entry;
                    // The browser asks for /favicon.ico on its own; a preview needs none.
                    let favicon = entry.url.as_deref().is_some_and(|u| u.ends_with("/favicon.ico"));
                    if entry.source != LogEntrySource::Worker
                        && !favicon
                        && let Some(issue) = console_issue(entry.level == LogEntryLevel::Error, &entry.text)
                    {
                        push(&w, Source::Log, *entry.timestamp.inner(), issue);
                    }
                }
                Some(event) = sent.next() => {
                    if let Some((key, host)) = unmocked_request(
                        &event.request.method,
                        &event.request.url,
                        event.r#type.as_ref(),
                        &page_url,
                    ) {
                        lock(&w).unmocked.push((*event.timestamp.inner(), key, host));
                    }
                    activity(&w, event.request_id.inner(), Some(&event.request.url));
                }
                Some(event) = finished.next() => activity(&w, event.request_id.inner(), None),
                Some(event) = failed.next() => activity(&w, event.request_id.inner(), None),
                else => break,
            }
        }
    });

    let result = navigate(page, request, watch, shot).await;
    collector.abort();
    result
}

/// Opens the page, waits until it is ready and quiet, then takes the picture and map.
async fn navigate(
    page: &Page,
    request: &RenderRequest,
    watch: &Shared,
    shot: &mut Shot,
) -> Result<(), String> {
    match tokio::time::timeout(NAVIGATE_MAX, page.goto(request.url.as_str())).await {
        Err(_) => {
            return Err(format!(
                "navigation timed out after {}s",
                NAVIGATE_MAX.as_secs()
            ));
        }
        Ok(Err(error)) => return Err(format!("navigation failed: {error}")),
        Ok(Ok(_)) => {}
    }
    shot.ready = ready(page, Duration::from_millis(request.ready_ms)).await;
    if shot.ready.is_none() {
        let pending: Vec<String> = lock(watch).inflight.values().cloned().collect();
        push(
            watch,
            Source::Render,
            f64::MAX,
            format!(
                "not ready after {}s (requests in flight: {pending:?})",
                request.ready_ms / 1000
            ),
        );
    }
    idle(watch, IDLE_MAX).await;
    tokio::time::sleep(Duration::from_millis(request.settle_ms)).await;
    grab(page, request, shot).await
}

async fn grab(page: &Page, request: &RenderRequest, shot: &mut Shot) -> Result<(), String> {
    let cdp = |e: CdpError| e.to_string();
    if let Some(format) = request.image {
        let mut params = CaptureScreenshotParams::default();
        match format {
            ImageFormat::Jpeg => {
                params.format = Some(CaptureScreenshotFormat::Jpeg);
                params.quality = Some(request.quality.unwrap_or(85).clamp(1, 100));
            }
            ImageFormat::Png => params.format = Some(CaptureScreenshotFormat::Png),
        }
        let result = page.execute(params).await.map_err(cdp)?;
        shot.image = Some(String::from(result.result.data));
    }
    if request.map {
        let elements = page.evaluate(COLLECT).await.map_err(cdp)?;
        shot.elements = elements.value().cloned();
        let resources = page
            .evaluate(
                "[location.href, ...performance.getEntriesByType('resource').map((e) => e.name)]",
            )
            .await
            .map_err(cdp)?;
        shot.resources = resources.into_value().ok();
    }
    Ok(())
}

/// Waits for `<html data-charet-ready>` and returns its value.
async fn ready(page: &Page, max: Duration) -> Option<String> {
    let end = Instant::now() + max;
    while Instant::now() < end {
        if let Ok(value) = page
            .evaluate("document.documentElement.dataset.charetReady ?? null")
            .await
            && let Ok(Some(state)) = value.into_value::<Option<String>>()
        {
            return Some(state);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

/// Returns once nothing is in flight and nothing happened for [`IDLE`], or after `max`.
async fn idle(watch: &Shared, max: Duration) {
    let start = Instant::now();
    while start.elapsed() < max {
        {
            let watch = lock(watch);
            let quiet = watch.last_activity.unwrap_or(start).elapsed();
            if watch.inflight.is_empty() && quiet >= IDLE {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A page's own `fetch` or XHR that went out on the network. The page's mocks answer in the page,
/// so such a request skipped them: its client was made before they started (it kept the real
/// `fetch`), or no handler matched. Returns `METHOD /path?query` (the form of a missing-mock
/// notice) and the host when it is not the page's. Asset-like paths on the page's own host (a
/// `fetch('/data.json')`) are not API calls; nor are other resource types.
fn unmocked_request(
    method: &str,
    url: &str,
    kind: Option<&ResourceType>,
    page_url: &str,
) -> Option<(String, Option<String>)> {
    if !matches!(kind, Some(ResourceType::Fetch | ResourceType::Xhr)) {
        return None;
    }
    let (host, target) = split_url(url)?;
    let own = split_url(page_url).is_some_and(|(page_host, _)| page_host == host);
    let path = target.split('?').next().unwrap_or(&target);
    if own && has_extension(path) {
        return None;
    }
    Some((
        format!("{method} {target}"),
        (!own).then(|| host.to_owned()),
    ))
}

/// Host and `/path?query` of an absolute http(s) URL.
fn split_url(url: &str) -> Option<(&str, String)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(end);
    let tail = tail.split('#').next().unwrap_or(tail);
    let target = if tail.starts_with('/') {
        tail.to_owned()
    } else {
        format!("/{tail}")
    };
    Some((host, target))
}

/// Whether the last path segment ends in `.word`, like the frame's own asset test.
fn has_extension(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or("");
    last.rsplit_once('.').is_some_and(|(_, ext)| {
        !ext.is_empty() && ext.chars().all(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Turns the requests that reached the network into issues, once each, except those the frame
/// already reported as a missing mock (the same request seen from both sides).
fn report_unmocked(watch: &mut Watch) {
    let reported: HashSet<String> = watch
        .issues
        .iter()
        .filter(|(source, ..)| *source == Source::Console)
        .filter_map(|(.., issue)| issue.strip_prefix("[charet] missing mock "))
        .map(str::to_owned)
        .collect();
    let mut seen = HashSet::new();
    for (at, key, host) in std::mem::take(&mut watch.unmocked) {
        if reported.contains(&key) || !seen.insert(key.clone()) {
            continue;
        }
        let to = host.map(|h| format!(" to {h}")).unwrap_or_default();
        let issue = format!(
            "[charet] unmocked request reached the network{to}: {key} (an API client made before \
             the mocks started keeps the real fetch; load the app inside renderScreen)"
        );
        watch.issues.push((Source::Network, at, first(&issue, 300)));
    }
}

fn push(watch: &Shared, source: Source, at: f64, issue: String) {
    lock(watch).issues.push((source, at, issue));
}

fn activity(watch: &Shared, id: &str, url: Option<&str>) {
    let mut watch = lock(watch);
    match url {
        Some(url) => {
            watch.inflight.insert(id.to_owned(), url.to_owned());
        }
        None => {
            watch.inflight.remove(id);
        }
    }
    watch.last_activity = Some(Instant::now());
}

/// First `n` characters.
fn first(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

fn arg_text(arg: &RemoteObject) -> String {
    match &arg.value {
        Some(Value::String(text)) => text.clone(),
        Some(value) if !value.is_null() => value.to_string(),
        _ => arg
            .description
            .clone()
            .unwrap_or_else(|| "undefined".into()),
    }
}

/// The exception's message line, without the `TypeError: ` prefix.
fn exception_message(details: &ExceptionDetails) -> String {
    let Some(description) = details.exception.as_ref().and_then(|e| {
        e.description.clone().or_else(|| {
            e.value
                .as_ref()
                .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
        })
    }) else {
        return details.text.clone();
    };
    let line = description.lines().next().unwrap_or_default();
    match line.split_once(": ") {
        Some((name, rest))
            if name.ends_with("Error") && name.chars().all(char::is_alphanumeric) =>
        {
            rest.to_owned()
        }
        _ => line.to_owned(),
    }
}

/// Console text that is a problem: every missing mock, and errors except the browser's own note
/// of an HTTP error answer. Mocked 401 / 404 / 500 answers are the point of signed-out, not-found
/// and error screens; how the app handles them shows up as page errors or its own console errors.
pub fn console_issue(error: bool, text: &str) -> Option<String> {
    if text.contains("[charet] missing mock") {
        return Some(text.to_owned());
    }
    let http_answer =
        text.starts_with("Failed to load resource: the server responded with a status");
    (error && !http_answer).then(|| format!("error: {}", first(text, 300)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_rules() {
        assert_eq!(
            console_issue(false, "[charet] missing mock GET /api/x"),
            Some("[charet] missing mock GET /api/x".into())
        );
        assert_eq!(console_issue(false, "just a warning"), None);
        assert_eq!(console_issue(true, "boom"), Some("error: boom".into()));
        assert_eq!(
            console_issue(
                true,
                "Failed to load resource: the server responded with a status of 401 ()"
            ),
            None
        );
        assert_eq!(
            console_issue(
                true,
                "Failed to load resource: the server responded with a status of 500 (Internal Server Error)"
            ),
            None
        );
        assert_eq!(
            console_issue(true, "Failed to load resource: net::ERR_CONNECTION_REFUSED"),
            Some("error: Failed to load resource: net::ERR_CONNECTION_REFUSED".into())
        );
    }

    const PAGE: &str = "http://127.0.0.1:5281/p/demo/.charet/frame.html?screen=a";

    #[test]
    fn a_fetch_that_reached_the_network_is_unmocked() {
        let fetch = Some(&ResourceType::Fetch);
        assert_eq!(
            unmocked_request(
                "GET",
                "http://127.0.0.1:5281/api/auth/me?x=1#h",
                fetch,
                PAGE
            ),
            Some(("GET /api/auth/me?x=1".into(), None))
        );
        assert_eq!(
            unmocked_request(
                "POST",
                "http://127.0.0.1:5281",
                Some(&ResourceType::Xhr),
                PAGE
            ),
            Some(("POST /".into(), None))
        );
        // Another host is named; its path alone is what a missing-mock notice would carry.
        assert_eq!(
            unmocked_request("GET", "https://api.example.com/v1/users", fetch, PAGE),
            Some(("GET /v1/users".into(), Some("api.example.com".into())))
        );
    }

    #[test]
    fn assets_and_other_resource_types_are_not_api_calls() {
        let fetch = Some(&ResourceType::Fetch);
        // The page's own files, like the frame's asset rule.
        assert_eq!(
            unmocked_request("GET", "http://127.0.0.1:5281/data.json", fetch, PAGE),
            None
        );
        // The same file name on another host is not the page's.
        assert!(
            unmocked_request("GET", "https://cdn.example.com/data.json", fetch, PAGE).is_some()
        );
        for kind in [
            ResourceType::Script,
            ResourceType::Document,
            ResourceType::Image,
        ] {
            assert_eq!(
                unmocked_request("GET", "http://127.0.0.1:5281/api/x", Some(&kind), PAGE),
                None
            );
        }
        assert_eq!(
            unmocked_request("GET", "http://127.0.0.1:5281/api/x", None, PAGE),
            None
        );
        assert_eq!(
            unmocked_request("GET", "data:text/plain,hi", fetch, PAGE),
            None
        );
        assert_eq!(
            unmocked_request("GET", "blob:http://127.0.0.1:5281/abc", fetch, PAGE),
            None
        );
    }

    #[test]
    fn a_missing_mock_is_not_reported_twice() {
        let mut watch = Watch::default();
        watch.issues.push((
            Source::Console,
            1.0,
            "[charet] missing mock GET /api/y".into(),
        ));
        watch.unmocked.push((2.0, "GET /api/y".into(), None));
        watch.unmocked.push((3.0, "GET /api/auth/me".into(), None));
        watch.unmocked.push((4.0, "GET /api/auth/me".into(), None));
        report_unmocked(&mut watch);
        let issues: Vec<&str> = watch
            .issues
            .iter()
            .map(|(.., issue)| issue.as_str())
            .collect();
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert_eq!(issues[0], "[charet] missing mock GET /api/y");
        assert!(
            issues[1]
                .starts_with("[charet] unmocked request reached the network: GET /api/auth/me"),
            "{issues:?}"
        );
    }
}
