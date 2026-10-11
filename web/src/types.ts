// Mirrors docs/api.md exactly — that file is the source of truth. Keep in sync with src/model.rs.
// Every field the backend can omit is typed nullable; components must render sane fallbacks.

export interface RatePoint { ts: number; rx_bps: number; tx_bps: number }

export interface Wan {
  ip: string | null;
  gateway: string | null;
  rx_bps: number; tx_bps: number;
  rx_bytes: number; tx_bytes: number;          // since boot
  window_rx_bytes: number; window_tx_bytes: number;   // last 24 h
  history: RatePoint[];
}

export interface Temp { label: string; celsius: number; crit: number | null }

export type Band = "2.4" | "5";

export interface Radio {
  name: string;
  iface: string | null;
  band: Band;
  disabled: boolean;
  ssids: string[];
  channel: number | null; htmode: string | null;
  txpower_dbm: number | null; noise_dbm: number | null;
  clients: number;
}

export type LinkKind = "mesh" | "wired" | "wan";
export type LinkQuality = "strong" | "fair" | "weak";

export interface Link {
  kind: LinkKind;
  peer: string;
  signal_dbm: number | null;
  rx_mbps: number | null; tx_mbps: number | null;
  quality: LinkQuality;
}

export type NodeRole = "gateway" | "ap";

export interface Node {
  name: string; host: string;
  role: NodeRole;
  online: boolean; error: string | null;
  model: string | null; firmware: string | null;
  uptime: number; load: [number, number, number];
  mem_total: number; mem_available: number;
  overlay_used: number | null; overlay_total: number | null;
  temps: Temp[];
  radios: Radio[];
  links: Link[];
}

export interface Client {
  mac: string; ip: string | null; hostname: string | null;
  online: boolean;
  wired: boolean;
  node: string | null;
  ssid: string | null; band: Band | null;
  signal_dbm: number | null;
  rx_rate: number | null; tx_rate: number | null;
  reserved: boolean;
  whitelisted: boolean;
  last_seen: number | null;
}

export interface ClientUsage {
  mac: string; hostname: string | null; ip: string | null;
  rx_bytes: number; tx_bytes: number; conns: number;
  rx_bps: number; tx_bps: number;
  top_apps: string[];
}
export interface AppUsage { name: string; rx_bytes: number; tx_bytes: number; conns: number }
export interface Usage {
  total_rx_bytes: number; total_tx_bytes: number; client_count: number;
  clients: ClientUsage[]; apps: AppUsage[];
}

export interface Dns {
  queries: number; blocked: number; avg_ms: number;
  top_blocked: { domain: string; count: number }[];
  hourly_queries: number[];
}

export interface QosClient { mac: string; month_bytes: number; tier: string; whitelisted: boolean }
/** One rung of the monthly fair-use ladder; max_gb null = open-ended, rate_mbit null = uncapped. */
export interface QosTier {
  id: string; label: string; min_gb: number; max_gb: number | null; rate_mbit: number | null; clients: number;
}
export interface Qos {
  enforcing_burst: boolean; enforcing_monthly: boolean;
  shaper_dl_kbit: number; shaper_ul_kbit: number;   // 0 = unlimited
  tiers: QosTier[];
  clients: QosClient[];                             // tier = tiers[].id
}

export type EventKind =
  | "roam" | "band_steer" | "joined" | "left"
  | "node_down" | "node_up" | "wan_down" | "wan_up" | "action";

export interface Event {
  ts: number;
  kind: EventKind;
  text: string;
  node: string | null; mac: string | null;
}

export type HealthStatus = "ok" | "degraded" | "down";
export interface Health { status: HealthStatus; reasons: string[] }

export interface Snapshot {
  ts: number;
  wan: Wan;
  nodes: Node[];
  clients: Client[];
  usage: Usage | null;
  dns: Dns | null;
  qos: Qos | null;
  events: Event[];
  health: Health;
}

// ---- read endpoints ----

export interface DailyUsage { day: string; rx_bytes: number; tx_bytes: number; devices: number }

export type WifiPurpose = "primary" | "iot" | "backhaul";
export type WifiSecurity = "sae-mixed" | "psk2" | "sae" | "none";
export type Pmf = "off" | "optional" | "required";

export interface WifiRoaming {
  ft: boolean; ft_over_ds: boolean; mobility_domain: string | null;
  rrm: boolean; btm: boolean; pmf: Pmf; usteer: boolean;
}

export interface WifiNetwork {
  id: string;
  ssid: string;
  purpose: WifiPurpose;
  locked: boolean;
  security: WifiSecurity;
  bands: Band[];
  broadcast: Record<string, { "2.4": boolean | null; "5": boolean | null }>;
  roaming: WifiRoaming;
  clients: number;
}

export type LogLevel = "info" | "warn" | "err";
export interface LogLine { ts: number; node: string; level: LogLevel; source: string; msg: string }

export type AuditResult = "ok" | "error";
export interface AuditEntry { ts: number; user: string; action: string; target: string; result: AuditResult; detail: string | null }

// ---- diagnostics ----

export type DiagTool = "ping" | "traceroute" | "nslookup";
export interface DiagRequest { tool: DiagTool; host: string; node?: string }
export interface DiagResult { output: string; exit_code: number }

// ---- auth ----

export interface LoginRequest { username: string; password: string }
export interface LoginResponse { user: string }
export interface MeResponse { user: string; expires: number }

// ---- actions ----

export interface KickRequest { ban_secs?: number }
export interface ReserveRequest { ip: string; name: string }
export interface WhitelistRequest { on: boolean }
export interface RadioPatch { disabled?: boolean; channel?: number | "auto"; txpower_dbm?: number }

// ---- multi-node wifi plan ----

export interface WifiPlanRequest {
  ssid?: string; key?: string; security?: WifiSecurity;
  broadcast?: Record<string, { "2.4"?: boolean; "5"?: boolean }>;
  roaming?: Partial<WifiRoaming>;
}

export interface PlanDiff { node: string; lines: string[] }
export interface Plan { id: string; network: string; order: string[]; diff: PlanDiff[] }

export type PlanStepStatus =
  | "queued" | "applying" | "confirming" | "confirmed" | "rolled_back" | "failed" | "skipped";
export type PlanState = "running" | "done" | "failed";

export interface PlanStep { node: string; status: PlanStepStatus; deadline: number | null; error: string | null }
export interface PlanStatus { id: string; state: PlanState; steps: PlanStep[] }

// ---- error shape ----

export interface ApiError { error: string }
