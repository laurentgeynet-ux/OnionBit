# Changelog — étapes franchies

Format : une entrée par étape de `docs/plans/roadmap.md`, la plus récente
en haut.

## Étape 1 — `tribler-format` : bencode, `.torrent`, magnet, `.mdblob` (2026-09-28)

- Parser bencode borné maison (`bencode/parser.rs`) : profondeur max,
  tailles de chaînes/listes/dicts limitées via `limits.rs`, offsets
  d'erreur précis, pas de dépendance réseau. Encodeur canonique
  (`bencode/encoder.rs`) avec clés de dict triées (BEP 3).
- `torrent.rs` : modèle `TorrentMeta` complet (announce, announce-list,
  fichiers multi, taille totale, private, comment, created by,
  creation date, url-list). Info-hash v1 = SHA-1 des **octets bruts**
  du dict `info` (`extract_raw_info` suit les offsets du fichier
  source, pas une re-sérialisation). Support partiel v2/hybride :
  `pieces root`, arbre `file tree`.
- `magnet.rs` : liens `magnet:?xt=urn:btih:` (hex 40c et base32) et
  `urn:btmh:` (v2), paramètres `dn`, `tr`, `ws`, `as`, `x.pe`.
- `mdblob.rs` : lecture séquentielle des `SignedPayload` pyipv8
  (`H type · H flags · 64s pubkey · champs · 64s signature`), mapping
  fidèle à `core/database/serialization.py` : `REGULAR_TORRENT`,
  `CHANNEL_TORRENT`, `COLLECTION_NODE`, `JSON_NODE`,
  `CHANNEL_DESCRIPTION`, `BINARY_NODE`, `CHANNEL_THUMBNAIL`, `DELETED` ;
  types absents du mapping Python rejetés comme
  `UnknownBlobTypeException`. Vérification de signature Ed25519 via
  `tribler-crypto`.
- 18 tests offline, `clippy -D warnings` et `fmt` propres.

## Étape 2 — `tribler-crypto` : hachage et crypto IPv8 (2026-09-28)

- `hash.rs` : SHA-1/SHA-256 + hex, validé contre vecteurs officiels.
- `ipv8/keys.rs` : clés `LibNaCLPK`/`LibNaCLSK` au format binaire pyipv8
  (préfixes `LibNaCLPK:`/`LibNaCLSK:` + paire X25519/Ed25519), MID
  = SHA-1 de la clé publique, signature/vérif Ed25519.
- `ipv8/dh.rs` : Diffie-Hellman X25519 fidèle à `ipv8-rust-tunnels`
  (`crypto_box_beforenm` + HSalsa20).
- `ipv8/session.rs` : dérivation de clés de session HKDF-SHA256
  (rôle client/serveur), compteur borné.
- Chiffrement authentifié **ChaCha20-Poly1305** (et non AES-GCM :
  correction de fidélité vs le plan initial, cf. roadmap notes).
- 16 tests (vecteurs, symétrie DH, rejet de tag invalide).

## Étape 0 — Mise en place du dépôt (2026-09-27)

- Dépôt git local initialisé (`D:\Projet\Tribler-Rust-Torrent`).
- Licence GPL-3.0-or-later (texte officiel FSF) ajoutée en `LICENSE`.
- `AGENTS.md` rédigé (règles critiques, conventions de code, workflow
  par étape, anti-duplication), inspiré de la structure du projet
  eMule-Rust de l'utilisateur.
- Analyse de l'architecture officielle Tribler
  (`D:\Projet\Tribler_sources\tribler`) : cartographie des modules
  Python (`core/libtorrent`, `core/database`, `core/restapi`,
  `core/tunnel`, `core/content_discovery`, `core/torrent_checker`,
  `core/socks5`, `core/rss`, `core/watch_folder`, `pyipv8/`).
- Recherche d'écosystème Rust : découverte de `librqbit` (moteur
  BitTorrent Rust pur mûr, Apache-2.0) et confirmation qu'aucune
  implémentation Rust complète du protocole IPv8 n'existe
  (`ipv8-rust-tunnels` ne couvre que le plan de données des tunnels).
- Décisions d'architecture actées avec l'utilisateur et formalisées en
  ADRs : ADR-0001 (moteur BitTorrent Rust pur via `librqbit`), ADR-0002
  (IPv8 inclus dès la V1), ADR-0003 (licence GPL-3.0), ADR-0004
  (structure workspace Cargo inspirée d'eMule-Rust), ADR-0005 (langue
  française).
- Workspace Cargo créé avec 12 crates squelettes
  (`tribler-format`, `tribler-crypto`, `tribler-bittorrent`,
  `tribler-ipv8`, `tribler-tunnel`, `tribler-core`, `tribler-db`,
  `tribler-network-policy`, `tribler-api`, `tribler-cli`,
  `tribler-daemon`, `tribler-test-support`) : chaque crate compile,
  documente sa responsabilité et son étape d'implémentation prévue, et
  passe `cargo check`/`cargo clippy -D warnings`/`cargo fmt --check`/
  `cargo test`.
- `docs/plans/plan_faisabilite.md`, `docs/architecture/architecture.md`,
  `docs/plans/roadmap.md`, `docs/reference_tribler/`, `docs/INDEX.md`
  rédigés.
- `scripts/verify_all.ps1` créé (validation complète).
