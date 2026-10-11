# Public repository launch

These are repository-owner steps; files alone do not enable GitHub settings.

1. Merge the public-readiness commit into `main` after CI and review.
2. Confirm the tracked source/history and any attached artifacts contain only sanitized data. Do not upload original network captures or private design previews.
3. Change visibility when ready. Enable Issues and private vulnerability reporting in repository settings; verify the **Report a vulnerability** entry actually appears. Publish a private maintainer contact method for conduct reports and reporting fallback.
4. Set the description to “Open source OpenWrt dashboard and configuration interface; working toward LuCI feature parity.” Suggested topics: `openwrt`, `luci`, `rust`, `preact`, `network-management`.
5. Protect `main`: require pull requests and the `build-and-test` CI check; disable force pushes and branch deletion. Adapt review requirements to maintainer availability.
6. Add labels such as `bug`, `enhancement`, `question`, `documentation`, `good first issue`, and `help wanted`. Select small tasks from [the parity milestones](luci-parity.md), with clear acceptance criteria and a pinned upstream baseline.
7. Publish a release only with a tag, release notes, target/firmware validation, installation/recovery instructions and checksums for distributed binaries. Local host builds do not validate router releases.

Current release status: experimental 0.1.0 source. There is no supported hardware/firmware matrix or universal installation package yet. The contribution, support and security policies avoid guaranteed response times.
