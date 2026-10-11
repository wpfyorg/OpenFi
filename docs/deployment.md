# Deployment and configuration

OpenFi is experimental. Use a test router first. Keep SSH/LuCI access and a configuration backup. Host tests and mock data do not establish real-device compatibility.

## Build and install

The current helper builds `aarch64-unknown-linux-musl`. Confirm the router architecture first. On the build host install Rust, Node.js 22.12+, npm, Zig and cargo-zigbuild. Install frontend dependencies with `npm ci --prefix web`.

Inspect `scripts/deploy.sh`, `packaging/openwrt/files/openwrtfi.init` and the sample JSON before using them. Configure an SSH alias for your test router, then run:

```sh
scripts/deploy.sh <router-ssh-alias>
```

The helper replaces `/usr/bin/openwrtfi` and `/etc/init.d/openwrtfi`, creates `/etc/openwrtfi.json` only if absent, then enables/restarts the service. It does not install required OpenWrt packages, configure rpcd accounts, or back up existing files. Save the prior binary, init script and config separately before installation so they can be restored and the service restarted.

The gateway requires ubus, rpcd, uhttpd with its ubus module, and the objects queried by the enabled collectors. The init script starts a private uhttpd bridge on `127.0.0.1:8089` and the controller on port 8088. Optional collectors need their corresponding services; unavailable objects should be tested on the target firmware. The shipped init script has a 64 MB address-space ceiling that may need validation on your hardware.

## Configure your own topology

`packaging/openwrt/files/openwrtfi.json` is an example, not discovery. Replace its node list, interface names, credentials and optional integrations. A local gateway uses transport `local`; remote nodes use `rpcd` and need a reachable ubus HTTP endpoint and appropriately scoped accounts. Review the example ACL files before installation; never widen permissions just to suppress an error.

The optional QoS helper and CSV format are project-specific integrations, not stock OpenWrt features. Disable integrations you do not run. Set `local_utc_offset_secs` for your usage-day boundary; the example offset is not automatically derived from your timezone.

Store configuration containing credentials with restrictive permissions (for example, mode 0600 owned by root). Do not add it to git or attach it to issues. The browser shell is disabled by default; see [API shell documentation](api.md) for its ttyd and loopback requirements.

## Access and verify

The binary has no built-in TLS. Use a trusted management network, an SSH tunnel, or an appropriately configured TLS reverse proxy. Restrict port 8088 to intended administrators; do not expose the service directly to the internet.

Check service logs, login, node availability and read views before trying writes. Real writes can disconnect clients or reboot routers. For configuration changes, verify confirmation, error handling, rollback and recovery on the actual device. Never infer successful application from a mock response.

## Upgrade and recovery

For each upgrade, retain the prior executable/config/service and read the diff. Restore those files and restart the prior service if necessary. There is no automatic binary update or rollback mechanism today. OpenWrt sysupgrade may remove the executable while preserving configuration: review your backup list and service configuration before upgrading firmware, and reinstall/disable OpenFi as appropriate afterward.
