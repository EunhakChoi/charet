//! Tunnels to the remote door of a local charet (`docs/local.md`, "Share it from this machine").
//!
//! ngrok runs inside the server with the person's authtoken ([`Ngrok`]).
//!
//! Tailscale runs through its CLI (`tailscale serve` / `tailscale funnel`, `--bg` so the tunnel
//! outlives this process and comes back after a reboot). Works with Headscale too: without HTTPS
//! certificates on the tailnet, private sharing uses plain HTTP inside the tailnet.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use charet_engine::{ShareMode, TunnelMode, TunnelOptions, Tunnels};
use serde::Deserialize;

mod ngrok;

pub use crate::ngrok::Ngrok;

/// Every tunnel this machine can run.
#[derive(Debug)]
pub struct Machine {
    pub tailscale: Tailscale,
    pub ngrok: Ngrok,
}

impl Tunnels for Machine {
    fn modes(&self) -> Vec<TunnelMode> {
        let mut modes = self.tailscale.modes();
        modes.push(TunnelMode {
            mode: ShareMode::Ngrok,
            available: true,
            detail: String::new(),
        });
        modes
    }

    fn open(&self, mode: ShareMode, options: &TunnelOptions) -> Result<String, String> {
        match mode {
            ShareMode::Ngrok => self.ngrok.open(options),
            _ => self.tailscale.open(mode),
        }
    }

    fn close(&self, mode: ShareMode) -> Result<(), String> {
        match mode {
            ShareMode::Ngrok => self.ngrok.close(),
            _ => self.tailscale.close(mode),
        }
    }
}

/// How long a CLI call may take. `tailscale funnel` waits for someone to enable Funnel in the
/// admin console; we stop it and pass the link on instead.
const TIMEOUT: Duration = Duration::from_secs(15);
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// The tailscale CLI on this machine.
#[derive(Debug, Clone)]
pub struct Tailscale {
    /// The CLI (`CHARET_TAILSCALE`, default `tailscale` on `PATH`).
    pub cli: String,
    /// tailscaled's socket when not the default (`CHARET_TAILSCALE_SOCKET`).
    pub socket: Option<String>,
    /// Where tailscaled sends people: the remote door, e.g. `http://127.0.0.1:5284`.
    pub target: String,
}

/// What `tailscale status --json` says about this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    /// `kim.tail1234.ts.net` (or a Headscale name), without the trailing dot.
    name: String,
    /// Whether the tailnet issues HTTPS certificates (Tailscale yes, Headscale usually not).
    https: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Status {
    backend_state: String,
    #[serde(rename = "Self")]
    me: Option<Peer>,
    #[serde(default)]
    cert_domains: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Peer {
    #[serde(rename = "DNSName")]
    dns_name: String,
}

impl Tailscale {
    fn node(&self) -> Result<Node, String> {
        let out = self.run(&["status", "--json"], STATUS_TIMEOUT)?;
        let status: Status = serde_json::from_str(&out)
            .map_err(|e| format!("tailscale status gave something unexpected: {e}"))?;
        if status.backend_state != "Running" {
            return Err(format!(
                "Tailscale is not connected ({}); run tailscale up",
                status.backend_state
            ));
        }
        let name = status
            .me
            .map(|p| p.dns_name.trim_end_matches('.').to_owned())
            .filter(|n| !n.is_empty())
            .ok_or("this machine has no name on the tailnet (turn on MagicDNS)")?;
        Ok(Node {
            https: status.cert_domains.is_some_and(|d| !d.is_empty()),
            name,
        })
    }

    /// The `serve` / `funnel` arguments for `mode` on `node`, and the address it gives.
    fn plan(mode: ShareMode, node: &Node) -> Result<(&'static str, &'static str, String), String> {
        match mode {
            ShareMode::Tailscale if node.https => {
                Ok(("serve", "--https=443", format!("https://{}", node.name)))
            }
            ShareMode::Tailscale => Ok(("serve", "--http=80", format!("http://{}", node.name))),
            ShareMode::Funnel if node.https => {
                Ok(("funnel", "--https=443", format!("https://{}", node.name)))
            }
            ShareMode::Funnel => Err(
                "Funnel needs HTTPS certificates on the tailnet (Tailscale, not Headscale)".into(),
            ),
            ShareMode::Address | ShareMode::Ngrok | ShareMode::Lan => {
                Err("not a Tailscale mode".into())
            }
        }
    }

    /// Runs the CLI; stdout on success, otherwise what it said (with any admin-console link).
    fn run(&self, args: &[&str], timeout: Duration) -> Result<String, String> {
        let mut command = Command::new(&self.cli);
        if let Some(socket) = &self.socket {
            command.arg(format!("--socket={socket}"));
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn(&mut command).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                format!("{} not found: install Tailscale on this machine", self.cli)
            }
            _ => format!("cannot run {}: {e}", self.cli),
        })?;
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break Some(status);
            }
            if started.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let out = stdout.join().unwrap_or_default();
        let err = stderr.join().unwrap_or_default();
        match status {
            Some(status) if status.success() => Ok(out),
            _ => Err(explain(&format!("{out}\n{err}"), status.is_none())),
        }
    }
}

