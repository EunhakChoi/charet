//! The components a project's screens use, read from the captures that already exist.
//!
//! An element map marks the root of each component instance (`use`: where it was placed) and names
//! the innermost component around it (the last of `chain`). Grouping those by name gives every
//! component's uses across the screens, each with its box on the capture, so the web tool can cut
//! the pictures out of captures it already has: no story has to be written to see a component.
//!
//! A component needs a definition file in the project. Icons and library components (a router's
//! `Link`) have none and are left out, and so is anything that fills most of a screen (a page).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use charet_model::Actor;
use serde::Serialize;
use serde_json::Value;

use crate::captures::{THEMES, app_dir};
use crate::policy::Command;
use crate::{Capture, Engine, EngineError};

/// Pictures kept per component: enough to see how it varies, few enough to scan.
const SHOWN: usize = 8;
/// A use with a side shorter than this (px) is a hidden or empty one.
const MIN_SIDE: i64 = 4;
/// Longest text kept with a use.
const MAX_TEXT: usize = 80;

/// One component of the project and where its screens use it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentGroup {
    pub name: String,
    /// Where it is defined, relative to the app.
    pub file: String,
    /// Every use found in the screens' captures of the theme.
    pub uses: usize,
    /// A few of them, varied: no two alike in size and text, spread over the screens.
    pub instances: Vec<ComponentUse>,
}

/// One use of a component: its box on a screen's capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComponentUse {
    pub screen: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    /// Its text, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Engine {
    /// The components the project's screens use, from their captures in `theme`, grouped by the
    /// directory of the file that defines them.
    pub fn components(
        &self,
        actor: &Actor,
        key: &str,
        theme: &str,
    ) -> Result<Vec<ComponentGroup>, EngineError> {
        self.visible(actor, key, Command::ProjectView)?;
        if !THEMES.contains(&theme) {
            return Err(EngineError::Invalid(format!("unknown theme {theme:?}")));
        }
        let Some(head) = self.ports.store.head(key)? else {
            return Ok(Vec::new());
        };
        let app = app_dir(self.ports.store.read(key, &head, "charet.json")?.as_deref());
        let files = self
            .ports
            .store
            .list(key, &head)?
            .into_iter()
            .filter_map(|path| {
                if app.is_empty() {
                    Some(path)
                } else {
                    path.strip_prefix(&format!("{app}/")).map(str::to_owned)
                }
            })
            .collect::<Vec<_>>();
        let mut shots = Vec::new();
        for capture in self.ports.captures.list(key)? {
            if capture.kind != "screen" || capture.theme != theme {
                continue;
            }
            if let Some(map) =
                self.ports
                    .captures
                    .elements(key, &capture.kind, &capture.target, &capture.theme)?
            {
                shots.push((capture, map));
            }
        }
        Ok(gather(&shots, &files))
    }
}

/// One use before its component is known to be the project's.
struct Found {
    screen: String,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    text: String,
}

/// Groups the component uses of `shots` (a capture and its element map each) by component.
/// `files` are the project's files, relative to the app, for components whose elements say
/// nowhere where they are defined.
fn gather(shots: &[(Capture, Value)], files: &[String]) -> Vec<ComponentGroup> {
    let mut found: BTreeMap<String, Vec<Found>> = BTreeMap::new();
    let mut defined: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (capture, map) in shots {
        let screen_area = i64::from(capture.width) * i64::from(capture.height);
        for element in map.as_array().into_iter().flatten() {
            if element.get("use").and_then(Value::as_str).is_none() {
                continue;
            }
            let Some(name) = element
                .get("chain")
                .and_then(Value::as_array)
                .and_then(|chain| chain.last())
                .and_then(Value::as_str)
                .map(plain_name)
            else {
                continue;
            };
            let [x, y, w, h] = ["x", "y", "w", "h"].map(|side| {
                element
                    .get(side)
                    .and_then(Value::as_i64)
                    .unwrap_or_default()
            });
            let on_screen =
                x >= 0 && y >= 0 && x < i64::from(capture.width) && y < i64::from(capture.height);
            // A page fills the screen; a hidden element has no size.
            if !on_screen
                || w < MIN_SIDE
                || h < MIN_SIDE
                || w.saturating_mul(h).saturating_mul(2) > screen_area
            {
                continue;
            }
            let file = element
                .get("src")
                .and_then(Value::as_str)
                .and_then(source_file);
            // The frame around every screen is charet's own, not the project's.
            if file.is_some_and(|f| f.starts_with(".charet/")) {
                continue;
            }
            if let Some(file) = file {
                *defined
                    .entry(name.clone())
                    .or_default()
                    .entry(file.to_owned())
                    .or_default() += 1;
            }
            let text = element
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .chars()
                .take(MAX_TEXT)
                .collect();
            found.entry(name).or_default().push(Found {
                screen: capture.target.clone(),
                x,
                y,
                w,
                h,
                text,
            });
        }
    }
    let mut groups: Vec<ComponentGroup> = found
        .into_iter()
        .filter_map(|(name, uses)| {
            let file = defined
                .get(&name)
                .and_then(|files| {
                    // The file most elements come from; the first by name when they tie.
                    files
                        .iter()
                        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                })
                .map(|(file, _)| file.clone())
                .or_else(|| definition_file(&name, files))?;
            Some(ComponentGroup {
                uses: uses.len(),
                instances: pick(uses),
                name,
                file,
            })
        })
        .collect();
    groups.sort_by(|a, b| (dir(&a.file), &a.name).cmp(&(dir(&b.file), &b.name)));
    groups
}

