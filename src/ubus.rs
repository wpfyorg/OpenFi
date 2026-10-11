//! One interface over three ubus transports:
//! - `Cli`: spawn `ubus call` on the local box (controller node).
//! - `Rpcd`: JSON-RPC to `http://<host>/ubus` (uhttpd-mod-ubus, already there with LuCI).
//! - `Fixture`: canned JSON from disk so the UI can be built on a laptop.
//!
//! TODO(perf): replace `Cli` with a native ubusd socket client (blobmsg) to avoid a fork per call.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::http;

pub type Result<T> = std::result::Result<T, String>;

const NULL_SESSION: &str = "00000000000000000000000000000000";

#[derive(Clone)]
pub enum Ubus {
    Cli,
    Rpcd(Arc<Rpcd>),
    Fixture { dir: PathBuf, started: Instant },
}

pub struct Rpcd {
    host: String,
    username: Option<String>,
    password: Option<String>,
    session: Mutex<Option<String>>,
}

impl Ubus {
    pub fn rpcd(host: String, username: Option<String>, password: Option<String>) -> Self {
        Ubus::Rpcd(Arc::new(Rpcd {
            host,
            username,
            password,
            session: Mutex::new(None),
        }))
    }

    pub fn fixture(dir: PathBuf) -> Self {
        Ubus::Fixture {
            dir,
            started: Instant::now(),
        }
    }

