# AGENTS.md — OnionBit

Portage Rust natif du daemon [Tribler](https://github.com/Tribler/tribler)
(BitTorrent anonyme : overlay IPv8 + circuits onion) avec app Flutter
multiplateforme (`app/`). Licence : **GPL-3.0-or-later**.

## Structure

- `crates/onionbit-*` — 13 crates : `format`, `crypto`, `bittorrent`
  (enveloppe librqbit vendored sous `vendor/`), `ipv8`, `tunnel`,
  `messaging`, `core`, `db`, `network-policy`, `api` (REST+SSE axum),
  `cli`, `daemon`, `test-support`.
- `app/` — UI Flutter ; consomme uniquement l'API, jamais le core.
- `docs/plans/roadmap.md` — source de vérité de l'avancement ;
  `docs/CHANGELOG.md` — historique ; `docs/architecture/decisions/` — ADRs.
- `scripts/verify_all.ps1` — validation complète ;
  `scripts/check_i18n.ps1` — lint i18n (à lancer si `app/` touché).

## Validation

```powershell
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo test -p <crate> --all-features   # ou --workspace

# Si app/ touché :
cd app; flutter analyze; flutter test
pwsh -File scripts/check_i18n.ps1
```

## Règles

- Code et docs en **français** ; `thiserror` ; pas de `unwrap()` hors
  tests ; `tracing` ; Tokio ; zéro warning.
- Mutex d'état : préférer `.lock().unwrap_or_else(|e| e.into_inner())`
  (poison-tolérant — une panique sous verrou ne doit pas cascader).
- **En-tête GPL sur chaque fichier source** (RS/Dart/PS1/Py/CMake) :
  `This file is part of OnionBit...` + `Copyright (C) 2026 Laurent Geynet`
  + `SPDX-License-Identifier: GPL-3.0-or-later`. `vendor/` exclu
  (librqbit reste Apache-2.0).
- **Aucune valeur en dur** : seuils/timeouts → structs de config.
- **Fidélité protocole** : tout comportement filaire IPv8/BitTorrent/REST
  est vérifié contre les sources Tribler (checkout local via
  `TRIBLER_SRC` / `TRIBLER_PYIPV8`, binaire via `TRIBLER_EXE`). Écarts →
  ADR ou `docs/reference_tribler/`.
- Jamais de copie verbatim du Python Tribler ; dépendances compatibles
  GPL-3.0 uniquement.
- Ne jamais affaiblir `onionbit-network-policy` (anti-SSRF, isolation
  loopback, kill switch, politique de sortie).
- **Secrets** : jamais de clé privée, mot de passe ou blob `OBID`/`OBV1`
  dans les logs, les events SSE ou les réponses non protégées ;
  endpoints sensibles derrière `api_key_auth`.
- **Blobs portables** (`OBID`, `OBV1`, `OBF`) : magic 4 o + version +
  sel/nonce aléatoires + AEAD + borne de taille à l'ouverture — ce
  format est la convention pour tout artefact qui voyage entre devices.
- **Édition de fichiers** : ne PAS réécrire de fichiers via PowerShell
  (`-replace`, `Set-Content`) — corruption UTF-8 constatée (mojibake
  dans les accents) ; utiliser les outils d'édition dédiés.
- **Scripts `.ps1`** : UTF-8 **avec BOM** + prologue d'encodage
  (PS 5.1 reste utilisable) ; invoquer de préférence `pwsh` (7+,
  UTF-8 natif), jamais `powershell` dans les appels imbriqués.
- Une fonctionnalité = un propriétaire (pas de doublons entre crates).
- Workflow d'étape : implémenter → tester → cocher la roadmap +
  CHANGELOG (+ ADR si architecture) → commit dédié.
