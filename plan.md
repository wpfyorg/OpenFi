# OpenWrtFi — plan

A UniFi Network–style controller UI for this home OpenWrt mesh (mainrouter + ap-2 + ap-4,
OpenWrt 25.12.5 on JioRouter AX6000, see `../home-router-setup/AGENTS.md`).
It is a single static Rust binary that runs **on mainrouter**.

## Goals

- **Look and feel like UniFi, with a dense dark "looking glass" style**: the Dashboard, WiFi, Devices,
  Usage, and Topology screens are in `design/openwrtfi.pen`. The style uses mono numbers, area charts
  with upload below the axis, top-talker tables with share bars, and segmented range and refresh pickers.
- **Light enough to run on the router**: the binary is under 2 MB with the UI built in, RSS is
  under 8 MB, and CPU is under 1% at a 2 s poll. The gateway only has 495 MB of RAM, and
  AdGuardHome already uses about 60 MB of it.
- **Zero install on the APs**: they already expose ubus over HTTP via uhttpd/rpcd (LuCI), so the
  controller polls them remotely.
- **Read-first**: observability comes first. Write actions (kick, whitelist, reserve IP,
  restart) come later and always need confirmation.

Non-goals: replacing LuCI for configuration, supporting multiple sites, cloud access, or
long-term metrics storage on flash.

## Architecture

```
 browser ──HTTP/SSE──► openwrtfi (mainrouter :8088)
                          │  poll every 2 s (tokio, current_thread)
          ┌───────────────┼─────────────────────────────┐
          ▼               ▼                             ▼
   local `ubus` CLI   rpcd JSON-RPC http://ap-N/ubus   files / local APIs
   system, hostapd.*, (same calls, read-only ACL user)  /tmp/dhcp.leases
   network.device                                        AdGuard :3000/control/stats
                                                         /usr/sbin/qos-status
```

| Layer | Choice | Why |
|---|---|---|
| Server | Rust with axum (http1 only) and tokio on a single thread | Small, static musl build, no runtime needed on the router |
| Transport to nodes | `ubus` enum: `Cli` / `Rpcd` / `Fixture` (`src/ubus.rs`) | One code path for local, remote, and mock data |
| HTTP client | Hand-rolled HTTP/1.1 client (`src/http.rs`) with a `curl` HTTPS fallback for uhttpd redirects | Keeps the binary small while still working with OpenWrt's common `redirect_https=1`; rpcd request bodies go to curl over stdin so passwords never appear in argv |
| Push to UI | SSE sends a full snapshot on each poll | Snapshots are a few KB, so deltas aren't worth the complexity |
| UI | Preact + Vite + TS, embedded with rust-embed | About 7 KB gzipped today, with a budget of 60 KB |
| Charts | uPlot (~45 KB): area and line charts, drag-to-zoom | The design relies on area charts, and uPlot is the lightest option that does them well |
| Build | `cargo zigbuild --target aarch64-unknown-linux-musl` | Already installed, produces a static binary |

Measured on the scaffold: the release binary for aarch64-musl is **1.0 MB**, with the UI included.

## Data sources (what each screen needs)

| Data | Source | Nodes |
|---|---|---|
| Uptime, load, memory, model, firmware | `ubus call system info/board` | all |
| WAN rate and bytes | `network.device status {"name":"wan"}` (the difference between polls) | mainrouter |
| WiFi clients, signal, rates | `hostapd.<iface> get_clients` / `get_status` | all |
| Mesh links (dBm / Mbps) | `iwinfo assoclist` on `phy1-mesh0` (or `iw ... station dump`) | all |
| Roams and band-steers | `usteer get_clients / remote_hosts / local_info` | all |
| Client names and IPs | `/tmp/dhcp.leases`, plus static reservations from `uci show dhcp` | mainrouter |
| Wired clients and online state | `ip neigh` / `ubus call network.device status` on br-lan ports | mainrouter, ap-2 |
| DNS stats | AdGuard Home `/control/stats`, `/control/querylog` | mainrouter |
| Usage, FUP tiers, whitelist | `/usr/sbin/qos-status` JSON, `/etc/usage-history/*.csv` | mainrouter |
| Radio channel, txpower, noise | `iwinfo info` | all |

