# LuCI parity roadmap

OpenFi's product goal is **full parity with LuCI core administration**, implemented in OpenFi's
own interface and APIs. Optional LuCI applications and protocol extensions are a separate scope:
they are tracked as optional application or extension work and are not required to declare core
parity. This is a roadmap, not a claim that OpenFi currently replaces LuCI.

LuCI is the OpenWrt configuration interface and feed. Its upstream repository separates core
modules, applications, and protocol packages; this roadmap uses that division to define a bounded
core target. See the [upstream LuCI repository](https://github.com/openwrt/luci), its
[`modules`](https://github.com/openwrt/luci/tree/master/modules),
[`applications`](https://github.com/openwrt/luci/tree/master/applications), and
[`protocols`](https://github.com/openwrt/luci/tree/master/protocols) directories. Relevant core
reference areas include [luci-mod-network](https://github.com/openwrt/luci/tree/master/modules/luci-mod-network),
[luci-mod-system](https://github.com/openwrt/luci/tree/master/modules/luci-mod-system),
[luci-mod-status](https://github.com/openwrt/luci/tree/master/modules/luci-mod-status),
[luci-mod-admin-full](https://github.com/openwrt/luci/tree/master/modules/luci-mod-admin-full),
and the [luci-base](https://github.com/openwrt/luci/tree/master/modules/luci-base) framework.
Core protocol workflows are in scope, including static addressing, DHCP client/server, PPPoE,
and IPv6. They are not excluded as optional protocol extensions. M0 must pin the reference OpenWrt
release and exact installed LuCI package set (including versions/revisions) so contributors can
resolve any scope ambiguity, such as which policy-routing UI is included. Optional LuCI
applications and non-baseline protocol packages remain separate.

## Status definitions

- **Implemented**: an OpenFi UI and backend path for the named workflow are present in this repo.
  This label does not imply full LuCI parity, broad firmware support, or live-device validation.
- **Partial**: meaningful parts exist, but important forms, actions, safety behavior, or supported
  cases are missing.
- **Planned**: no sufficient OpenFi workflow exists yet.

Status is based on source inspection of the current repository. Paths below are repo-relative.
No live router validation is claimed here.

## Feature inventory

| LuCI core area | Status | Evidence in OpenFi | Remaining parity work |
|---|---|---|---|
| Network interfaces, bridges, VLANs, devices and core protocols | **Partial** | `src/collectors/links.rs`, `src/collectors/radios.rs`, `src/collectors/uci.rs` collect selected live state; `src/api.rs` has no interface configuration routes. | Interface CRUD and common protocol workflows: static, DHCP client/server, PPPoE and IPv6; bridge/VLAN membership, validation, apply and recovery. |
| Static and policy routes, IPv4 and IPv6 | **Planned** | No route configuration page or route CRUD endpoint in `web/src/routes.tsx` or `src/api.rs`. | IPv4/IPv6 route tables, gateways, metrics and route rules. M0 must establish whether policy-routing UI is in the pinned core package baseline; ordinary static routes remain in scope. |
| DHCP and DNS configuration | **Partial** | `src/collectors/clients.rs` reads leases and reservations; `src/collectors/dns.rs` reads AdGuard statistics; `src/actions.rs` can create a reservation. No DHCP/DNS config editor route exists. | DHCP pools, static lease list/edit/delete, DHCPv6/RA, DNS forwarders and service-specific validation. |
| Firewall zones, rules, forwarding, NAT | **Planned** | No firewall route or configuration action in `src/api.rs`, `src/actions.rs`, or `web/src/routes.tsx`. | Zones, rules, forwarding/NAT, traffic rules, validation, diff review and lockout recovery. |
| Wireless radios and networks | **Partial** | `src/collectors/radios.rs`, `src/collectors/wifi_networks.rs`, `src/actions.rs`; UI in `web/src/pages/Wifi.tsx` and `web/src/pages/SettingsWifi.tsx`; coordinated apply in `src/plan.rs`. | Current editor handles selected SSID/security/broadcast/roaming fields; new networks, full radio/device settings, scan/survey, country/regulatory and release-specific modes remain. |
| System overview and node inventory | **Partial** | `web/src/pages/Dashboard.tsx`, `web/src/pages/Topology.tsx`, `web/src/pages/SettingsNodes.tsx`; node API in `src/nodes_api.rs`. | Full per-node system information and consistent capability/error reporting. |
| Users, groups, password management | **Partial** | `src/auth.rs`, `src/session.rs`, `web/src/pages/Login.tsx` authenticate an existing rpcd user. No user/group/password administration workflow exists. | User/group CRUD, password change/reset, privilege assignment and recovery. |
| Time, timezone, NTP | **Planned** | `src/collectors/date.rs` provides calendar conversion helpers for history/log grouping; `src/collectors/system.rs` collects system status, not clock settings. No time configuration route or page exists. | Clock, timezone, NTP servers and status, input validation and apply. |
| Package installation and removal | **Planned** | No package manager page or install/remove API; prior plan's package-manager-link-only decision is retired. | Search/index refresh, install/remove, dependency and storage visibility, progress/errors, and safe handling of package-manager locks. |
| Backup and restore | **Planned** | No backup/restore route in `src/api.rs` or page in `web/src/routes.tsx`. | Create/download, validate/upload, restore, per-node scope, secrets handling and recovery UX. |
| Firmware flash and upgrade | **Planned** | No flash/upgrade endpoint or page in `src/api.rs` or `web/src/routes.tsx`. | Image compatibility/checksum, storage and power checks, confirmation, upgrade status and recovery guidance. |
| Processes and services | **Partial** | `src/collectors/system.rs` exposes selected system metrics; `src/actions.rs` supports WiFi reload and reboot. No general process/service manager UI. | Process listing, service status/start/stop/restart and bounded, permission-checked actions. |
| Mount points and storage | **Planned** | No mount/storage page or corresponding API route found in `src/api.rs` or `web/src/routes.tsx`. | Mount table, filesystem capacity, mount/unmount and storage-device details. |
| Startup scripts and scheduled tasks | **Planned** | No startup/task editor route or page found in `src/api.rs` or `web/src/routes.tsx`. | Inspect and manage init scripts and scheduled tasks with syntax checks and safe execution semantics. |
| System logs | **Partial** | `src/collectors/logs.rs`, `src/api.rs` `/logs`, and `web/src/pages/Logs.tsx` provide log display. | Filtering, bounded live refresh, export, and parity for available log sources and levels. |
| Diagnostics | **Partial** | `src/diag.rs`, `src/api.rs` `/diag`, `web/src/components/DiagPanel.tsx` implement selected diagnostic requests. | Complete ping/traceroute/DNS diagnostic inputs, IPv4/IPv6 choice, progress, cancellation and output handling. |
| Multi-node operation | **Partial** | `src/nodes_api.rs`, `src/session.rs`, and `src/plan.rs` support configured nodes and a staged multi-node WiFi apply. | Per-node capability discovery, consistent configuration coverage, partial-failure recovery and fleet-wide backup/upgrade operations. |
| Authentication and ACL | **Partial** | `src/auth.rs`, `src/session.rs`, `src/api.rs` enforce sessions and mutation headers; writes use session-scoped ubus handles. | User/role administration, feature-level authorization, node-scoped ACL review, session lifecycle hardening and documented threat model. |
| Safe apply, confirm, rollback | **Partial** | `src/actions.rs` and `src/plan.rs` use UCI apply/confirm/rollback for selected WiFi/reservation changes; UI review/confirmation is present for WiFi in `web/src/pages/SettingsWifi.tsx`. | Shared transaction contract for every config form; exact diff, validation, reachability checks, timeout rollback and auditable recovery. |
| Localization | **Planned** | No translation catalog or locale selector found under `web/src`. | Extract user-facing strings, choose supported locales, format dates/numbers and test fallback behavior. |
| Accessibility | **Partial** | `web/src/components/` contains reusable buttons, dialogs, tables and toggles, but no documented or automated accessibility coverage was found. | Keyboard-only flows, focus management, names/roles/states, contrast, reduced motion, screen-reader feedback and repeatable checks across core workflows. |
| Extensions and protocol-specific integrations | **Planned; optional scope** | `src/ubus.rs` abstracts local/RPC/fixture transport; no general LuCI application or protocol extension framework exists. | Separately track extension discovery, capability contracts, authorization, UI integration and per-extension acceptance criteria. Not a blocker for core parity. |

## Priority milestones

Each milestone is intentionally contributor-sized. A milestone is accepted when its criteria and
tests are in the same change; implementation status advances only after source review. Device
validation, when needed, must name the tested device and firmware and must not be inferred from
mock or unit-test results.

### M0 — parity foundation

- Pin the reference OpenWrt release and exact LuCI core package set with versions/revisions; record
  separately which optional apps and protocol packages are excluded.
- Publish the capability matrix and distinguish unsupported capabilities from failed requests.
- Define reusable form, validation, diff-review, apply, rollback, and audit contracts.
- Contributor slices: inventory installed packages for one reference image; map each core menu item
  to a workflow; document the shared apply transaction; add server-side capability/error types.
- Acceptance: baseline is reproducible from a manifest; one documented API/UI pattern has
  authorization and error behavior; backend object availability alone never counts as parity.

### M1 — network foundations

- Contributor slices: interface status table; edit one static IPv4 interface; add DHCP client;
  add PPPoE credentials form; add IPv6 static addressing; list static routes; add one route; edit
  DHCP pool bounds; add/edit/delete one static lease.
- Add IPv4/IPv6 route read views and CRUD in small independent slices. Add DNS settings separately
  from lease management.
- Acceptance: validation rejects malformed addresses and incompatible combinations; apply uses the
  common safe-change flow; tests cover IPv4, IPv6, empty state and apply failure.

### M2 — firewall

- Contributor slices: read-only zone table; create/edit one zone; add forwarding; add one rule;
  add DNAT/port-forward CRUD; add source NAT only if present in the pinned package baseline.
- Acceptance: display the full effective diff; validate references and rule fields; arm rollback
  before applying connectivity-sensitive changes; verify recovery path in tests.

### M3 — wireless completion

- Contributor slices: create one AP network; delete one unlocked network; expose radio country;
  edit channel/width; add survey results for one supported driver.
- Acceptance: unsupported radio capabilities are explicit; multi-node partial failure reports each
  node and preserves rollback; credentials never appear in logs or diffs.

### M4 — system administration

- Contributor slices: list/create one local user; change one user's password; edit timezone;
  configure one NTP server; list/restart one service; list processes; show mount capacity; edit
  one startup entry. Keep each workflow and API change independently reviewable.
- Acceptance: authorization is enforced server-side; inputs are validated; dangerous actions need
  explicit confirmation and produce sanitized audit records.

### M5 — packages, backup and flash

- Contributor slices: refresh package index and show progress; install one package; remove one
  package; download one config backup; validate one restore upload; add image checksum display.
  Firmware flash/upgrade follows only after its compatibility and recovery contract is accepted.
- Acceptance: progress/error states, storage or compatibility checks where applicable, integrity
  validation, cancellation/recovery behavior, and tests for failure paths. Do not claim successful
  upgrade safety from mocked tests alone.

### M6 — release-quality core coverage

- Contributor slices: add one log filter; add IPv6 choice to one diagnostic; add one missing node
  capability status; extract strings for one page; fix keyboard focus in one dialog; add one
  accessible name/state regression check.
- Acceptance: a release-by-release checklist maps each core LuCI workflow to an OpenFi screen/API,
  test evidence, and any explicit capability limitation. Core parity is complete only when every
  applicable core workflow is implemented or a documented platform limitation is accepted.

Optional applications and protocols are selected after M0, scoped independently, and never
silently folded into the core parity claim.
