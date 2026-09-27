# AGENTS.md — Règles pour agents IA

## Vue d'ensemble

**Tribler-Rust-Torrent** est un portage du daemon **Tribler** (client BitTorrent
anonymisé, Python/aiohttp, https://github.com/Tribler/tribler) vers un daemon
**Rust** natif, avec une future interface **Flutter** multiplateforme
(Windows x64/arm64, Linux, macOS, Android, iOS, Web) qui ne sera démarrée
qu'une fois le backend validé à 100 % sur ses fonctionnalités clés.

- Moteur : **Rust** (workspace Cargo, Tokio async)
- Moteur BitTorrent : réutilisation de **`librqbit`** (Apache-2.0) — bencode,
  peer-wire, DHT mainline BEP 5, uTP, trackers — plutôt qu'une réécriture
  (cf. ADR-0001)
- Réseau d'anonymisation : **portage du protocole IPv8** (`pyipv8` +
  `TunnelCommunity`) — aucune implémentation Rust complète n'existe à ce
  jour, c'est la brique la plus risquée du projet (cf. ADR-0002)
- Contrôle du daemon : API **REST + SSE** (axum), en parité
  fonctionnelle avec `tribler.core.restapi`, consommée par `tribler-cli`
  puis plus tard par l'UI Flutter — jamais d'accès direct à `tribler-core`
- Licence : **GPL-3.0-or-later** (héritée de Tribler, cf. ADR-0003 —
  le projet s'appuie sur l'architecture/la logique du code source GPL-3.0
  de Tribler, pas une réimplémentation clean-room)

Plateformes cibles finales : Windows 10/11 x64 et arm64, Linux, macOS,
Android, iOS, Web (le build Web du frontend Flutter se connecte à un
daemon distant/local via HTTP — un navigateur ne peut pas ouvrir de
sockets BitTorrent/UDP bruts, cf. `docs/plans/plan_faisabilite.md`).

Références de vérité en local :

- **Protocole / comportement Tribler** : `D:\Projet\Tribler_sources\tribler`
  (dont le sous-module `pyipv8/`). Tout portage de comportement filaire ou
  de protocole IPv8/BitTorrent doit être vérifié contre ces sources avant
  d'être considéré terminé.
- **API interne du moteur BitTorrent** : `D:\Projet\Rqbit` (sources
  complètes de rqbit, branche main — plus récent que la version
  `librqbit 9.0.1` packagée sur crates.io). À consulter pour connaître
  l'API exacte de `Session`/`ManagedTorrent`/stats/options plutôt que
  de deviner les signatures (docs.rs en secours : `librqbit 9.0.1`).

## Cartographie du dépôt

| Chemin | Rôle |
| :--- | :--- |
| `crates/tribler-format` | Bencode, `.torrent`, magnet URI, blobs `.mdblob` de canaux |
| `crates/tribler-crypto` | Hachage BitTorrent (SHA-1/SHA-256), clés IPv8 (Ed25519/X25519), crypto tunnel |
| `crates/tribler-bittorrent` | Enveloppe autour de `librqbit` : sessions de téléchargement/upload |
| `crates/tribler-ipv8` | Portage du moteur overlay IPv8 (discovery, communities, DHT overlay, signatures). **Composant à plus haut risque — cf. ADR-0002** |
| `crates/tribler-tunnel` | Portage de `TunnelCommunity` : circuits onion routing, hidden seeding |
| `crates/tribler-core` | Domaine/orchestration : `Session`, `Notifier`, règles métier (decouverte, RSS, watch folder, torrent checker) |
| `crates/tribler-db` | Persistance SQLite (torrents, canaux, votes, réglages) via `rusqlite` |
| `crates/tribler-network-policy` | Anti-SSRF, politique des noeuds de sortie, kill switch, garde-fous SOCKS5 |
| `crates/tribler-api` | Plan de contrôle REST + SSE (axum), seule porte d'entrée réseau locale |
| `crates/tribler-cli` | CLI de pilotage, parle uniquement à `tribler-api` |
| `crates/tribler-daemon` | Binaire principal : assemble tout, config, cycle de vie |
| `crates/tribler-test-support` | Fixtures/helpers de tests partagés inter-crates (dev-dependency uniquement) |
| `docs/plans/plan_faisabilite.md` | Analyse de faisabilité, risques, décisions arbitrées |
| `docs/plans/roadmap.md` | Plan d'implémentation détaillé étape par étape (source de vérité de l'avancement) |
| `docs/architecture/architecture.md` | Vue d'ensemble de la clean architecture, couches, flux de dépendances |
| `docs/architecture/decisions/` | ADRs (décisions d'architecture actées) |
| `docs/reference_tribler/` | Tables de correspondance module Python Tribler ↔ crate Rust |
| `docs/INDEX.md` | Inventaire des fichiers du dépôt |
| `docs/CHANGELOG.md` | Historique des étapes franchies |
| `scripts/verify_all.ps1` | Validation complète (check/clippy/fmt/test) |

## Commandes de validation

À exécuter avant de considérer une étape terminée :

```powershell
# Rust — lint (toujours, tout le workspace : détecte les cassures cross-crates)
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check

# Rust — tests : le(s) crate(s) modifié(s)
cargo test -p <crate> --all-features
# élargir au workspace si le changement touche plusieurs crates ou des API publiques
cargo test --workspace --all-features

# Validation complète
powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\verify_all.ps1
```

## Règles critiques

1. **Backend d'abord, à 100 %** : aucune ligne d'UI (Flutter) ne doit être
   écrite avant que les fonctionnalités du daemon prévues dans
   `docs/plans/roadmap.md` soient validées (tests + vérification manuelle).
   L'UI réutilisera les patterns de `C:\Emule-Sion-UI-UX\app` (référence de
   style), mais ce chemin ne doit pas être modifié par ce projet.
