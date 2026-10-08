# Threat model

## What OnionBit protects against

- **Swarm peers learning your IP** — in anonymous mode, peers only ever see the
  exit node's address.
- **Your ISP/local observer seeing BitTorrent traffic** — tunnel traffic is
  layered encryption over ordinary UDP; it does not look like peer-wire.
- **Hidden seeder exposure** — a seeder can serve content behind rendezvous
  circuits without publishing a reachable address.

## What it does NOT protect against

- **Global passive adversary** (nation-state monitoring both ends of the
  network): OnionBit circuits lack the padding/constant-rate defenses needed
  for that threat class. Tribler's design — and this port — targets
  peer-level and local-observer anonymity, not Tor-grade protection.
- **Malicious exit collusion**: with short circuits, a colluding first hop and
  exit could correlate flows. Longer hop counts reduce but do not eliminate
  this.
- **Application-level leaks**: files you download can still identify you
  (watermarked content, telemetry embedded in media, etc.).

## Private download area (ADR-0018)

The portable bundle splits downloads into a **public** area
(`data/public/`, cleartext) and a **private** area (`data/private/`,
encrypted and identity-bound):

- Private payloads are chunked `OBD` files (AEAD per chunk, random nonce per
  write, sealed header). Group directories, database row keys and fastresume
  `.bitv` files are all `HMAC(K_names, infohash)` — no cleartext name,
  infohash or path is persisted outside the encrypted `manifest.obm`.
- The private catalogue lives in `data/private/manifest.obm` (sealed, atomic
  rewrite with `.bak` rotation). Recovery after double loss rebuilds minimal
  entries from the sealed `scan_ct` headers of orphan `.obd` files.
- Keys derive from the identity root (`store_root` — HKDF domains per
  purpose). A bundle copied to a machine **without the same identity** sees
  inert `.obd` blobs and an empty private listing — verified by
  `scripts/bench_portable.ps1` (scenario C).
- Guest sessions use `data/private/temp/.guest/`, write no manifest, and
  purge on shutdown.

**Limits — read carefully on removable media:**

- **FAT32/exFAT (most USB keys)**: no POSIX ACLs — anyone who can mount the
  media can read the *public* zone and the identity files. The private zone
  stays encrypted, but the unsealed identity seed (`state/identity/
  identity_seed.bin`) would let an attacker decrypt it. Enable
  `identity.at_rest` (Settings → Identity) so the seed is sealed `OBSK` —
  the UI proposes it automatically when a removable/ACL-less volume is
  detected (`storage_removable` in `GET /api/identity`).
- **No plausible deniability**: `OBD` blob entropy and group counts are
  observable — an adversary can tell an encrypted area *exists*. Hidden
  volumes are out of scope.
- **No sparse/`noexec` assumptions**: FAT32 has no sparse files or POSIX
  exec bits; on Linux/macOS removable mounts are often `noexec` — copy the
  binaries locally or remount `exec` (see `docs/BUILDING.md`).

## Alpha disclaimer

The tunnel implementation is being validated against live Tribler 8.x nodes.
Until the interop milestones are complete, treat anonymity as best-effort.