    pub async fn call(&self, object: &str, method: &str, args: Value) -> Result<Value> {
        match self {
            Ubus::Cli => {
                let out = tokio::process::Command::new("ubus")
                    .args(["call", object, method, &args.to_string()])
                    .output()
                    .await
                    .map_err(|e| format!("ubus: {e}"))?;
                if !out.status.success() {
                    return Err(format!(
                        "ubus call {object} {method}: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
                if out.stdout.is_empty() {
                    return Ok(Value::Null);
                }
                serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
            }
            Ubus::Rpcd(r) => r.call(object, method, args).await,
            Ubus::Fixture { dir, started } => {
                let bytes = read_fixture(dir, object, method, &args).await?;
                let mut v: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                animate(&mut v, started.elapsed().as_secs_f64());
                Ok(v)
            }
        }
    }

    /// Eagerly authenticates (for `Rpcd`, calling `session login` right away instead of lazily
    /// on the first `call()`) so a caller — the login handler — gets an immediate pass/fail
    /// instead of deferring it to whatever the first real call happens to be. A no-op success
    /// for `Cli`/`Fixture`, which have nothing to authenticate.
    pub async fn ensure_session(&self) -> Result<()> {
        match self {
            Ubus::Rpcd(r) => r.session().await.map(|_| ()),
            Ubus::Cli | Ubus::Fixture { .. } => Ok(()),
        }
    }

    /// Best-effort `session destroy` on rpcd (logout); a no-op for `Cli`/`Fixture`. Errors are
    /// swallowed by the caller — a session the router already reaped is not a logout failure.
    pub async fn destroy_session(&self) {
        if let Ubus::Rpcd(_) = self {
            let _ = self.call("session", "destroy", json!({})).await;
        }
    }

    /// Object names matching a glob prefix, e.g. `hostapd.*`.
    pub async fn list(&self, pattern: &str) -> Result<Vec<String>> {
        match self {
            Ubus::Cli => {
                let out = tokio::process::Command::new("ubus")
                    .args(["list", pattern])
                    .output()
                    .await
                    .map_err(|e| format!("ubus: {e}"))?;
                Ok(String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect())
            }
            Ubus::Rpcd(r) => {
                let v = r.rpc("list", json!([pattern])).await?;
                Ok(v.as_object()
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default())
            }
            Ubus::Fixture { dir, .. } => {
                let prefix = pattern.trim_end_matches('*');
                let mut names = vec![];
                let mut rd = tokio::fs::read_dir(dir).await.map_err(|e| e.to_string())?;
                while let Ok(Some(e)) = rd.next_entry().await {
                    let f = e.file_name().to_string_lossy().into_owned();
                    if let Some(stem) = f.strip_suffix(".get_clients.json")
                        && stem.starts_with(prefix)
                    {
                        names.push(stem.to_owned());
                    }
                }
                names.sort();
                Ok(names)
            }
        }
    }
}

impl Rpcd {
    async fn call(&self, object: &str, method: &str, args: Value) -> Result<Value> {
        for attempt in 0..2 {
            let sid = self.session().await?;
            let res = self.rpc("call", json!([sid, object, method, args])).await?;
            // rpcd replies [code] or [code, data]; 6 = permission denied / session expired.
            let code = res.get(0).and_then(Value::as_u64).unwrap_or(1);
            match code {
                0 => return Ok(res.get(1).cloned().unwrap_or(Value::Null)),
                6 if attempt == 0 => *self.session.lock().await = None,
                _ => {
                    return Err(format!(
                        "rpcd {}: {object}.{method} -> code {code}",
                        self.host
                    ));
                }
            }
        }
        unreachable!()
    }

    async fn session(&self) -> Result<String> {
        let mut guard = self.session.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        let (Some(user), Some(pass)) = (&self.username, &self.password) else {
            return Ok(NULL_SESSION.into());
        };
        let res = self
            .rpc(
                "call",
                json!([NULL_SESSION, "session", "login", {"username": user, "password": pass}]),
            )
            .await?;
        let sid = res
            .get(1)
            .and_then(|d| d.get("ubus_rpc_session"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("rpcd {}: login failed", self.host))?
            .to_owned();
        *guard = Some(sid.clone());
        Ok(sid)
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let body =
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        let (status, bytes) = http::request(
            "POST",
            &self.host,
            "/ubus",
            &[("Content-Type", "application/json")],
            Some(body.as_bytes()),
        )
        .await?;
        if status != 200 {
            return Err(format!("rpcd {}: HTTP {status}", self.host));
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if let Some(err) = v.get("error") {
            return Err(format!("rpcd {}: {err}", self.host));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// Same object+method can mean different devices (`iwinfo info {device: "radio0"}` vs
/// `{device: "phy1-mesh0"}`), so a plain `{object}.{method}.json` isn't enough. If the call args
/// have one of a few well-known "which thing" keys, try `{object}.{method}.<value>.json` first
/// (e.g. `iwinfo.assoclist.phy1-mesh0.json`, `uci.get.wireless.json`) and fall back to the plain
/// name — this keeps every existing fixture (called with `{}` or an ignored arg) working.
async fn read_fixture(dir: &Path, object: &str, method: &str, args: &Value) -> Result<Vec<u8>> {
    if let Some(suffix) = fixture_suffix(args) {
        let file = dir.join(format!("{object}.{method}.{suffix}.json"));
        if let Ok(bytes) = tokio::fs::read(&file).await {
            return Ok(bytes);
        }
    }
    let file = dir.join(format!("{object}.{method}.json"));
    tokio::fs::read(&file)
        .await
        .map_err(|e| format!("{}: {e}", file.display()))
}

fn fixture_suffix(args: &Value) -> Option<String> {
    if let Some(s) = args.get("path").and_then(Value::as_str) {
        return Some(sanitize(s.trim_start_matches('/')));
    }
    for key in ["device", "config", "name", "iface", "command"] {
        if let Some(s) = args.get(key).and_then(Value::as_str) {
            return Some(sanitize(s));
        }
    }
    None
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Make fixture counters move so the mock UI shows live traffic.
fn animate(v: &mut Value, t: f64) {
    // Counters grow by the integral of rate·(1 + 0.4·sin(t/20)), so the derived rate stays within
    // ±40% of `rate` no matter how long the mock runs (growing by rate·t·wave made it unbounded).
    let integral = t + 8.0 - 8.0 * (t / 20.0).cos();
    let grow = |x: &mut Value, rate: f64| {
        if let Some(n) = x.as_f64() {
            *x = json!((n + rate * integral) as u64);
        }
    };
    if let Some(stats) = v.get_mut("statistics") {
        if let Some(rx) = stats.get_mut("rx_bytes") {
            grow(rx, 22_000_000.0);
        }
        if let Some(tx) = stats.get_mut("tx_bytes") {
            grow(tx, 2_800_000.0);
        }
    }
}
