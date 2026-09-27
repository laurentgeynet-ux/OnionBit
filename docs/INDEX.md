# Index du dépôt

Inventaire des fichiers/dossiers significatifs. À maintenir à jour à
chaque étape (cf. `AGENTS.md`, "Workflow par étape").

| Chemin | Contenu |
| :--- | :--- |
| `AGENTS.md` | Règles pour agents IA (règles critiques, conventions, workflow) |
| `LICENSE` | Texte complet GPL-3.0 |
| `Cargo.toml` | Workspace Cargo, 12 crates, dépendances partagées |
| `crates/tribler-format/` | Bencode, `.torrent`, magnet, `.mdblob` |
| `crates/tribler-crypto/` | Hachage, clés IPv8, crypto tunnel |
| `crates/tribler-bittorrent/` | Intégration `librqbit`, sessions de téléchargement |
| `crates/tribler-ipv8/` | Moteur overlay IPv8 (discovery, communities, DHT overlay) |
| `crates/tribler-tunnel/` | `TunnelCommunity` : circuits, hidden seeding |
| `crates/tribler-core/` | Domaine/orchestration : `Session`, `Notifier`, règles métier |
| `crates/tribler-db/` | Persistance SQLite |
| `crates/tribler-network-policy/` | Garde-fous réseau (anti-SSRF, exit policy, kill switch) |
| `crates/tribler-api/` | API REST + WebSocket (axum) |
| `crates/tribler-cli/` | CLI de pilotage |
| `crates/tribler-daemon/` | Binaire principal (composition racine) |
| `crates/tribler-test-support/` | Fixtures/helpers de tests partagés |
| `docs/plans/plan_faisabilite.md` | Analyse de faisabilité, risques, décisions |
| `docs/plans/roadmap.md` | Plan d'implémentation détaillé (source de vérité de l'avancement) |
| `docs/architecture/architecture.md` | Vue d'ensemble de la clean architecture |
| `docs/architecture/decisions/000X-*.md` | ADRs |
| `docs/reference_tribler/correspondance_modules.md` | Correspondance Python Tribler ↔ Rust |
| `docs/CHANGELOG.md` | Historique des étapes franchies |
| `scripts/verify_all.ps1` | Validation complète (check/clippy/fmt/test) |

## Sources externes de référence (locales)

| Chemin | Rôle |
| :--- | :--- |
| `D:\Projet\Tribler_sources\tribler` | Sources officielles Tribler (+ sous-module `pyipv8/`) — référence de vérité protocolaire IPv8 / formats / API REST |
| `D:\Projet\Rqbit` | Sources de rqbit (branche main) — référence de l'API interne `librqbit` (`Session`, `ManagedTorrent`, options, stats) ; plus récent que `librqbit 9.0.1` sur crates.io |
| `C:\Emule-Sion-UI-UX\app` | UI Flutter de référence (style) — **à ne pas modifier**, consultée uniquement pour la phase 6 |
