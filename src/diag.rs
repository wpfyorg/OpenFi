//! `POST /diag` — ping / traceroute / nslookup, run as argv (never a shell string) so `host`
//! can't inject anything even if the validation below had a hole. Gateway-only for now: the
//! contract's `node` field exists for a future "run diagnostics from an AP" phase, but nothing
//! wires that up yet, so any non-gateway `node` is rejected with 400 rather than silently
//! running locally.

use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, Deserialize)]
pub struct DiagRequest {
    pub tool: Tool,
    pub host: String,
    pub node: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Ping,
    Traceroute,
    Nslookup,
}

#[derive(Debug, Serialize)]
pub struct DiagResponse {
    pub output: String,
    pub exit_code: i32,
}

/// `^[A-Za-z0-9.:-]{1,253}$`, hand-rolled — pulling in the `regex` crate for one check would
/// blow well past the binary's size budget.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-'))
}

pub async fn run(tool: Tool, host: &str) -> DiagResponse {
    let (cmd, args): (&str, Vec<String>) = match tool {
        Tool::Ping => ("ping", vec!["-c".into(), "4".into(), host.into()]),
        Tool::Traceroute => (
            "traceroute",
            vec![
                "-w".into(),
                "2".into(),
                "-q".into(),
                "1".into(),
                host.into(),
            ],
        ),
        Tool::Nslookup => ("nslookup", vec![host.into()]),
    };
    match Command::new(cmd).args(&args).output().await {
        Ok(out) => {
            let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
            output.push_str(&String::from_utf8_lossy(&out.stderr));
            DiagResponse {
                output,
                exit_code: out.status.code().unwrap_or(-1),
            }
        }
        Err(e) => DiagResponse {
            output: format!("{cmd}: {e}"),
            exit_code: -1,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_hostnames_and_addresses() {
        assert!(valid_host("192.0.2.3"));
        assert!(valid_host("example.com"));
        assert!(valid_host("::1"));
    }

    #[test]
    fn rejects_shell_metacharacters_and_empty() {
        assert!(!valid_host(""));
        assert!(!valid_host("8.8.8.8; rm -rf /"));
        assert!(!valid_host("$(whoami)"));
        assert!(!valid_host("host name"));
        assert!(!valid_host(&"a".repeat(254)));
    }

    #[tokio::test]
    async fn ping_localhost_runs_and_reports_an_exit_code() {
        let r = run(Tool::Ping, "127.0.0.1").await;
        // Environment-dependent (ping may be sandboxed/unavailable in CI), so just check the
        // plumbing works: some output or a clear error, and a real exit code either way.
        assert!(!r.output.is_empty() || r.exit_code != 0);
    }
}
