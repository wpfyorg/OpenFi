# OpenWrtFi API contract (v1)

Single source of truth for backend (`src/`) and frontend (`web/src/`). Change this file first,
then both sides. Types are TypeScript. Rust structs in `src/model.rs` serialize to exactly these
shapes (snake_case). All times are unix seconds. Byte counters are u64, rates are bits per second.

## Conventions

- Base path `/api/v1`. JSON in and out. Errors: HTTP 4xx/5xx with `{ "error": string }`.
- **Auth.** Every route except `POST /auth/login` and `GET /health` requires the
  `owfi_session` cookie (HttpOnly, SameSite=Strict, Path=/). Missing or expired → `401`.
- **Writes** (every non-GET) must also send header `X-OpenWrtFi: 1` (CSRF guard). Missing → `403`.
- **Mock mode** (`--mock fixtures`): login accepts `root` / `mock`. Writes succeed without touching
  any router, are recorded in the audit log, and mutate in-memory mock state where it is cheap
  (e.g. whitelist toggle, radio disabled), so the UI visibly reacts.

## Live data

`GET /snapshot` → `Snapshot`. `GET /stream` → SSE, event `snapshot`, data `Snapshot`, once per poll.

```ts
interface Snapshot {
  ts: number;
  wan: Wan;
  nodes: Node[];
  clients: Client[];
  usage: Usage | null;        // nlbwmon, monthly per-client accounting and full-network totals
  dns: Dns | null;            // AdGuard Home
  qos: Qos | null;            // qos-tiers
  events: Event[];            // newest first, max 50
  health: Health;
}

interface Wan {
  ip: string | null;          // WAN address (e.g. 192.168.225.59)
  gateway: string | null;     // upstream (Jio ONT 192.168.225.1)
  rx_bps: number; tx_bps: number;
  rx_bytes: number; tx_bytes: number;   // since boot
  window_rx_bytes: number; window_tx_bytes: number;   // last 24 h (sum of history buckets)
  history: RatePoint[];       // 24 h, 30-min buckets (oldest first)
}
interface RatePoint { ts: number; rx_bps: number; tx_bps: number }

interface Node {
  name: string; host: string;
  role: "gateway" | "ap";
  online: boolean; error: string | null;
  model: string | null; firmware: string | null;
  uptime: number; load: [number, number, number];
  mem_total: number; mem_available: number;
  overlay_used: number | null; overlay_total: number | null;   // bytes, from `system info` root/overlay
  temps: Temp[];
  radios: Radio[];
  links: Link[];              // mesh peers + wired uplink
}
interface Temp { label: string; celsius: number; crit: number | null }
interface Radio {
  name: string;               // uci section, e.g. "radio0"
  iface: string | null;       // hostapd iface, e.g. "phy1-ap0"
  band: "2.4" | "5";
  disabled: boolean;
  ssids: string[];
  channel: number | null; htmode: string | null;       // "HE80"
  txpower_dbm: number | null; noise_dbm: number | null;
  clients: number;
}
interface Link {
  kind: "mesh" | "wired" | "wan";
  peer: string;               // node name, or "ONT"
  signal_dbm: number | null;  // mesh only
  rx_mbps: number | null; tx_mbps: number | null;
  quality: "strong" | "fair" | "weak";   // mesh: > -65 strong, > -78 fair, else weak; wired: strong
}

interface Client {
  mac: string; ip: string | null; hostname: string | null;
  online: boolean;
  wired: boolean;
  node: string | null;        // associated AP node (WiFi) or the node that sees it in neigh (wired)
  ssid: string | null; band: "2.4" | "5" | null;
  signal_dbm: number | null;
  rx_rate: number | null; tx_rate: number | null;     // PHY rates, bps
  reserved: boolean;          // has a static DHCP lease
  whitelisted: boolean;       // qos-tiers whitelist
  last_seen: number | null;
}

interface ClientUsage {
  mac: string; hostname: string | null; ip: string | null;
  rx_bytes: number; tx_bytes: number; conns: number;
  rx_bps: number; tx_bps: number;
  top_apps: string[];
}
interface AppUsage { name: string; rx_bytes: number; tx_bytes: number; conns: number }

interface Usage {
  total_rx_bytes: number;     // totals across all valid nlbwmon client records
  total_tx_bytes: number;
  client_count: number;       // number of valid client records before the display cap
  clients: ClientUsage[];     // top 50 talkers, sorted by total bytes
  apps: AppUsage[];
}

interface Dns {
  queries: number; blocked: number; avg_ms: number;
  top_blocked: { domain: string; count: number }[];
  hourly_queries: number[];   // last 24 h, oldest first
}

interface Qos {
  enforcing_burst: boolean; enforcing_monthly: boolean;
  shaper_dl_kbit: number; shaper_ul_kbit: number;    // 0 = unlimited
  // Monthly fair-use ladder from /etc/device-qos.conf; ordered low to high. max_gb null = open-ended.
  tiers: { id: string; label: string; min_gb: number; max_gb: number | null; rate_mbit: number | null; clients: number }[];
  // If qos-status has an entry for a MAC, its whitelist value overrides the legacy flat file.
  // The legacy file supplies the value only for MACs absent from qos-status.
  clients: { mac: string; month_bytes: number; tier: string; whitelisted: boolean }[];   // tier = tiers[].id
}

interface Event {
  ts: number;
  kind: "roam" | "band_steer" | "joined" | "left" | "node_down" | "node_up" | "wan_down" | "wan_up" | "action";
  text: string;               // human sentence, e.g. "Client 01 moved from ap-2 to ap-4"
  node: string | null; mac: string | null;
}

interface Health { status: "ok" | "degraded" | "down"; reasons: string[] }
```

