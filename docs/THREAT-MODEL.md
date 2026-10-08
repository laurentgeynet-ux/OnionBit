# Threat model

OnionBit is an anonymous, censorship-resistant peer-to-peer ecosystem —
the onion fabric below carries file sharing, messaging and portable
identity (see
[ADR-0020](architecture/decisions/0020-definition-ecosysteme-p2p-anonyme.md)
for the canonical definition). This document bounds the anonymity and
anti-censorship claims: what the fabric protects against, and what it
explicitly does not.

## What OnionBit protects against

- **Swarm peers learning your IP** — in anonymous mode, peers only ever see the
  exit node's address.
- **Your ISP/local observer seeing BitTorrent or OnionBit traffic** — tunnel
  traffic is layered encryption over ordinary UDP; it does not look like
  peer-wire.
- **Hidden seeder exposure** — a seeder can serve content behind rendezvous
  circuits without publishing a reachable address.
- **Server-side surveillance of conversations** — messaging has no server to
  compel or breach: frames travel e2e-encrypted over hidden services and
  relays only forward cells.
- **Censorship by protocol fingerprint** — stealth mode (ADR-0017) removes
  every static protocol marker: morphed wire format, Elligator2 ephemeral
  keys, uniform silence under probing, bridge links distributed out-of-band.

## What it does NOT protect against

- **Global passive adversary** (nation-state monitoring both ends of the
  network): OnionBit circuits lack the padding/constant-rate defenses needed
  for that threat class. Tribler's design — and OnionBit's — targets
  peer-level and local-observer anonymity, not Tor-grade protection.
- **Malicious exit collusion**: with short circuits, a colluding first hop and
  exit could correlate flows. Longer hop counts reduce but do not eliminate
  this.
- **Application-level leaks**: files you download can still identify you
  (watermarked content, telemetry embedded in media, etc.).
- **Traffic volume and timing analysis** — stealth mode removes static
  markers, not the *amount* or *timing* of traffic; a censor can still see
  that something flows, and blanket UDP throttling breaks the transport.
- **Bridge link distribution** — a first `onionbit-bridge://` link must reach
  the censored user out-of-band; a leaked link can get that bridge blocked.

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

## Beta disclaimer

The tunnel implementation has completed bidirectional interoperability with
live Tribler 8.x nodes (see `docs/interop/README.md`) — yet OnionBit remains
beta software. Treat anonymity as a design goal with the limits above, not a
guarantee; it is not yet recommended for high-stakes anonymity.
