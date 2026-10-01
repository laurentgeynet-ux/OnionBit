# AGENTS.md — OnionBit

Portage Rust natif du daemon [Tribler](https://github.com/Tribler/tribler)
(BitTorrent anonyme : overlay IPv8 + circuits onion) avec app Flutter
multiplateforme (`app/`). Licence : **GPL-3.0-or-later**.

## Structure

- `crates/onionbit-*` — 12 crates : `format`, `crypto`, `bittorrent`
  (enveloppe librqbit vendored sous `vendor/`), `ipv8`, `tunnel`, `core`,
  `db`, `network-policy`, `api` (REST+SSE axum), `cli`, `daemon`,
  `test-support`.
- `app/` — UI Flutter ; consomme uniquement l'API, jamais le core.
- `docs/plans/roadmap.md` — source de vérité de l'avancement ;
  `docs/CHANGELOG.md` — historique ; `docs/architecture/decisions/` — ADRs.
- `scripts/verify_all.ps1` — validation complète.

## Validation

```powershell
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo test -p <crate> --all-features   # ou --workspace
```

## Règles

- Code et docs en **français** ; `thiserror` ; pas de `unwrap()` hors
  tests ; `tracing` ; Tokio ; zéro warning.
- **Aucune valeur en dur** : seuils/timeouts → structs de config.
- **Fidélité protocole** : tout comportement filaire IPv8/BitTorrent/REST
  est vérifié contre les sources Tribler (checkout local via
  `TRIBLER_SRC` / `TRIBLER_PYIPV8`, binaire via `TRIBLER_EXE`). Écarts →
  ADR ou `docs/reference_tribler/`.
- Jamais de copie verbatim du Python Tribler ; dépendances compatibles
  GPL-3.0 uniquement.
- Ne jamais affaiblir `onionbit-network-policy` (anti-SSRF, isolation
  loopback, kill switch, politique de sortie).
- Une fonctionnalité = un propriétaire (pas de doublons entre crates).
- Workflow d'étape : implémenter → tester → cocher la roadmap +
  CHANGELOG (+ ADR si architecture) → commit dédié.
