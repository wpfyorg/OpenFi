# Contributing to OpenFi

Thanks for helping improve OpenFi. Focus changes on one problem, keep pull requests reviewable, and describe any behavior change clearly.

## Before you start

- Check [open issues](https://github.com/wpfyorg/OpenFi/issues) and existing pull requests for related work.
- For larger changes, open an issue first to agree on scope.
- For security problems, follow [SECURITY.md](SECURITY.md); do not file a public issue.

## Development setup

You need Rust stable with edition 2024 support, Node.js 22.12+ with npm, and Git. From the repository root:

```sh
npm ci --prefix web
npm run build --prefix web
cargo fmt --check
cargo test --locked
cargo run --locked -- --mock fixtures --listen 127.0.0.1:8088
```

Open <http://127.0.0.1:8088>. Mock mode uses the sanitized sample data under [`fixtures/`](fixtures/README.md) and does not need a router. The frontend build is required to embed the UI in the Rust application. The deployed cross-compile command and router deployment script are environment-specific; see [`plan.md`](plan.md) and [deployment guidance](docs/deployment.md) before using them.

## Changes and pull requests

- Keep each pull request focused. Explain the problem, approach, user-visible effects, and any compatibility or security implications.
- Add or update focused tests for changed behavior. Run `cargo test` and `npm run build --prefix web` when relevant, and report the actual results. Include manual verification steps for UI or router behavior that automated checks do not cover.
- Do not include credentials, tokens, real device identifiers, private network details, or unredacted router captures. Use the sanitized fixtures or redact reports before attaching them.
- Link related issues and update [`docs/api.md`](docs/api.md) when changing the API contract. Update [`plan.md`](plan.md) when project status or roadmap changes.
- For LuCI parity work, use [the feature matrix](docs/luci-parity.md). Name the baseline workflow, update its status only with evidence, and cover permissions, validation, apply/rollback, errors and recovery.
- For UI changes, compare the implementation with the intended design and describe any remaining differences. Mark design parity complete only when all relevant screens and states meet the design, responsive behavior, and accessibility expectations; do not describe partial progress as completed parity.
- Keep unrelated formatting and generated files out of the pull request.

Maintainers review submissions for correctness, security, scope, and fit with project goals. A pull request may need revisions before it can be merged.
