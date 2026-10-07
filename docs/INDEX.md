# Index du dépôt

Inventaire des fichiers/dossiers significatifs. À maintenir à jour à
chaque étape (cf. `AGENTS.md`, "Workflow par étape").

> `docs/plans/` (roadmap, faisabilité, catalogue de bancs, plans
> d'enrichissement) est de la documentation interne de pilotage,
> versionnée dans ce dépôt ; les fichiers marqués *(local)* sont
> filtrés de la publication publique (`scripts/sync_public.ps1`,
> hors dépôt).

| Chemin | Contenu |
| :--- | :--- |
| `AGENTS.md` | Règles pour agents IA (règles critiques, conventions, workflow) |
| `LICENSE` | Texte complet GPL-3.0 |
| `Cargo.toml` | Workspace Cargo, 13 crates, dépendances partagées |
| `crates/onionbit-format/` | Bencode, `.torrent`, magnet, `.mdblob` |
| `crates/onionbit-crypto/` | Hachage, clés IPv8, crypto tunnel |
| `crates/onionbit-bittorrent/` | Intégration `librqbit`, sessions de téléchargement |
| `crates/onionbit-ipv8/` | Moteur overlay IPv8 (discovery, communities, DHT overlay) |
| `crates/onionbit-tunnel/` | `TunnelCommunity` : circuits, hidden seeding, `TunnelUdpSocket` (uTP/DHT/tracker via cellules `data`) |
| `crates/onionbit-messaging/` | Messagerie anonyme e2e (ADR-0011) : frames signées, consentement, coffre `OBV1`, gates ADR-0015 |
| `crates/onionbit-core/` | Domaine/orchestration : `Session`, `Notifier`, règles métier |
| `crates/onionbit-db/` | Persistance SQLite |
| `crates/onionbit-network-policy/` | Garde-fous réseau (anti-SSRF, exit policy, kill switch) |
| `crates/onionbit-api/` | API REST + SSE (axum) |
| `crates/onionbit-cli/` | CLI de pilotage |
| `crates/onionbit-daemon/` | Binaire principal (composition racine) |
| `crates/onionbit-test-support/` | Fixtures/helpers de tests partagés |
| `vendor/` | `librqbit*` vendored+patchés (`[patch.crates-io]`) : `DatagramSocket` injectable, uTP/DHT/tracker-UDP sur tunnel (ADR-0007) |
| `app/` | Interface Flutter desktop (Riverpod + go_router, consomme `onionbit-api` REST/SSE) |
| `assets/` | Logo, screenshots et social-preview référencés par le README public |
| `.github/workflows/ci.yml` | CI GitHub Actions ; `CODEOWNERS`, `dependabot.yml`, `ISSUE_TEMPLATE/` |
| `docs/plans/plan_faisabilite.md` *(local)* | Analyse de faisabilité, risques, décisions |
| `docs/plans/roadmap.md` *(local)* | Plan d'implémentation détaillé (source de vérité de l'avancement) |
| `docs/architecture/architecture.md` | Vue d'ensemble de la clean architecture |
| `docs/architecture/decisions/000X-*.md` | ADRs |
| `docs/reference_tribler/correspondance_modules.md` | Correspondance Python Tribler ↔ Rust |
| `docs/reference_tribler/api_rest_mapping.md` | Mapping endpoints/DTO/topics SSE API Python ↔ `onionbit-api` |
| `docs/reference_tribler/api_endpoints_complet.md` | Inventaire exhaustif des fonctions de l'API web Tribler (routes, paramètres, défauts/min-max, implantation Python ↔ Rust) |
| `docs/reference_tribler/configuration_cablage.md` | Câblage des champs `configuration.json` : mapping direct, décisions explicites et écarts assumés |
| `docs/reference_tribler/ecarts_fidelite.md` | Écarts de fidélité assumés vs Tribler (justifiés) |
| `docs/reference_tribler/ipv8_rust_tunnels/` | Extraits des sources `ipv8-rust-tunnels` (formats clés, DH, paquets) |
| `docs/security/revue_garde_fous.md` | Inventaire des protections réseau (anti-SSRF, exit policy, kill switch, proxy guard, hidden seeding) + tests qui les couvrent |
| `docs/security/threat_model.md` | Modèle de menace : ce que les bancs prouvent / ne prouvent pas (corrélation de trafic, Sybil, exits malveillants, endurance) |
| `docs/security/fingerprinting.md` | Empreinte protocole : ce qu'un observateur voit du trafic OnionBit (avant/après OBF) |
| `docs/security/fuzz_journal.md` | Journal des campagnes fuzz (corpus, crashes, minimas retenus en régression) |
| `docs/interop/README.md` | Preuves d'interop publiques (EN) — scénarios vérifiés Tribler 8.4.3, ce qui est exercé sur le fil ; importée de l'ancienne ligne de publication |
| `docs/interop/public_dht_runs.md` | Registre des runs du banc DHT publique (paramètres, route observée, octets vérifiés, échecs) |
| `docs/THREAT-MODEL.md` | Modèle de menace public (EN, court) — version détaillée FR : `docs/security/threat_model.md` |
| `docs/BUILDING.md` | Build & packaging publics (EN) : daemon/CLI/UI Flutter, zip Windows x64 |
| `docs/diagnostics/memoire_charge_reelle.md` | Diagnostic empreinte mémoire d'un daemon public en charge (postes, caps, réfs code) + garde-fou `scripts/mem_watchdog.ps1` |
| `docs/plans/mobile_execution_model.md` *(local)* | Étape 18 — modèle d'exécution Android/iOS (façade FFI, foreground service, contraintes) |
| `docs/plans/app_downloads_enrichissement.md` *(local)* | Plan d'enrichissement de l'onglet Téléchargements (UI Flutter, 8 étapes) |
| `docs/plans/app_search_enrichissement.md` *(local)* | Plan d'enrichissement de l'onglet Rechercher (UI Flutter, 8 étapes) |
| `docs/plans/app_settings_enrichissement.md` *(local)* | Plan d'enrichissement de l'onglet Réglages (UI Flutter, 7 étapes) |
| `docs/plans/app_sidebar_enrichissement.md` *(local)* | Plan d'enrichissement de la sidebar (UI Flutter, 6 étapes) |
| `docs/plans/app_enrichissement_global.md` *(local)* | Enrichissement global de l'app (revue transverse UI+API) |
| `docs/plans/flutter_architecture.md` *(local)* | Architecture de l'UI Flutter desktop (étape 20) |
| `docs/plans/web_ui_plan.md` *(local)* | Portage Flutter web servi same-origin (étapes 31-35) |
| `docs/plans/mobile_ffi_surface.md` *(local)* | Contrat FFI mobile figé avant build (étape 19) |
| `docs/plans/bench_adr0015.md` *(local)* | Journal de campagne `OnionbitExtCommunity` (hello lazy, attestations) |
| `docs/plans/bancs_tests.md` *(local)* | Catalogue des bancs de test (matrice release P0, oracles, artefacts, journal des runs) |
| `fuzz/` | Cibles cargo-fuzz (`ext_packet`, `ipv8_packet`, `messaging_*`) — corpus/artifacts non versionnés |
| `docs/P0-transport-manifest.md` | Manifeste de preuve figé — clôture P0 transport : scénarios 17a/17b/17c, commits de référence, limites assumées, bascule P1 |
| `docs/ruptures/README.md` | Philosophie fail-closed + recette d'un banc de rupture (injection, fenêtrage, attribution, oracle `INTERDIT=0`) |
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