## Phases

### Phase 0: scaffold (done)
- Cargo project with a size-tuned release profile, cross-compiling to aarch64-musl.
- Poll loop with system, hostapd, DHCP lease, and WAN rate collectors, plus 24 h of history in RAM.
- `/api/v1/snapshot`, `/api/v1/stream` (SSE), `/api/v1/health`, and the embedded SPA.
- `--mock fixtures/` dev mode with animated counters.
- procd init script, rpcd ACL, sample config, and `scripts/deploy.sh`.
- Pen design with Dashboard, Topology, and Clients (with the detail drawer).

### Phase 1: real data on the real routers
- [ ] Create a read-only rpcd user `openwrtfi` on ap-2 and ap-4, install `rpcd-acl-openwrtfi.json`,
      and test with `curl http://192.0.2.3/ubus`.
- [ ] Deploy to mainrouter, then compare the snapshot against `ubus` output by hand.
- [x] Add mesh link collection (`iwinfo assoclist` on the mesh iface) and add a `Node.links` field to the model.
      Also added the wired uplink (ap-2 `lan1`) and the gateway's WAN link to the same field
      (`src/collectors/links.rs`), and a full `Node.radios` rebuild from `uci get wireless` +
      `iwinfo info` (`src/collectors/radios.rs`) since the old `Radio` shape didn't carry
      channel/txpower/noise/disabled.
- [x] Detect online state for wired clients (`ip neigh` REACHABLE/STALE).
      No ubus object exposes the neighbor table directly, so this reads `/proc/net/arp` through
      the `file` ubus object's `read` method instead (works locally and, through rpcd, on ap-2's
      wired ports too) — see the assumption noted in `src/collectors/clients.rs::read_arp`.
- [x] Map the real `qos-status` JSON shape into typed per-client usage and tier fields.
      Verified on mainrouter (2026-09-24) — `fixtures/qos-status.json` is the real captured
      output. `src/collectors/qos.rs` was rewritten around the real shape (`policy.fup`,
      `services.qos-tiers.running`, `devices[]`), with `#[serde(default)]` on every field for
      forward/backward tolerance rather than the old JSON-pointer-probing approach. The fair-use
      ladder now prefers `policy.fup` (what qos-tiers is actually enforcing right now) over
      `/etc/device-qos.conf` (real shell `KEY=VALUE` format, `FUP_GB_*`/`FUP_RATE_*` — also
      verified and reflected in `fixtures/device-qos.conf`), with the hardcoded AGENTS.md ladder
      as the last resort. `Qos.clients[].tier` is still *computed* from `monthly_bytes`, so it's
      always one of `Qos.tiers[].id` regardless of source. qos-status's own `devices[].online`/
      `whitelisted` are now also folded into `Client.online` (for ARP-ambiguous wired devices)
      and `Client.whitelisted` — see `collectors/mod.rs`.
- [x] Add AdGuard auth to the config and map stats into a typed `Dns` struct.
      Config already had `adguard.username`/`password`; added the typed `Dns` struct and parser
      in `src/collectors/dns.rs` (real, documented AdGuard `/control/stats` shape).
- [x] Overlay usage units. Verified on mainrouter (2026-09-24): `system info`'s `root` object is
      KiB (`{"total":103216,"free":78848,"used":24368,"avail":74012}`), not bytes —
      `collectors/system.rs` now multiplies by 1024 to match the contract's byte fields.
- [x] Wired-uplink speed shape. Verified on mainrouter: `network.device status {"name":"lan1"}`
      returns `"speed": "1000F"` (a string: Mbps + duplex letter), not a bare number —
      `collectors/links.rs::parse_speed` handles both shapes.
