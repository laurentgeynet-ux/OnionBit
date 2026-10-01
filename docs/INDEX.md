# Index du dépôt

Inventaire des fichiers/dossiers significatifs. À maintenir à jour à
chaque étape (cf. `AGENTS.md`, "Workflow par étape").

| Chemin | Contenu |
| :--- | :--- |
| `AGENTS.md` | Règles pour agents IA (règles critiques, conventions, workflow) |
| `LICENSE` | Texte complet GPL-3.0 |
| `Cargo.toml` | Workspace Cargo, 12 crates, dépendances partagées |
| `crates/onionbit-format/` | Bencode, `.torrent`, magnet, `.mdblob` |
| `crates/onionbit-crypto/` | Hachage, clés IPv8, crypto tunnel |
| `crates/onionbit-bittorrent/` | Intégration `librqbit`, sessions de téléchargement |
| `crates/onionbit-ipv8/` | Moteur overlay IPv8 (discovery, communities, DHT overlay) |
| `crates/onionbit-tunnel/` | `TunnelCommunity` : circuits, hidden seeding, `TunnelUdpSocket` (uTP/DHT/tracker via cellules `data`) |
| `crates/onionbit-core/` | Domaine/orchestration : `Session`, `Notifier`, règles métier |
| `crates/onionbit-db/` | Persistance SQLite |
| `crates/onionbit-network-policy/` | Garde-fous réseau (anti-SSRF, exit policy, kill switch) |
| `crates/onionbit-api/` | API REST + SSE (axum) |
| `crates/onionbit-cli/` | CLI de pilotage |
| `crates/onionbit-daemon/` | Binaire principal (composition racine) |
| `crates/onionbit-test-support/` | Fixtures/helpers de tests partagés |
| `vendor/` | `librqbit*` vendored+patchés (`[patch.crates-io]`) : `DatagramSocket` injectable, uTP/DHT/tracker-UDP sur tunnel (ADR-0007) |
| `app/` | Interface Flutter desktop (Riverpod + go_router, consomme `onionbit-api` REST/SSE) |
| `docs/plans/plan_faisabilite.md` | Analyse de faisabilité, risques, décisions |
| `docs/plans/roadmap.md` | Plan d'implémentation détaillé (source de vérité de l'avancement) |
| `docs/architecture/architecture.md` | Vue d'ensemble de la clean architecture |
| `docs/architecture/decisions/000X-*.md` | ADRs |
| `docs/reference_tribler/correspondance_modules.md` | Correspondance Python Tribler ↔ Rust |
| `docs/reference_tribler/api_rest_mapping.md` | Mapping endpoints/DTO/topics SSE API Python ↔ `onionbit-api` |
| `docs/reference_tribler/api_endpoints_complet.md` | Inventaire exhaustif des fonctions de l'API web Tribler (routes, paramètres, défauts/min-max, implantation Python ↔ Rust) |
| `docs/reference_tribler/configuration_cablage.md` | Câblage des champs `configuration.json` : mapping direct, décisions explicites et écarts assumés |
| `docs/reference_tribler/ipv8_rust_tunnels/` | Extraits des sources `ipv8-rust-tunnels` (formats clés, DH, paquets) |
| `docs/security/revue_garde_fous.md` | Inventaire des protections réseau (anti-SSRF, exit policy, kill switch, proxy guard, hidden seeding) + tests qui les couvrent |
| `docs/interop/public_dht_runs.md` | Registre des runs du banc DHT publique (paramètres, route observée, octets vérifiés, échecs) |
| `docs/plans/mobile_execution_model.md` | Étape 18 — modèle d'exécution Android/iOS (façade FFI, foreground service, contraintes) |
| `docs/plans/app_downloads_enrichissement.md` | Plan d'enrichissement de l'onglet Téléchargements (UI Flutter, 8 étapes) |
| `docs/plans/app_search_enrichissement.md` | Plan d'enrichissement de l'onglet Rechercher (UI Flutter, 8 étapes) |
| `docs/plans/app_settings_enrichissement.md` | Plan d'enrichissement de l'onglet Réglages (UI Flutter, 7 étapes) |
| `docs/plans/app_sidebar_enrichissement.md` | Plan d'enrichissement de la sidebar (UI Flutter, 6 étapes) |
| `scripts/build_release.ps1` | Build release reproductible multi-cibles + `dist/<target>/` + manifest |
| `docs/CHANGELOG.md` | Historique des étapes franchies |
| `scripts/verify_all.ps1` | Validation complète (check/clippy/fmt/test) |
| `scripts/interop_ipv8.ps1` + `scripts/interop/` | Jalon d'interop Rust↔pyipv8 sur loopback : `py_node.py` (noeud Python), `verify_packets.py` (vérification Ed25519 via le vrai `default_eccrypto`), chemins réglables via `TRIBLER_PYIPV8`/`TRIBLER_INTEROP_PY` |
| `crates/onionbit-ipv8/tests/fixtures/` | Paquets filaires réels enregistrés pendant l'interop (`.hex`) + `README.md` de provenance (commit pyipv8, sens, msg_ids) ; rejoués par `tests/interop_replay.rs` |

## Sources externes de référence (locales)

| Chemin | Rôle |
| :--- | :--- |
| `<Tribler sources checkout> (env `TRIBLER_SRC`)` | Sources officielles Tribler (+ sous-module `pyipv8/`) — référence de vérité protocolaire IPv8 / formats / API REST |
| `<rqbit source checkout> (env `RQBIT_SRC`)` | Sources de rqbit (branche main) — référence de l'API interne `librqbit` (`Session`, `ManagedTorrent`, options, stats) ; plus récent que `librqbit 9.0.1` sur crates.io |
| `<Tribler install dir> (env `TRIBLER_EXE`)` | Tribler **8.4.3** installé (application figée CPython 3.12) — `Tribler.exe` = noeud Tribler réel pour les tests d'interop/ping-pong des étapes 9-12 ; `lib/` = dépendances figées (`ipv8` en `.pyc`, `ipv8_rust_tunnels.pyd`, `libtorrent`, `tribler`) importables dans un venv **CPython 3.12** via `PYTHONPATH` ; `tribler_source/` = sources `.py` de la version installée ; `tools/reset*.bat` = réinitialisation de l'état |
| `the reference Flutter UI project` | UI Flutter de référence (style) — **à ne pas modifier**, consultée uniquement pour la phase 6 |
