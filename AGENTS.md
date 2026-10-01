# AGENTS.md — OnionBit

Rust-native port of the [Tribler](https://github.com/Tribler/tribler) daemon
(anonymous BitTorrent: IPv8 overlay + onion circuits) with a Flutter app.
License: **GPL-3.0-or-later**.

## Layout

- `crates/onionbit-*` — workspace crates: `format`, `crypto`, `bittorrent`
  (wrapper over vendored librqbit), `ipv8`, `tunnel`, `core`, `db`,
  `network-policy`, `api` (REST+SSE, axum), `cli`, `daemon`, `test-support`.
- `app/` — Flutter UI; talks to the REST API only, never the core directly.
- `docs/plans/roadmap.md` — progress source of truth; `docs/CHANGELOG.md`.

## Validation

```sh
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo test -p <crate> --all-features
```

## Rules

- `thiserror` for errors, no `unwrap()` outside tests, `tracing` for logs,
  Tokio async, zero warnings.
- No hardcoded thresholds/timeouts — they belong in documented config structs.
- **Protocol fidelity**: IPv8/BitTorrent wire behavior and REST shapes are
  verified against the Tribler sources. Deviations go to an ADR.
- Never weaken `onionbit-network-policy` guards (anti-SSRF, loopback
  isolation, kill switch, exit policy).
- Dependencies must be GPL-3.0-compatible; never copy Tribler Python verbatim.
- One feature = one owner crate.
