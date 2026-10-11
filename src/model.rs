//! The snapshot the UI renders, plus the response types for the other read endpoints.
//! Serialized as-is (snake_case) to match `docs/api.md` exactly — that file is the contract,
//! shared with `web/src/types.ts`; change it first, then this file.

use std::collections::HashMap;

use serde::Serialize;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Snapshot {
    /// Unix seconds of the poll that produced this snapshot.
    pub ts: u64,
    pub wan: Wan,
    pub nodes: Vec<Node>,
    pub clients: Vec<Client>,
    /// nlbwmon per-client / per-application traffic accounting (gateway only, None if disabled).
    pub usage: Option<Usage>,
    /// AdGuard Home stats (gateway only, None if unreachable / not configured).
    pub dns: Option<Dns>,
    /// `qos-status` output (gateway only, None if the command is missing).
    pub qos: Option<Qos>,
    /// Newest first, capped to 50.
    pub events: Vec<Event>,
    pub health: Health,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Wan {
    /// WAN address, e.g. `192.168.225.59` (double-NAT behind the Jio ONT).
    pub ip: Option<String>,
    /// Upstream gateway, e.g. the ONT at `192.168.225.1`.
    pub gateway: Option<String>,
    pub rx_bps: u64,
    pub tx_bps: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// Bytes over the last 24h: sum of each 24h history bucket's `rate * bucket_secs / 8`
    /// (including the in-progress bucket), not a separate running counter.
    pub window_rx_bytes: u64,
    pub window_tx_bytes: u64,
    /// Rolling 24h history, one point per `HISTORY_BUCKET_SECS`, oldest first.
    pub history: Vec<RatePoint>,
}

#[derive(Debug, Default, Clone, Copy, Serialize, serde::Deserialize)]
pub struct RatePoint {
    pub ts: u64,
    pub rx_bps: u64,
    pub tx_bps: u64,
}

#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Gateway,
    /// Never actually the "real" state: `system::collect` always sets the true role right
    /// after constructing the `Node` with `..Default::default()`.
    #[default]
    Ap,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Node {
    pub name: String,
    pub host: String,
    pub role: Role,
    pub online: bool,
    pub error: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub uptime: u64,
    pub load: [f32; 3],
    pub mem_total: u64,
    pub mem_available: u64,
    /// `/overlay` usage in bytes, from `system info`'s root/overlay fields. `None` on boards
    /// that don't report it (or when `system info` doesn't include the field at all — see the
    /// assumption noted in `collectors/system.rs`).
    pub overlay_used: Option<u64>,
    pub overlay_total: Option<u64>,
    pub temps: Vec<Temp>,
    pub radios: Vec<Radio>,
    /// Mesh peers + wired uplink (+ the WAN link, gateway only).
    pub links: Vec<Link>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Temp {
    /// Friendly label, e.g. "CPU", "WiFi 5 GHz"; raw sensor title when unknown.
    pub label: String,
    pub celsius: f32,
    /// Lowest "hot"/"critical" trip point, when the driver exposes one.
    pub crit: Option<f32>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Band {
    /// Only meaningful as a `Default` so `Client` (which needs one, for `..Default::default()`
    /// call sites) can derive it; the field is always wrapped in `Option`, so this is never
    /// actually shown as "the" band.
    #[default]
    #[serde(rename = "2.4")]
    Ghz24,
    #[serde(rename = "5")]
    Ghz5,
}

#[derive(Debug, Clone, Serialize)]
pub struct Radio {
    /// uci section name, e.g. "radio0".
    pub name: String,
    /// hostapd iface bound to this radio, e.g. "phy1-ap0". `None` if disabled or no AP iface.
    pub iface: Option<String>,
    pub band: Band,
    pub disabled: bool,
    /// SSIDs of every AP-mode wifi-iface on this radio (mesh ifaces are not SSIDs).
    pub ssids: Vec<String>,
    pub channel: Option<u32>,
    /// e.g. "HE80".
    pub htmode: Option<String>,
    pub txpower_dbm: Option<i32>,
    pub noise_dbm: Option<i32>,
    pub clients: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Mesh,
    Wired,
    Wan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Strong,
    Fair,
    Weak,
}

impl Quality {
    /// Mesh thresholds from the contract: > -65 dBm strong, > -78 dBm fair, else weak.
    pub fn from_mesh_signal(signal_dbm: i32) -> Self {
        if signal_dbm > -65 {
            Quality::Strong
        } else if signal_dbm > -78 {
            Quality::Fair
        } else {
            Quality::Weak
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Link {
    pub kind: LinkKind,
    /// Node name, or "ONT" for the WAN link.
    pub peer: String,
    /// Mesh only.
    pub signal_dbm: Option<i32>,
    pub rx_mbps: Option<u32>,
    pub tx_mbps: Option<u32>,
    pub quality: Quality,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Client {
    pub mac: String,
    pub ip: Option<String>,
    pub hostname: Option<String>,
    pub online: bool,
    pub wired: bool,
    /// Associated AP node (WiFi), or the node that sees it in its ARP/neigh table (wired).
    pub node: Option<String>,
    pub ssid: Option<String>,
    pub band: Option<Band>,
    pub signal_dbm: Option<i32>,
    /// PHY rates, bps.
    pub rx_rate: Option<u64>,
    pub tx_rate: Option<u64>,
    /// Has a static DHCP lease (`uci show dhcp` host section).
    pub reserved: bool,
    /// On the qos-tiers whitelist (exempt from shaping / monthly caps).
    pub whitelisted: bool,
    pub last_seen: Option<u64>,
}

/// Per-client and per-application traffic accounting for the current nlbwmon period
/// (a calendar month), from `nlbw -c json -g mac` / `-g mac,layer7`.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Usage {
    /// Network-wide current-period totals, calculated before the client display cap.
    pub total_rx_bytes: u64,
    pub total_tx_bytes: u64,
    /// Number of valid client records in the full nlbwmon result.
    pub client_count: u64,
    /// Top talkers this month, sorted by total bytes, capped to keep the snapshot small.
    pub clients: Vec<ClientUsage>,
    /// Protocol/app mix aggregated across all clients: top apps by bytes, plus "Other".
    pub apps: Vec<AppUsage>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ClientUsage {
    pub mac: String,
    /// Joined from `clients` (DHCP lease / host hint) by MAC.
    pub hostname: Option<String>,
    pub ip: Option<String>,
    /// Cumulative bytes for the current nlbwmon period.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub conns: u64,
    /// Current rate, derived from the delta between the last two nlbw samples.
    pub rx_bps: u64,
    pub tx_bps: u64,
    /// Up to 3 layer7 app names by bytes, e.g. `["HTTPS", "QUIC", "DNS/53"]`.
    pub top_apps: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct AppUsage {
    /// layer7 name if nlbwmon identified one, else a `PROTO/port` fallback.
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub conns: u64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Dns {
    pub queries: u64,
    pub blocked: u64,
    /// Milliseconds; AdGuard reports processing time in seconds.
    pub avg_ms: f64,
    pub top_blocked: Vec<TopBlocked>,
    /// Last 24h, oldest first.
    pub hourly_queries: Vec<u64>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct TopBlocked {
    pub domain: String,
    pub count: u64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Qos {
    pub enforcing_burst: bool,
    pub enforcing_monthly: bool,
    /// 0 = unlimited.
    pub shaper_dl_kbit: u64,
    pub shaper_ul_kbit: u64,
    pub clients: Vec<QosClient>,
    /// The monthly fair-use ladder, low to high. Every `clients[].tier` is one of these `id`s.
    pub tiers: Vec<QosTier>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct QosClient {
    pub mac: String,
    pub month_bytes: u64,
    pub tier: String,
    pub whitelisted: bool,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct QosTier {
    pub id: String,
    pub label: String,
    pub min_gb: f64,
    pub max_gb: Option<f64>,
    pub rate_mbit: Option<f64>,
    /// Count of `Qos.clients` currently in this tier.
    pub clients: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Roam,
    BandSteer,
    Joined,
    Left,
    NodeDown,
    NodeUp,
    WanDown,
    WanUp,
    Action,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub ts: u64,
    pub kind: EventKind,
    /// Human sentence, e.g. "Client 01 moved from ap-2 to ap-4".
    pub text: String,
    pub node: Option<String>,
    pub mac: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    #[default]
    Ok,
    Degraded,
    Down,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Health {
    pub status: HealthStatus,
    pub reasons: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// `GET /wifi/networks` — not part of the snapshot; built on demand from `uci show wireless`
// across every node, so it always reflects the config actions will write to.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Primary,
    Iot,
    Backhaul,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Security {
    #[serde(rename = "sae-mixed")]
    SaeMixed,
    #[serde(rename = "psk2")]
    Psk2,
    #[serde(rename = "sae")]
    Sae,
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct BandBroadcast {
    #[serde(rename = "2.4")]
    pub ghz24: Option<bool>,
    #[serde(rename = "5")]
    pub ghz5: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pmf {
    Off,
    Optional,
    Required,
}

#[derive(Debug, Clone, Serialize)]
pub struct Roaming {
    pub ft: bool,
    pub ft_over_ds: bool,
    pub mobility_domain: Option<String>,
    pub rrm: bool,
    pub btm: bool,
    pub pmf: Pmf,
    pub usteer: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WifiNetwork {
    /// uci section name shared across nodes, e.g. "default_radio1".
    pub id: String,
    pub ssid: String,
    pub purpose: Purpose,
    /// Backhaul is read-only in the UI.
    pub locked: bool,
    pub security: Security,
    pub bands: Vec<Band>,
    /// node name -> band -> broadcasting; `None` = radio off/absent on that node.
    pub broadcast: HashMap<String, BandBroadcast>,
    pub roaming: Roaming,
    pub clients: usize,
}

// ---------------------------------------------------------------------------------------------
// Other read-endpoint response rows.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct DailyUsage {
    /// "YYYY-MM-DD".
    pub day: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub devices: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Err,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub ts: u64,
    pub node: String,
    pub level: LogLevel,
    pub source: String,
    pub msg: String,
}

// ---------------------------------------------------------------------------------------------
// Multi-node WiFi change ("apply plan") — `POST /wifi/networks/{id}/plan`,
// `POST /wifi/plans/{id}/apply`, `GET /wifi/plans/{id}`.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PlanDiff {
    pub node: String,
    /// `uci set` lines, secrets masked (`key='••••••••'`).
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub id: String,
    pub network: String,
    /// APs first (mesh-only APs before wired-uplink APs), gateway last.
    pub order: Vec<String>,
    pub diff: Vec<PlanDiff>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanRunState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Queued,
    Applying,
    Confirming,
    Confirmed,
    RolledBack,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepStatus {
    pub node: String,
    pub status: StepState,
    pub deadline: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanStatus {
    pub id: String,
    pub state: PlanRunState,
    pub steps: Vec<StepStatus>,
}