- [x] Daily usage rollup. Verified on mainrouter: `/etc/usage-history/<YYYY-MM>.csv` is
      per-device-per-month (`mac,name,ip,dl_bytes,ul_bytes`), not daily — there is no daily
      source anywhere on the router. `GET /usage/daily` is now served by this binary's own
      rollup (`collectors/usage_daily.rs::DailyTracker`): today's WAN rx/tx counter deltas and
      distinct online-device count are accumulated in memory, rolled into a completed day at
      local midnight (`Config.local_utc_offset_secs`, since there's no tzdata in this binary),
      and the last 90 days are persisted to `Config.daily_usage_file`
      (`/etc/openwrtfi/daily.json`) at most once an hour and on shutdown. Days before this
      binary was ever running are simply absent from the response, not zero-filled.
- [ ] Measure RSS and CPU on mainrouter (`/proc/<pid>/status`, `top`) and record them here.

### Phase 2: UI parity with the design
- [x] App shell, hash router, sidebar, top bar, and search (`/` focuses it).
- [x] Dashboard: stat band (4 stats), bandwidth area chart with a 7-day brush, top talkers, nodes & mesh,
      fair-use tiers, DNS sparkline. Charts use uPlot, which handles zoom, brush, and mirrored series.
- [x] WiFi: client distribution per AP (stacked), throughput per AP, radios table, mesh backhaul links.
- [x] Devices: top talkers over time and the all-devices table (connection, rates, month, QoS tier, share).
- [x] Usage: per-day bars, daily table, monthly fair-use per device.
- [x] Topology: node graph (SVG, laid out from live links) with a node card (CPU, memory, overlay, temps, radios, links, actions). Top processes not yet.
- [x] Traffic, DNS, and QoS pages. Plus Logs (logread, diagnostics, audit) and WiFi networks settings.
- [x] Mobile layout: single column, sidebar becomes a drawer. (Light theme not done; dark only.)

### Phase 3: auth and actions
- [x] Login checks credentials against rpcd (`session login` over a private loopback-only uhttpd
      bridge at `127.0.0.1:8089/ubus`, never the `ubus` CLI, so the password never touches
      argv/`ps`) and issues an HttpOnly session
      cookie (`src/session.rs`, `src/auth.rs`). Every `/api/v1` route requires it except
      `POST /auth/login` and `GET /health`; unmatched `/api/v1/...` paths still 401 first, then
      404 JSON once authenticated (`src/api.rs`'s `fallback`). Login also signs into each AP's
      rpcd with the same credentials, marking a node read-only for the session if that fails,
      and is rate-limited (5 failures/min/IP, 429 + `Retry-After`).
- [x] Actions, all in `src/actions.rs` (confirmation dialogs are a UI concern, not wired here):
  - kick a client (`hostapd.<iface> del_client`, looked up from the live snapshot)
  - whitelist toggle (`file exec /usr/sbin/qos-tiers whitelist <MAC> on|off`)
  - reserve an IP (`uci add dhcp host` → apply/rollback → confirm; warns, but doesn't block, on
    an IP inside the DHCP pool `.100`-`.239`)
  - restart a node (`system reboot`)
  - reload wifi (`network.wireless reload`)
  - toggle a radio / set channel / txpower (`uci set wireless.radioN.*` → apply/rollback →
    confirm, with validated channel/txpower ranges)
  - the multi-node WiFi "apply plan" (`src/plan.rs`) — see below
- [x] Audit log of actions: in-memory (`GET /audit`) plus a JSON-lines mirror in
      `/tmp/openwrtfi-audit.log`, capped at 1000 lines, secrets always masked (`audit::
      mask_secret`/`mask_uci_line`).
- [x] The multi-node WiFi change ("apply plan"): `POST /wifi/networks/{id}/plan` computes a
      masked, ordered diff (mesh-only APs, then wired APs, then the gateway last);
      `POST /wifi/plans/{id}/apply` runs it in the background, one node at a time
      (`uci set` → `apply {rollback:true, timeout:30}` → re-reach the node → `confirm`, or leave
      it to rpcd's own revert); `GET /wifi/plans/{id}` polls status. The gateway nuance — the
      controller runs *on* the gateway, so its own `127.0.0.1` reachability proves nothing — is
      handled by requiring every AP to still answer before the gateway itself is confirmed
      (`plan::gateway_may_confirm`). Mock mode simulates realistic delays and a `fail-<node>`
      SSID value to exercise the rollback path in the UI.

#### Browser shell (opt-in, off by default)

A root terminal in the dashboard (the "Shell" button on each device card). It is root on every
node, so it ships **disabled** and only unlocks when every guard below is in place.

Checked on mainrouter (2026-09-24):
- `ttyd` and `luci-app-ttyd` are available in the 25.12 feed.
- uhttpd's TLS cert and key exist (`/etc/uhttpd.crt`, `/etc/uhttpd.key`).
- /overlay has 73 MB free.

Design:
- **Backend:** `ttyd` bound to **127.0.0.1 only**, running `login` (never a bare `ash`), so there
  is a second password prompt inside the terminal. OpenWrtFi proxies the WebSocket
  `/api/v1/shell/<node>` to it. It never listens on the LAN.
- **APs:** run `ttyd` on 127.0.0.1 on each AP too. The controller reaches it through an SSH
  tunnel, or through ttyd on the AP's LAN IP with firewall rules that only allow
  192.0.2.2. Pick one in Phase 3. Until then the shell is mainrouter-only, and you run
  `ssh ap-2` from inside it.
- **Size:** WebSocket proxying needs axum's `ws` feature (~100 KB). No PTY code in our binary;
  ttyd owns that.

Implemented (`src/shell.rs`): `POST /shell/token {password}` is the step-up re-auth (re-validates
the password against the gateway's rpcd, or `root`/`mock` in mock mode) and mints a one-time,
60-second token bound to the session. `GET /shell/{node}` upgrades to a WebSocket that this
binary itself proxies to `ttyd`'s own WebSocket at `127.0.0.1:7681/ws` (as a client, via
`tokio-tungstenite` — already pulled in transitively by axum's `ws` feature, so no extra crate).

Prerequisites (hard gates; the endpoint returns 403 unless all are true):
1. `"shell": {"enabled": true}` in `/etc/openwrtfi.json` (default `false`). There is no toggle in
   the UI, so a hijacked browser session can't turn it on.
2. Phase 3 login is in place, and the request carries a valid session cookie.
3. **HTTPS or loopback.** This binary has **no TLS** (`rustls` was never added — the read side's
   size budget was already tight, and the contract's own wording allows "HTTPS *or* localhost").
   In practice this means the shell only ever opens from a loopback connection, i.e. through an
   SSH tunnel (`ssh -L 8088:127.0.0.1:8088 mainrouter`), exactly like the alternative plan.md
   originally sketched — never over plain LAN HTTP. Revisit if the 2 MB budget grows enough to
   afford `rustls` (+0.6–0.9 MB) for real browser-facing HTTPS.
4. **Step-up auth.** Opening a shell re-asks for the root password, even when you're already
   logged in. That issues a one-time, 60-second shell token bound to the session.
5. **Origin check** on the WebSocket upgrade (`Origin` must match `Host`), plus `SameSite=Strict`
   HttpOnly session cookies.
6. Gateway only for now — an AP node gets a clear `501`, per the original "Until then the shell
   is mainrouter-only" note below.

Runtime limits (enforced server-side in `shell.rs`, not by ttyd):
- 10-minute idle timeout, 60-minute hard cap, whichever comes first.
- Audit log records shell open/close (user, node, duration). Keystrokes are **not** logged,
  because that would capture passwords.
- Mock mode: if `shell.enabled` is also true, `GET /shell/{node}` proxies to a fake echo backend
  instead of ttyd, so the UI's terminal panel can be exercised without a real router.

Still open (unverified without the real routers / not yet built):
- **APs:** run `ttyd` on 127.0.0.1 on each AP too, and reach it through an SSH tunnel or a
  firewalled LAN listener — not implemented; `GET /shell/{node}` for an AP returns `501`.
- The UI's xterm.js panel itself (Phase 2/3 frontend work, out of scope for this pass).
- One-shell-session-per-user and "closed when the dashboard session is logged out" aren't
  separately enforced (a session's tokens simply stop being issuable once the session itself
  expires or is logged out — good enough for now, but not the same as actively tearing down an
  *open* WebSocket on logout).

Out of scope:
- Shell access from outside the LAN. The WAN firewall stays closed, and double NAT via the Jio
  ONT already blocks it.

### LuCI integration (spans phases 1–4)

Principle: **don't reimplement LuCI, drive its backend**. Every LuCI page is JavaScript that calls
rpcd/ubus. OpenWrtFi makes the same calls, so:

- **No privilege escalation.** Write calls run under the *logged-in user's* rpcd session (from
  `session login`), not a service account. The existing LuCI ACLs in `/usr/share/rpcd/acl.d/`
  decide what is allowed.
- **LuCI's safety net comes for free.** Config changes go `uci set` → `uci apply {rollback:true,
  timeout:30}` → the browser calls `uci confirm` once it can still reach the router. If a change
  cuts connectivity (bad WiFi key, wrong LAN IP), rpcd reverts it after 30 s.
- **Deep links as a fallback.** Anything we don't cover links to the LuCI page on the right node
  (`http://<node>/cgi-bin/luci/admin/...`).

Verified on mainrouter (2026-09-24): the ubus objects `uci` (incl. `apply`/`confirm`/`rollback`),
`luci-rpc` (`getDHCPLeases`, `getHostHints`, `getWirelessDevices`, `getNetworkDevices`), `file`,
`rc`, `service`, `iwinfo`, `hostapd.*`, `usteer`, `network.wireless`, and `luci.temp-status`.
`nlbwmon` is running and its per-MAC counters work (`nlbw -c json -g mac`). `dawn` is installed
but disabled, so it doesn't conflict with usteer.

nlbwmon notes (from building the collector): `-c json` output is columnar
(`{"columns": [...], "data": [[...], ...]}`) and the column order is different for every `-g`
grouping, so the parser indexes by name rather than position (`nlbw.rs::parse_columnar`).
`layer7` is frequently `null` (unclassified flows, or protocols like plain IP/ICMP) — those fall
back to a `PROTO/port` label. nlbwmon only refreshes its own counters every ~30s, so the
collector is only run every 15th poll (at the default 2s `poll_secs`); rates are the delta
between consecutive *nlbw* samples, not consecutive polls, and a decreasing counter (nlbwmon
restart or a new monthly period) yields a 0 rate for one sample instead of going negative. The
fixture MACs in `fixtures/nlbw/` don't match the ones in `fixtures/dhcp.leases` (they were
captured from the live network at different times, and several look like randomized private
MACs per the caveat below) — a few `dhcp.leases` entries were added purely so the mock UI has
some named "top talkers" to show; on the real router both come from the same live lease file so
this doesn't come up.

| Tier | Feature | ubus / backend | Replaces LuCI page |
|---|---|---|---|
| **A: read-only** (Phase 1–2) | **Done.** Per-client totals, top apps, protocol mix | `nlbw -c json -g mac` + `-g mac,layer7` (shell, not ubus — `src/collectors/nlbw.rs`), surfaced as `Snapshot.usage` and a "Top talkers" / "Application mix" pair of dashboard panels | nlbwmon |
| | Host names from every source | `luci-rpc getHostHints`, `getDHCPLeases` | Status → Overview |
| | System log, live tail | `file exec logread` (later `log` object) | Status → System log |
| | Temperatures, internet detector | `luci.temp-status`, `luci.internet-detector` | those apps |
| | Diagnostics: ping, traceroute, nslookup | `file exec` (ACL already in luci-mod-network) | Network → Diagnostics |
| | Site survey / channel scan | `iwinfo scan` per radio | Wireless → Scan |
| **B: one-click actions** (Phase 3) | Kick or ban a client | `hostapd.<if> del_client {addr, ban_time}` | none (LuCI can't) |
| | Reserve IP from a client row | `uci add dhcp host` → apply | DHCP → Static leases |
| | Restart WiFi, restart a service, reboot a node | `network.wireless` down/up, `rc init`, `system reboot` | System → Startup / Reboot |
| | Radio on/off, channel, txpower | `uci set wireless.radioN.*` → apply | Wireless |
| | Whitelist toggle, test modes | `file exec qos-tiers ...` (custom ACL) | qosusage |
| **C: config forms** (Phase 3–4) | **One WiFi network, all APs.** Edit SSID, key, or 802.11r once and push it to every node | `uci` on each node, APs first, then the gateway, each with rollback | Wireless ×3 (the UniFi feature) |
| | Port forwards (with a double-NAT reminder for the Jio ONT) | `uci firewall redirect` → apply | Firewall → Port forwards |
| | DHCP static leases list | `uci dhcp host` | DHCP |
| | usteer policy (roam/kick thresholds) | `uci usteer` on all nodes | none |
| **D: system** (Phase 4) | Firmware upgrade per node | `attendedsysupgrade` (luci-app-attendedsysupgrade) | Attended sysupgrade |
| | Config backup / restore, all nodes at once | `file exec sysupgrade -b` | Backup / Flash |
| | Packages | link out to LuCI package manager | kept in LuCI |

Order: A first (safe, and makes the dashboard richer), then B, then the multi-node WiFi form from C.
D mostly stays as deep links, because upgrades and package management are rare and risky.

Guard rails for every write:
- Confirm dialog showing the exact diff (`uci changes`).
- Audit log entry.
- Never write to more than one node at once without rollback armed on each.
- Refuse config writes while a node is offline.

### Phase 4: history and polish
- [x] Persist the 24 h ring buffer to `/tmp` so it survives a service restart. Optional 30-day
      rollup file, written at most hourly to spare flash.
      Persists both the 24h (30-min bucket) and the new 7d (3h bucket, `GET /history?range=7d`)
      rings to `history_file` (`/tmp/openwrtfi-history.json` by default) every 150 polls (~5 min
      at the default 2s `poll_secs`), and loads them at startup (`collectors::traffic::Tracker::
      load`/`save`). No separate 30-day rollup file — the 7d/3h ring covers the same "don't lose
      everything on restart" need at the range the contract asks for; add one later if 30-day
      granularity turns out to matter.
- [x] Per-client throughput: sample nlbwmon counters (already running) and fall back to the qos-tiers accounting.
- [x] Events feed: roams (usteer), new device joined, node offline, WAN down. Browser notifications.
      `src/collectors/events.rs` diffs consecutive snapshots for joined/left/roam/band_steer/
      node_down/node_up/wan_down/wan_up, keeping the last 50 in memory (newest first). It does
      *not* poll usteer directly (see the module doc comment for why the plain diff was enough);
      "action" events and actual browser notifications are for later phases (actions + UI).
- [ ] Replace the `ubus` CLI fork per call with a native ubusd socket client (blobmsg).
- [ ] OpenWrt package (`packaging/openwrt/Makefile` for the SDK feed) as an `.apk` for 25.12.

## Resource budget (enforced)

| Metric | Budget | Check |
|---|---|---|
| Binary (with UI) | ≤ 2 MB | `ls -la target/aarch64-unknown-linux-musl/release/openwrtfi` |
| RSS | ≤ 8 MB | `grep VmRSS /proc/$(pidof openwrtfi)/status` |
| CPU at 2 s poll | ≤ 1% avg | `top -b -n 5` |
| UI JS (gzip) | ≤ 60 KB (the lazy xterm.js shell chunk is excluded) | `npm run build` output |
| procd memory limit | 64 MB hard cap | `openwrtfi.init` |

If the router gets tight on memory, there are two levers: `poll_secs` goes to 5, and the APs
are polled every other tick.

## Risks and open questions

- **rpcd ACL on the APs**: LuCI's own ACLs are broad. Use a dedicated user limited to
  `rpcd-acl-openwrtfi.json`, and never store the root password in `openwrtfi.json`.
- **Double NAT**: the dashboard should show it (Jio ONT at 192.168.225.1). It can't see
  the ONT's own stats.
- **Randomised MACs**: Macs rotate private MACs, so client identity should prefer the DHCP
  hostname and reservation, with MAC as a fallback. This matches the caveat in AGENTS.md.
- **Port**: 8088 avoids LuCI (80/443) and AdGuard (3000). A later option is to proxy it
  under uhttpd at `/openwrtfi`.

## Dev loop

```sh
cargo run -- --mock fixtures --listen 127.0.0.1:8088   # API with canned data
cd web && npm run dev                                   # UI on :5173, proxies /api
python3 design/gen_design.py                            # regenerate the .pen design
scripts/deploy.sh mainrouter                            # build and push to the router
```
