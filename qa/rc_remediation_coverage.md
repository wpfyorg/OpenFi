# RC remediation coverage and boundaries

This file describes QA assets and records tests that cannot live outside the binary crate's
private module tree. It does not replace targeted unit tests owned by the remediation workers.

## Runnable black-box coverage

Build and run the mock-backed HTTP suite:

```sh
cargo build
python3 qa/rc_blackbox.py
```

The script starts the built binary with checked-in fixtures, an isolated config, audit path and
`TMPDIR`, so it does not touch shared mock history files. It exercises protected
routes, CSRF, logout, malformed extractor requests, concurrent audit writes, SSE, repeated
traffic-history restarts, and successful plus injected-failure WiFi rollouts. It never submits a
node add/remove request or talks to a router.

The baseline commit had three reproducible black-box defects. The current branch includes fixes
for audit serialization, traffic-history persistence, and API extractor error normalization. All
three checks are acceptance tests and are expected to pass against the current implementation:

- `test_audit_concurrent_successes_persist_to_disk`: audit API successes vs. JSONL records.
- `test_api_extractor_rejections_are_json_errors`: malformed JSON, wrong body type and missing
  JSON content type.
- `test_traffic_history_restart_has_no_duplicate_buckets`: repeated restarts within the active
  30-minute history bucket.

The auth, CSRF, SSE, and WiFi rollout cases are expected to pass before and after remediation.
The concurrent node config and daily usage restart fixes have owner-module unit tests in the
current tree; the external harness does not duplicate private implementation tests.

Generate synthetic >100-client, timezone, and directional-counter-reset inputs for the owning
Rust unit tests:

```sh
python3 qa/rc_accounting_vectors.py /tmp/openwrtfi-accounting-vectors.json
```

The vector file is intentionally generated on demand. It supplies 120 nlbw rows, exact full-list
and top-50 sums, UTC/+05:30/-08:00 local-midnight points, and RX-only/TX-only/both/neither reset
expectations. Because the production aggregation and date helpers are private to the binary
crate, the owner tests must consume these cases from inside their module tests; the Python vector
generator alone does not claim to test production behavior.

Optional real browser check (requires a locally available Playwright package and browser):

```sh
OPENWRTFI_BASE_URL=http://127.0.0.1:18088 node qa/rc_browser.mjs
```

Start the fixture-mode server separately on that address. The browser script checks deep-link
auth redirection, wrong and correct login, all application routes, reload, SSE connection, and
browser console/network errors. It performs no write actions.

## Tests that belong inside the owning Rust modules

The crate declares one binary target (`src/main.rs`); its modules are private, and the `AppState`
constructor plus collector helpers are not exported as a library. A new external Rust test under
`tests/` cannot name `audit::Audit`, `nodes_api`, `DailyTracker`, `nlbw::Tracker::apply`, or the
whitelist/client merge functions. Making those items public or adding a library target would be a
production refactor outside Luna-QA's write scope. Owners should add deterministic unit tests
inside their existing `#[cfg(test)]` modules:

| Coverage | Required adversarial assertion | Exact boundary |
| --- | --- | --- |
| Daily usage restart | Save/reload mid-day; continue counters; total is prior usage plus new delta exactly once | Owner-module test now covers save/load and restored WAN baseline; HTTP mock counters are not controllable through the shipped fixtures during a running process |
| 100+ client accounting | 120 generated client rows; total includes all rows while presentation stays capped at 50 | Current public snapshot exposes only truncated client rows and has no full aggregate contract; wait for the worker's aggregate shape |
| Local-day boundaries | Select correct date at UTC, +05:30 and -08:00 immediately before/at/after local midnight | `day_key` is private and API window derives from private `AppState` snapshot timestamps |
| One-sided counter resets | RX-only reset retains valid TX delta; TX-only reset retains RX; both reset follows explicit policy | Daily tracker delta state is private; WAN traffic `Tracker` also keeps its counter baseline private |
| Whitelist source precedence | Test file=true/QoS=false and file=false/QoS=true | Client merge function is private in collector module |
| QoS-only last_seen | Online fallback stamps the known current poll time | Merge pipeline is private; test must use a deterministic poll timestamp |
| Node add/add and add/remove | Both successful updates survive; config fields/permissions stay intact | Owner-module transaction tests cover concurrent add/add and add/remove using temporary config paths; mock mode rejects API writes and real API success schedules `/etc/init.d/openwrtfi restart` |

The node concurrency test now uses the safe transaction seam. Do not point it at the live node
API: successful requests schedule a system service restart.

## Hardware-only gate

No test in this directory establishes real OpenWrt behavior. `ubus`, `uci`, nftables, hostapd,
dnsmasq leases, SQM/CAKE qdiscs, rpcd ACLs, service restart, and reboot persistence require the
supported OpenWrt/rpcd image or a disposable router. The `--mock` server only verifies controller
state and simulated rollout outcomes.

## Expected command set after worker patches

```sh
cargo fmt --check
cargo clippy --all-targets --all-features
cargo test
npm --prefix web run build
cargo build
python3 qa/rc_blackbox.py
```

The Playwright script is an optional separately provisioned browser check. Real-router checks
remain a distinct sign-off gate.