## Read endpoints

| Route | Returns |
|---|---|
| `GET /health` | `"ok"` (no auth) |
| `GET /history?range=24h\|7d` | `RatePoint[]` (24h: 30-min buckets; 7d: 3-h buckets, from RAM ring + `/tmp/openwrtfi-history.json`) |
| `GET /usage/daily?days=30` | `{ day: "YYYY-MM-DD", rx_bytes, tx_bytes, devices }[]` from OpenWrtFi's WAN-counter rollup |
| `GET /wifi/networks` | `WifiNetwork[]` (see below) |
| `GET /logs?node=<name>&lines=200` | `{ ts, node, level: "info"\|"warn"\|"err", source, msg }[]` from `logread` |
| `GET /audit` | `{ ts, user, action, target, result: "ok"\|"error", detail }[]` newest first |

Daily usage accumulates deltas from the gateway WAN interface counters and counts distinct devices
observed online each local calendar day. The local day uses `local_utc_offset_secs` from the
controller configuration. OpenWrtFi persists the completed days and in-progress day, including
its device set and WAN counter baseline, in `daily_usage_file` (default `/etc/openwrtfi/daily.json`);
older files containing only completed days remain supported. At most 90 completed days are kept.
Days before the controller first observed them are omitted. The router's
`/etc/usage-history/*.csv` files contain monthly per-device records and are not the source for
this daily endpoint.

```ts
interface WifiNetwork {
  id: string;                 // uci section name shared across nodes, e.g. "default_radio1"
  ssid: string;
  purpose: "primary" | "iot" | "backhaul";
  locked: boolean;            // backhaul is read-only in the UI
  security: "sae-mixed" | "psk2" | "sae" | "none";
  bands: ("2.4" | "5")[];
  broadcast: Record<string, { "2.4": boolean | null; "5": boolean | null }>;  // node -> band -> on; null = radio off/absent
  roaming: { ft: boolean; ft_over_ds: boolean; mobility_domain: string | null; rrm: boolean; btm: boolean; pmf: "off" | "optional" | "required"; usteer: boolean };
  clients: number;
}
```

## Diagnostics

`POST /diag` `{ tool: "ping" | "traceroute" | "nslookup", host: string, node?: string }`
→ `{ output: string, exit_code: number }`. `host` must match `^[A-Za-z0-9.:-]{1,253}$`; never shell-interpolated.

## Auth

| Route | Body | Result |
|---|---|---|
| `POST /auth/login` | `{ username, password }` | `200 { user }` + Set-Cookie, or `401` |
| `GET /auth/me` | | `{ user, expires }` or `401` |
| `POST /auth/logout` | | `204`, cookie cleared |

Login validates with `ubus call session login` on the gateway (the LuCI root password). The session
also logs in to each AP's rpcd with the same credentials, so remote writes run under the user's own
rpcd sessions and existing ACLs. Sessions expire after 30 min idle.

## Actions (all require session + `X-OpenWrtFi: 1`; each writes an audit entry)

| Route | Body | Effect |
|---|---|---|
| `POST /clients/{mac}/kick` | `{ ban_secs?: number }` | `hostapd.<iface> del_client {addr, reason:5, deauth:true, ban_time}` on its AP |
| `POST /clients/{mac}/reserve` | `{ ip: string, name: string }` | `uci add dhcp host` → apply with rollback → confirm |
| `POST /clients/{mac}/whitelist` | `{ on: boolean }` | `qos-tiers whitelist <MAC> on\|off` |
| `POST /nodes/{node}/reboot` | | `system reboot` |
| `POST /nodes/{node}/wifi/reload` | | `network.wireless` reload |
| `POST /nodes/{node}/radios/{radio}` | `{ disabled?: boolean, channel?: number \| "auto", txpower_dbm?: number }` | `uci set wireless.<radio>.*` → apply with rollback → confirm |

### Multi-node WiFi change (the "apply plan")

1. `POST /wifi/networks/{id}/plan` `{ ssid?, key?, security?, broadcast?, roaming? }`
   → `Plan { id, network, order: string[], diff: { node: string, lines: string[] }[] }`.
   Secrets in `lines` are masked (`key='••••••••'`). Order is APs first, gateway last.
2. `POST /wifi/plans/{id}/apply` → `202 Plan`. Rollout runs in the background, one node at a time:
   `uci set …` → `uci apply {rollback:true, timeout:30}` → the controller re-reaches the node over
   rpcd → `uci confirm`. If the node is unreachable within the window it auto-reverts and the rollout
   stops (`rolled_back`); later nodes stay `skipped`.
3. `GET /wifi/plans/{id}` → `PlanStatus { id, state: "running"|"done"|"failed", steps: { node, status: "queued"|"applying"|"confirming"|"confirmed"|"rolled_back"|"failed"|"skipped", deadline: number|null, error: string|null }[] }`.

Refuses (`409`) while any target node is offline or another plan is running.

## Browser shell (off by default)

`POST /shell/token` `{ password }` → `{ token }` (step-up re-auth, one-time, 60 s TTL, bound to the session).

`GET /shell/{node}` upgrades to a WebSocket proxied to `ttyd` on `127.0.0.1:7681` of that node.
Returns `403` unless `shell.enabled` is true in `/etc/openwrtfi.json`, the request is over HTTPS or
from localhost, a valid session exists, the `Origin` matches the host, and a one-time
`?token=` from `POST /shell/token {password}` (step-up re-auth, 60 s TTL) is supplied.
