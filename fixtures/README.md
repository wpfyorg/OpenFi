# Fixtures

`--mock fixtures` loads these snapshots through `Config::use_fixtures`. Per-node ubus calls read files under `mainrouter/`, `ap-2/`, and `ap-4/`; top-level files model local gateway commands.

All fixture identity and telemetry values are synthetic: MAC addresses, IP addresses, SSIDs, credentials, hostnames, client labels, and usage-history totals were replaced. The JSON field structure and repeated-ID relationships remain suitable for mock UI and collector development. Wi-Fi keys are redacted.

`fixtures/usage-history/*.csv` preserves the source column schema but contains generated sample totals. `fixtures/nlbw/README.md` documents the nlbw mock data. Run the app with `cargo run -- --mock fixtures --listen 127.0.0.1:8088`.