/// A component's name as written: the bundler adds `$1` to a second one of the same name.
fn plain_name(name: &str) -> String {
    match name.rsplit_once('$') {
        Some((plain, digits))
            if !plain.is_empty()
                && !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit()) =>
        {
            plain.to_owned()
        }
        _ => name.to_owned(),
    }
}

/// The file of an element's `src` (`path:line:col`).
fn source_file(src: &str) -> Option<&str> {
    let mut parts = src.rsplitn(3, ':');
    let (_, _, path) = (parts.next()?, parts.next()?, parts.next()?);
    Some(path)
}

/// The letters and digits of a name, lowercase: `Button`, `button.tsx` and `Side-Bar` ~ `sidebar`.
fn squash(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The file that defines a component none of whose elements say so: a component file named for it
/// (`ui/button.tsx` for `Button`, `ui/button/index.tsx` as well). Prefers one under `components/`,
/// then the shortest path.
fn definition_file(name: &str, files: &[String]) -> Option<String> {
    let wanted = squash(name);
    if wanted.is_empty() {
        return None;
    }
    files
        .iter()
        .filter(|path| {
            let Some((stem, ext)) = path.rsplit_once('.') else {
                return false;
            };
            if !matches!(ext, "tsx" | "jsx")
                || path.starts_with(".charet/")
                || path.contains("node_modules/")
                || [".test", ".spec", ".stories"]
                    .iter()
                    .any(|marker| stem.ends_with(marker))
            {
                return false;
            }
            let mut parts = stem.rsplit('/');
            let last = parts.next().unwrap_or_default();
            let named = if last == "index" {
                parts.next().unwrap_or_default()
            } else {
                last
            };
            squash(named) == wanted
        })
        .min_by(|a, b| {
            let rank = |path: &str| (!path.contains("/components/"), path.len());
            rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
        })
        .cloned()
}

/// The directory of a path (`""` at the top).
fn dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Up to [`SHOWN`] uses that differ (size and text), taken one screen after another so the screens
/// all show before one repeats.
fn pick(mut uses: Vec<Found>) -> Vec<ComponentUse> {
    uses.sort_by(|a, b| (&a.screen, a.y, a.x).cmp(&(&b.screen, b.y, b.x)));
    let mut seen = BTreeSet::new();
    let mut by_screen: BTreeMap<String, VecDeque<Found>> = BTreeMap::new();
    for found in uses {
        if seen.insert((found.text.clone(), found.w, found.h)) {
            by_screen
                .entry(found.screen.clone())
                .or_default()
                .push_back(found);
        }
    }
    let mut picked = Vec::new();
    'rounds: while by_screen.values().any(|queue| !queue.is_empty()) {
        for queue in by_screen.values_mut() {
            if picked.len() == SHOWN {
                break 'rounds;
            }
            if let Some(found) = queue.pop_front() {
                picked.push(ComponentUse {
                    screen: found.screen,
                    x: found.x,
                    y: found.y,
                    w: found.w,
                    h: found.h,
                    text: Some(found.text).filter(|text| !text.is_empty()),
                });
            }
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn capture(target: &str) -> Capture {
        Capture {
            kind: "screen".into(),
            target: target.into(),
            theme: "light".into(),
            commit: "c".into(),
            width: 1000,
            height: 800,
            issues: Vec::new(),
            sources: Vec::new(),
            captured_at: "t".into(),
        }
    }

    /// The root element of a component use, as the element map has it.
    fn root(chain: &[&str], src: Option<&str>, at: [i64; 4], text: &str) -> Value {
        let mut element = json!({
            "chain": chain, "use": "src/pages/Home.tsx:9:3", "tag": "div", "sel": "body",
            "x": at[0], "y": at[1], "w": at[2], "h": at[3], "text": text,
        });
        if let Some(src) = src {
            element["src"] = json!(src);
        }
        element
    }

    fn group<'a>(groups: &'a [ComponentGroup], name: &str) -> &'a ComponentGroup {
        groups
            .iter()
            .find(|g| g.name == name)
            .unwrap_or_else(|| panic!("no {name} in {groups:?}"))
    }

    #[test]
    fn uses_are_grouped_by_component_and_defined_where_their_elements_say() {
        let home = json!([
            root(
                &["Page", "Badge"],
                Some("src/ui/badge.tsx:5:3"),
                [10, 10, 60, 20],
                "New"
            ),
            root(
                &["Page", "Badge$1"],
                Some("src/ui/badge.tsx:5:3"),
                [10, 40, 60, 20],
                "Old"
            ),
        ]);
        let list = json!([root(
            &["Badge"],
            Some("src/ui/badge.tsx:5:3"),
            [5, 5, 60, 20],
            "New"
        )]);
        let groups = gather(&[(capture("home"), home), (capture("list"), list)], &[]);
        assert_eq!(groups.len(), 1, "{groups:?}");
        let badge = group(&groups, "Badge");
        assert_eq!(badge.file, "src/ui/badge.tsx");
        assert_eq!(badge.uses, 3, "`Badge$1` is a Badge too");
        // "New" at 60x20 is on both screens but is one look: shown once.
        let seen: Vec<_> = badge
            .instances
            .iter()
            .map(|i| (i.screen.as_str(), i.text.as_deref()))
            .collect();
        assert_eq!(seen, [("home", Some("New")), ("home", Some("Old"))]);
    }

    #[test]
    fn a_component_with_no_source_in_its_elements_is_found_by_its_file_name() {
        let home = json!([
            root(&["Button"], None, [10, 10, 80, 30], "Save"),
            root(&["Sun"], None, [100, 10, 16, 16], ""),
            root(&["Link"], None, [120, 10, 40, 16], "Home"),
        ]);
        let files: Vec<String> = [
            "src/components/ui/button.tsx",
            "src/components/ui/button.test.tsx",
            "src/lib/button.ts",
            "src/pages/Home.tsx",
        ]
        .map(String::from)
        .into();
        let groups = gather(&[(capture("home"), home)], &files);
        let names: Vec<_> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            ["Button"],
            "an icon and a library `Link` have no file"
        );
        assert_eq!(groups[0].file, "src/components/ui/button.tsx");
    }

    #[test]
    fn an_index_file_is_named_for_its_folder() {
        let files = ["src/ui/side-bar/index.tsx".to_owned()];
        assert_eq!(
            definition_file("SideBar", &files).as_deref(),
            Some("src/ui/side-bar/index.tsx")
        );
        assert_eq!(definition_file("Other", &files), None);
    }

    #[test]
    fn pages_hidden_elements_and_the_frame_are_not_components() {
        let home = json!([
            root(
                &["Layout"],
                Some("src/Layout.tsx:3:3"),
                [0, 0, 1000, 800],
                "everything"
            ),
            root(&["Hidden"], Some("src/Hidden.tsx:3:3"), [10, 10, 0, 0], ""),
            root(&["Off"], Some("src/Off.tsx:3:3"), [2000, 10, 50, 50], ""),
            root(
                &["Screen"],
                Some(".charet/frame.tsx:3:3"),
                [10, 10, 100, 100],
                "x"
            ),
            root(
                &["Card"],
                Some("src/Card.tsx:3:3"),
                [10, 10, 100, 100],
                "ok"
            ),
        ]);
        let groups = gather(&[(capture("home"), home)], &[]);
        let names: Vec<_> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Card"]);
    }

    #[test]
    fn the_pictures_are_capped_and_taken_across_screens() {
        let many = |n: i64, text: &str| {
            json!(
                (0..n)
                    .map(|i| root(
                        &["Tag"],
                        Some("src/Tag.tsx:1:1"),
                        [10, 10 + i * 30, 40 + i, 20],
                        text
                    ))
                    .collect::<Vec<_>>()
            )
        };
        let shots = [
            (capture("a"), many(20, "a")),
            (capture("b"), many(2, "b")),
            (capture("c"), many(2, "c")),
        ];
        let tag = &gather(&shots, &[])[0];
        assert_eq!(tag.uses, 24);
        assert_eq!(tag.instances.len(), SHOWN);
        let screens: Vec<_> = tag
            .instances
            .iter()
            .take(3)
            .map(|i| i.screen.as_str())
            .collect();
        assert_eq!(
            screens,
            ["a", "b", "c"],
            "every screen shows before one repeats"
        );
        let from_a = tag.instances.iter().filter(|i| i.screen == "a").count();
        assert_eq!(from_a, SHOWN - 4, "b and c give two each, a the rest");
    }

    const MANIFEST: &str = r#"{"manifestVersion":1,"title":"T","pages":[],
        "screens":[{"id":"list","page":"p","title":"L","note":"","route":"/","size":{"w":800,"h":600},"hasSetup":false},
                   {"id":"empty","page":"p","title":"E","note":"","route":"/","size":{"w":800,"h":600},"hasSetup":false}],
        "connections":[],"flows":[],
        "stories":[{"id":"button","component":"Button","title":"B","size":{"w":200,"h":100}}]}"#;

    #[test]
    fn the_engine_reads_the_screens_of_one_theme_and_finds_files_under_the_app() {
        use crate::BuildState;
        use crate::testing::{engine_with_fakes, file};

        let (engine, fakes) = engine_with_fakes();
        let import = crate::Import {
            files: vec![
                file("charet.json", r#"{"app":"web"}"#),
                file("web/src/components/ui/button.tsx", "button"),
                file("web/src/list.tsx", "list"),
                file("other/src/button.tsx", "not the app's"),
            ],
            source: "test".into(),
            commit: None,
        };
        engine
            .create_project(&Actor::Local, "shop", "Shop", Some(import))
            .unwrap();
        engine
            .preview(&Actor::Local, "shop", "main", false)
            .unwrap();
        fakes.workspace.finish(BuildState::Ready);
        fakes.workspace.serve(".charet/manifest.json", MANIFEST);
        fakes
            .renderer
            .set_elements(json!([root(&["Button"], None, [10, 10, 80, 30], "Save")]));
        engine
            .capture(&Actor::Local, "shop", "main", &[], &["light".into()], false)
            .unwrap();

        let groups = engine.components(&Actor::Local, "shop", "light").unwrap();
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].name, "Button");
        assert_eq!(
            groups[0].file, "src/components/ui/button.tsx",
            "relative to the app, and not the button outside it"
        );
        assert_eq!(
            groups[0].uses, 2,
            "a use on each of the two screens, none from the story"
        );
        assert_eq!(
            engine.components(&Actor::Local, "shop", "dark").unwrap(),
            [],
            "nothing was captured in dark"
        );
        assert!(matches!(
            engine.components(&Actor::Local, "shop", "sepia"),
            Err(EngineError::Invalid(_))
        ));
        assert!(matches!(
            engine.components(&Actor::Local, "nothing", "light"),
            Err(EngineError::NotFound(_))
        ));
    }

    #[test]
    fn components_are_listed_by_folder_then_name() {
        let home = json!([
            root(&["Zed"], Some("src/a/Zed.tsx:1:1"), [1, 1, 10, 10], ""),
            root(&["Alpha"], Some("src/b/Alpha.tsx:1:1"), [1, 20, 10, 10], ""),
            root(&["Beta"], Some("src/a/Beta.tsx:1:1"), [1, 40, 10, 10], ""),
        ]);
        let names: Vec<_> = gather(&[(capture("home"), home)], &[])
            .into_iter()
            .map(|g| g.name)
            .collect();
        assert_eq!(names, ["Beta", "Zed", "Alpha"]);
    }
}
