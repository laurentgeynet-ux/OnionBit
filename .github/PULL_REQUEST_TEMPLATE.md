## Summary

<!-- What does this change, and why? -->

## Checklist

- [ ] `cargo check --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] Tests added/updated (`cargo test -p <crate>`)
- [ ] Docs updated (roadmap step checked, CHANGELOG, ADR if architecture changed)
- [ ] No new dependency incompatible with GPL-3.0
- [ ] No hardcoded thresholds/timeouts — goes through a config struct

## Protocol impact

<!-- Does this touch IPv8 wire format, BitTorrent protocol, or REST API shape?
     If yes: which Tribler/pyipv8 behavior was used as reference? -->

## Security impact

<!-- Does this touch anonymity, tunnel crypto, kill switch, anti-SSRF,
     exit policy, or API exposure? -->
