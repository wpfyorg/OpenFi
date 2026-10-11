use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ubus::Ubus;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Seconds between polls of every node.
    pub poll_secs: u64,
    /// Network device whose counters are the internet link.
    pub wan_device: String,
    pub dhcp_leases: PathBuf,
    pub nodes: Vec<NodeConfig>,
    pub adguard: Option<AdGuardConfig>,
    /// Command printing the qos-tiers JSON (see home-router-setup `qos-status`).
    pub qos_status_cmd: Option<String>,
    /// The monthly fair-use ladder (min/max GB, throttled rate). Falls back to the hardcoded
    /// ladder from AGENTS.md if this file is missing or doesn't parse.
    pub device_qos_conf: PathBuf,
    /// `nlbw` binary (or wrapper) used as `<nlbw_cmd> -c json -g <group>`; None disables
    /// per-client usage accounting entirely.
    pub nlbw_cmd: Option<String>,
    /// Mock mode: read `<dir>/nlbw.<group>.json` instead of running `nlbw_cmd`.
    #[serde(skip)]
    pub nlbw_fixtures: Option<PathBuf>,
    /// Per-device *monthly* usage CSVs (`mac,name,ip,dl_bytes,ul_bytes`), one per month
    /// (`YYYY-MM.csv`) — verified on mainrouter (2026-09-24): there is no daily source on the
    /// router at all, so this is *not* used for `GET /usage/daily` (see `daily_usage_file`
    /// below, which is this binary's own rollup). Kept for a future per-device-per-month view.
    pub usage_history_dir: PathBuf,
    /// Mock mode: read AdGuard's `/control/stats` shape from this file instead of over HTTP.
    #[serde(skip)]
    pub dns_fixture: Option<PathBuf>,
    /// Where the 24h/7d rate history survives a restart.
    pub history_file: PathBuf,
    /// Where the last 90 days of `GET /usage/daily` rollup survive a restart (our own daily
    /// aggregation — there's no daily source on the router, see `collectors::usage_daily`).
    pub daily_usage_file: PathBuf,
    /// Seconds east of UTC, for deciding when "today" rolls over to "yesterday" in the daily
    /// usage rollup. There's no timezone database in this binary, so this is the only knob;
    /// `0` (the default) rolls over at UTC midnight.
    pub local_utc_offset_secs: i64,
    /// The browser-shell feature gate (`GET /shell/{node}`) — off by default; see
    /// `docs/api.md`'s "Browser shell" section and `src/shell.rs`.
    #[serde(default)]
    pub shell: ShellConfig,
    /// Where the audit log's JSON-lines mirror lives.
    pub audit_log: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShellConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeConfig {
    pub name: String,
    pub host: String,
    /// "local" = `ubus` CLI on this box; "rpcd" = JSON-RPC at http://<host>/ubus.
    #[serde(default = "default_transport")]
    pub transport: String,
    /// rpcd login (a dedicated read-only ACL user is recommended — see plan.md).
    pub username: Option<String>,
    pub password: Option<String>,
    /// Net device name of this node's wired uplink to the gateway (e.g. "lan1" on ap-2), if any.
    /// `None` for nodes that only reach the gateway over the mesh.
    pub wired_uplink: Option<String>,
    #[serde(skip)]
    pub fixtures: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdGuardConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

fn default_transport() -> String {
    "rpcd".into()
}

impl Default for Config {
    fn default() -> Self {
        let node =
            |name: &str, host: &str, transport: &str, wired_uplink: Option<&str>| NodeConfig {
                name: name.into(),
                host: host.into(),
                transport: transport.into(),
                username: None,
                password: None,
                wired_uplink: wired_uplink.map(str::to_owned),
                fixtures: None,
            };
        Self {
            poll_secs: 2,
            wan_device: "wan".into(),
            dhcp_leases: "/tmp/dhcp.leases".into(),
            nodes: vec![
                node("mainrouter", "127.0.0.1", "local", None),
                node("ap-2", "192.168.1.3", "rpcd", Some("lan1")),
                node("ap-4", "192.168.1.5", "rpcd", None),
            ],
            adguard: Some(AdGuardConfig {
                url: "http://127.0.0.1:3000".into(),
                username: None,
                password: None,
            }),
            qos_status_cmd: Some("/usr/sbin/qos-status".into()),
            device_qos_conf: "/etc/device-qos.conf".into(),
            nlbw_cmd: Some("nlbw".into()),
            nlbw_fixtures: None,
            usage_history_dir: "/etc/usage-history".into(),
            dns_fixture: None,
            history_file: "/tmp/openwrtfi-history.json".into(),
            daily_usage_file: "/etc/openwrtfi/daily.json".into(),
            local_utc_offset_secs: 0,
            shell: ShellConfig::default(),
            audit_log: "/tmp/openwrtfi-audit.log".into(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!("bad config {}: {e}; using defaults", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Point every node (and the local file sources) at `<dir>/<node>/`.
    pub fn use_fixtures(&mut self, dir: PathBuf) {
        for n in &mut self.nodes {
            n.fixtures = Some(dir.join(&n.name));
        }
        self.dhcp_leases = dir.join("dhcp.leases");
        self.qos_status_cmd = Some(format!("cat {}", dir.join("qos-status.json").display()));
        self.device_qos_conf = dir.join("device-qos.conf");
        self.adguard = None;
        self.nlbw_fixtures = Some(dir.join("nlbw"));
        self.usage_history_dir = dir.join("usage-history");
        self.dns_fixture = Some(dir.join("adguard-stats.json"));
        // Mock runs write state; keep it out of the fixtures (they're checked-in test data).
        self.history_file = std::env::temp_dir().join("openwrtfi-mock-history.json");
        self.daily_usage_file = std::env::temp_dir().join("openwrtfi-mock-daily.json");
    }

    pub fn ubus_for(&self, n: &NodeConfig) -> Ubus {
        if let Some(dir) = &n.fixtures {
            return Ubus::fixture(dir.clone());
        }
        match n.transport.as_str() {
            "local" => Ubus::Cli,
            _ => Ubus::rpcd(n.host.clone(), n.username.clone(), n.password.clone()),
        }
    }
}