2. **Fidélité protocolaire** : la référence de vérité pour tout ce qui
   touche au protocole IPv8, au format `.torrent`/DHT BitTorrent ou à
   l'API REST est `D:\Projet\Tribler_sources\tribler` (et son sous-module
   `pyipv8/`). Documenter tout écart dans une ADR ou dans
   `docs/reference_tribler/`.
3. **Ne pas réinventer ce qui existe déjà en Rust mûr** : le moteur
   BitTorrent passe par `librqbit` (cf. ADR-0001) plutôt qu'une
   réimplémentation de bencode/peer-wire/DHT. En cas de doute sur l'API
   `librqbit`, consulter les sources locales `D:\Projet\Rqbit` (branche
   main) avant docs.rs. Vérifier `docs/plans/plan_faisabilite.md`
   avant d'ajouter une grosse dépendance protocolaire alternative.
4. **Sécurité non négociable** : ne jamais affaiblir les garde-fous de
   `tribler-network-policy` (anti-SSRF, isolation loopback par défaut de
   `tribler-api`, kill switch des tunnels, politique des noeuds de sortie).
5. **Licence GPL-3.0** : toute dépendance ajoutée doit être compatible
   GPL-3.0 (permissif MIT/Apache-2.0/BSD OK ; vérifier toute dépendance
   copyleft avant ajout). Ne jamais copier de code Python Tribler
   verbatim — porter la logique/le comportement, pas le texte source.
6. **Dépendances** : préférer les dépendances déclarées dans
   `[workspace.dependencies]` (`Cargo.toml` racine) ; vérifier que la
   version choisie n'est pas sortie il y a moins de 7 jours.
7. **Aucune valeur en dur** : réglages Rust (seuil, timeout, quota, taille,
   retry, nombre de sauts de circuit) → une structure de configuration
   documentée dans le crate concerné (`DaemonConfig` dans
   `tribler-daemon`, `TunnelConfig` dans `tribler-tunnel`, etc.), jamais un
   littéral dans la logique.
8. **Un composant par fichier** : toute fonction ou bloc logique qui peut
   vivre dans son propre fichier doit l'avoir — pas de fichier monolithique.
9. **Mise à jour de la documentation à chaque étape** : chaque étape de
   `docs/plans/roadmap.md` complétée doit être cochée, `docs/CHANGELOG.md`
   mis à jour, et un commit git local créé avant de passer à l'étape
   suivante (cf. section "Workflow par étape").

## Conventions de code

- **Langue** : code et documentation en français (cohérent avec les autres
  projets de l'utilisateur, ex. eMule-Rust).
- **Erreurs Rust** : `thiserror` pour les erreurs typées, pas de `unwrap()`
  en dehors des tests.
- **Logging** : `tracing` (jamais `println!` dans les crates de bibliothèque
  — `println!` toléré uniquement dans les squelettes `main.rs` avant
  l'étape d'implémentation réelle du daemon/CLI).
- **Async** : Tokio ; pas de blocage synchrone dans les tâches async.
- **Warnings** : le workspace compile avec `-D warnings` (clippy) ; aucun
  warning toléré.
- **Tests** : pas d'assertions triviales sur des constantes
  (`clippy::assertions_on_constants`) — voir les tests des crates squelette
  pour le pattern accepté.

## Anti-duplication

Une fonctionnalité = **un seul propriétaire**. Avant d'écrire un helper,
vérifier qu'il n'existe pas déjà :

| Besoin | Propriétaire unique |
| :--- | :--- |
| Fixtures/helpers de tests cross-crates | `crates/tribler-test-support` (dev-dependency) |
| Hachage BitTorrent / clés IPv8 / crypto tunnel | `crates/tribler-crypto` |
| Parsing `.torrent`/magnet/`.mdblob` | `crates/tribler-format` |
| Politiques réseau (anti-SSRF, exit policy, kill switch) | `crates/tribler-network-policy` |
| Traduction HTTP/JSON ↔ domaine | `crates/tribler-api` uniquement |

## Workflow par étape (obligatoire)

Pour chaque étape de `docs/plans/roadmap.md` :

1. Implémenter l'étape dans le(s) crate(s) concerné(s).
2. Écrire/adapter les tests correspondants (unitaires puis, si pertinent,
   test d'intégration via `tribler-test-support`).
3. Exécuter `scripts/verify_all.ps1` (ou au minimum check/clippy/test du
   crate modifié) et corriger jusqu'à ce que ça passe.
4. Mettre à jour `docs/plans/roadmap.md` (cocher l'étape), `docs/CHANGELOG.md`
   et, si l'architecture a changé, `docs/architecture/architecture.md` /
   ajouter une ADR.
5. Créer un commit git local dédié à l'étape (message clair, en français,
   décrivant le "pourquoi").

## Documentation associée

- `docs/plans/plan_faisabilite.md` — analyse de faisabilité et risques
- `docs/plans/roadmap.md` — plan d'implémentation détaillé, étape par étape
- `docs/architecture/architecture.md` — vue d'ensemble de la clean architecture
- `docs/architecture/decisions/` — ADRs
- `docs/reference_tribler/` — correspondance Python Tribler ↔ Rust
- `docs/INDEX.md` — inventaire des fichiers
- `docs/CHANGELOG.md` — historique des étapes