impl Tailscale {
    fn modes(&self) -> Vec<TunnelMode> {
        let node = self.node();
        [ShareMode::Tailscale, ShareMode::Funnel]
            .into_iter()
            .map(|mode| {
                let plan = node
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|n| Self::plan(mode, n));
                TunnelMode {
                    mode,
                    available: plan.is_ok(),
                    detail: match plan {
                        Ok((_, _, url)) => url,
                        Err(why) => why,
                    },
                }
            })
            .collect()
    }

    fn open(&self, mode: ShareMode) -> Result<String, String> {
        let node = self.node()?;
        let (command, port, url) = Self::plan(mode, &node)?;
        self.run(&[command, "--bg", "--yes", port, &self.target], TIMEOUT)?;
        Ok(url)
    }

    fn close(&self, mode: ShareMode) -> Result<(), String> {
        let node = self.node()?;
        let (command, port, _) = Self::plan(mode, &node)?;
        self.run(&[command, port, &self.target, "off"], TIMEOUT)
            .map(|_| ())
    }
}

/// Starts `command`, again if the program was being written a moment ago (`ETXTBSY`: another
/// thread forked while holding it open).
fn spawn(command: &mut Command) -> std::io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.raw_os_error() == Some(26) && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            result => return result,
        }
    }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    })
}

/// A short reason from the CLI's output. When it asks for something to be enabled in the admin
/// console, the link is what the person needs.
fn explain(output: &str, timed_out: bool) -> String {
    let link = output
        .split_whitespace()
        .find(|w| w.starts_with("https://login.tailscale.com/") || w.contains("/admin/"));
    if let Some(link) = link {
        return format!("enable it for this machine first, then try again: {link}");
    }
    if timed_out {
        return "tailscale did not answer in time".into();
    }
    let text = output.trim();
    if text.is_empty() {
        "tailscale failed".into()
    } else {
        text.lines().last().unwrap_or(text).trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The calls a stand-in CLI recorded. The folder it lives in goes away with this, so a test
    /// run leaves nothing in the temp folder.
    struct Calls(PathBuf);

    impl Calls {
        fn read(&self) -> String {
            std::fs::read_to_string(self.0.join("calls")).unwrap()
        }
    }

    impl Drop for Calls {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A stand-in CLI: answers `status --json` with `status`, records every call, and fails
    /// `funnel` with an admin-console link.
    fn fake(status: &str) -> (Tailscale, Calls) {
        let dir = std::env::temp_dir().join(format!(
            "charet-tunnel-{}-{}",
            std::process::id(),
            status.len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls");
        let _ = std::fs::remove_file(&log);
        let script = dir.join("tailscale");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" >> {log}\ncase \"$1 $2\" in\n  \"status --json\") echo '{status}' ;;\n  funnel*) [ \"$2\" = --bg ] && {{ echo 'Funnel is not enabled. To enable, visit:'; echo '  https://login.tailscale.com/f/funnel?node=n1'; exit 1; }} ;;\nesac\n",
                log = log.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (
            Tailscale {
                cli: script.display().to_string(),
                socket: None,
                target: "http://127.0.0.1:5284".into(),
            },
            Calls(dir),
        )
    }

    const TAILSCALE: &str = r#"{"BackendState":"Running","Self":{"DNSName":"kim.tail1.ts.net."},"CertDomains":["kim.tail1.ts.net"]}"#;
    const HEADSCALE: &str =
        r#"{"BackendState":"Running","Self":{"DNSName":"kim.hs.example."},"CertDomains":null}"#;

    #[test]
    fn serve_on_tailscale_uses_https_and_funnel_passes_on_the_enable_link() {
        let (tailscale, recorded) = fake(TAILSCALE);
        assert_eq!(
            tailscale.open(ShareMode::Tailscale).unwrap(),
            "https://kim.tail1.ts.net"
        );
        let err = tailscale.open(ShareMode::Funnel).unwrap_err();
        assert!(
            err.contains("https://login.tailscale.com/f/funnel?node=n1"),
            "{err}"
        );
        tailscale.close(ShareMode::Tailscale).unwrap();
        let calls = recorded.read();
        assert!(
            calls.contains("serve --bg --yes --https=443 http://127.0.0.1:5284"),
            "{calls}"
        );
        assert!(
            calls.contains("serve --https=443 http://127.0.0.1:5284 off"),
            "{calls}"
        );
        let modes = tailscale.modes();
        assert!(modes.iter().all(|m| m.available));
    }

    #[test]
    fn headscale_shares_over_http_inside_the_tailnet_and_has_no_funnel() {
        let (tailscale, recorded) = fake(HEADSCALE);
        assert_eq!(
            tailscale.open(ShareMode::Tailscale).unwrap(),
            "http://kim.hs.example"
        );
        assert!(recorded.read().contains("serve --bg --yes --http=80"));
        let modes = tailscale.modes();
        assert!(modes[0].available);
        assert!(!modes[1].available && modes[1].detail.contains("Headscale"));
    }

    #[test]
    fn a_missing_cli_says_to_install_tailscale() {
        let tailscale = Tailscale {
            cli: "/nonexistent/tailscale".into(),
            socket: None,
            target: String::new(),
        };
        let modes = tailscale.modes();
        assert!(!modes[0].available);
        assert!(
            modes[0].detail.contains("install Tailscale"),
            "{}",
            modes[0].detail
        );
    }
}
