# OpenWrtFi

A UniFi-style dashboard for OpenWrt. It is one static Rust binary (about 1 MB, with the UI built in) that runs on the router.

- Plan and roadmap: [plan.md](plan.md)
- Design: `design/openwrtfi.pen` (open it in Pen; regenerate with `python3 design/gen_design.py`)

```sh
(cd web && npm install && npm run build)
cargo run -- --mock fixtures --listen 127.0.0.1:8088   # open http://127.0.0.1:8088
scripts/deploy.sh mainrouter
```
