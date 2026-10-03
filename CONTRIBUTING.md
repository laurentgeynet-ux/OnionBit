# Contributing to OnionBit

Thanks for your interest! OnionBit is a Rust port of Tribler — contributions of all
kinds are welcome: code, protocol review, testing against live Tribler nodes, docs,
UI work, bug reports.

## Ground rules

- **Rust**: stable toolchain, `cargo fmt`, `cargo clippy -D warnings`, `thiserror`
  for typed errors, no `unwrap()` outside tests, `tracing` for logging.
- **Protocol fidelity**: anything touching IPv8 wire format, `.torrent`/DHT semantics
  or the REST API must be checked against the Tribler/pyipv8 sources — deviations
  need to be documented (ADR or mapping doc).
- **Security**: never weaken the network-policy guards (anti-SSRF, loopback isolation,
  kill switch, exit policy). If a change touches anonymity or leak protection, say so
  explicitly in the PR.
- **Licensing**: new dependencies must be GPL-3.0-compatible (MIT/Apache-2.0/BSD fine;
  flag any copyleft). Do not copy Python source verbatim — port the behavior.
- **No hardcoded values**: thresholds, timeouts, quotas go into documented config
  structs, not literals.

## Workflow

1. Open an issue (or comment on an existing one) before large changes.
2. Branch from `master`, keep commits focused, write/extend tests.
3. Before submitting: `cargo check --workspace`, `cargo clippy -D warnings`,
   `cargo fmt --check`, `cargo test -p <crate>` (workspace-wide for API changes).
4. PR template must be filled — describe protocol impact and security impact.

## Areas where help matters most

- **IPv8/Tunnel port conformance** — wire-level checks against pyipv8
- **Interop testing** — running OnionBit against Tribler 8.x nodes
- **Flutter UI** — screens, state management, platform packaging
- **Documentation** — module mapping tables, ADRs, user docs

## Commit style

Short imperative subject describing *why*, not just *what*. Reference issues and
roadmap steps when relevant.
