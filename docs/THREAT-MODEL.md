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

## Embedded Tor lane (ADR-0028)

OnionBit embeds an Arti/Tor client (`tor.enabled`, default on) used
**only** for approved `.onion` metadata — search sources — and the
reserved phase-3 services. It never carries OnionBit torrent traffic,
and OnionBit tunnels never carry Tor: the two planes are disjoint by
construction.

- **ISP sees Tor usage**: the bootstrap downloads a consensus from
  publicly listed directory authorities/guards — the *fact* of Tor use
  is observable even though content is not. `tor.bridges` (unlisted
  relays) defeats IP-blocklists of public guards but **not** DPI
  fingerprinting of the Tor handshake — pluggable transports
  (obfs4/Snowflake) are a separate, not-yet-built work item.
- **Bridge-only intent is honored strictly**: with a non-empty
  `tor.bridges`, bootstrap is bridges-only (`enabled: Explicit(true)`)
  and fails hard rather than silently falling back to public guards.
- **Search↔swarm correlation**: an observer watching both the onion
  service and the OnionBit overlay could link a query to a swarm join by
  timing/content. Mitigations: no auto-download from external results,
  per-request bounded circuits, jitter on gateway paths. Assumed limit.
- **Gateway trust (`EXIT_TOR`, phase 2 — not built)**: a peer without
  Tor could exit through an OnionBit gateway's local Arti; that gateway
  would see queries in the clear (honeypot risk), mitigated by the
  requester-side `number_hops ≥ 3` constraint and explicit UI warning —
  not eliminable.
- **No clearnet egress**: every target — including every redirect hop —
  must satisfy `TorTargetPolicy` (strict `.onion` allowlist) **before**
  any connection; a dead onion means a typed error, never a direct
  fallback. The fetcher is a minimal bounded GET client (no cookies, ≤
  `max_response_bytes`, ≤ `max_redirects`).
- **`state_dir/tor` sensitivity is low**: consensus, guard state,
  keystore — no OnionBit identity material. Consistent with the
  `state_dir` model.
- **Query terms are never logged**; source identifiers appear in logs
  and SSE (`remote_query_results`, `tor:<source>`) behind the API key.

## Hosted `.onion` services (ADR-0029)

Phase 3 lets an identity *host* `.onion` services — messaging, channel
catalogs and the private vault — next to the OnionBit overlay. The two
planes remain disjoint: a `.onion` contact address is used over Tor,
an `ipv8` key over tunnel circuits, never translated.

- **Presence is observable**: a `.onion` that answers proves the peer is
  online right now — an activity signal a censor or probing party can
  collect. OnionBit makes no continuous presence pings (Briar model:
  rendezvous at send time only), but hosting itself is publishable
  activity. Assumed limit.
- **Inter-plane correlation**: serving the *same* content on both planes
  (a channel catalog over IPv8 **and** `.onion`, a reachable contact on
  both) lets an observer of both networks link the identities by timing
  and content. The opt-in `.onion` binding signature
  (`onionbit/onion-bind/v1`) makes that link explicit *by choice* — it is
  the only authorized correlation; absent it, addresses stay unlinkable.
- **Vault authorization is discovery-level, not secrecy**: the vault
  service (`tor-hs-vault/v1` address) is locked by Arti
  restricted-discovery — its descriptor is decryptable only by the
  x25519 client key derived from the same identity (`tor-hs-client/v1`).
  This blocks discovery/probing by third parties; the served blobs
  (`OBV1`, `manifest.obm`) remain AEAD-encrypted for the master key
  anyway — a stolen blob is unreadable. Caveat: Arti's
  `restricted-discovery` is `__is_experimental` (non-semver API) and
  revoking a client does not tear down already-reached introduction
  points — OnionBit's revocation story is therefore "rotate the
  identity", documented rather than patched.
- **Service fingerprinting**: the vault service serves plain bounded
  HTTP on its virtual port — indistinguishable from an ordinary onion
  site at the descriptor level; its *address* is never published in
  channels or the public overlay (derived-only, known to the owner's
  devices). Messaging/channel services ride the identity `.onion`,
  multiplexed by virtual `BEGIN` port; unsubscribed ports get the
  standard Tor `DONE`.
- **No new trust for content**: channel catalog entries served over
  `.onion` are still `mdblob` signatures verified against the channel
  key — the transport never grants credibility; a poisoned entry is
  rejected exactly as on IPv8.

## Embedded `.onion` browser (ADR-0030)

The bundle can ship `windows\bin\onionbit-browser.exe` (DilloRust) —
a minimal `.onion` reader launched by `POST /api/browser/launch`
through a loopback HTTP forward proxy served by `onionbit-tor`. The
browser is "dumb": all Tor transport lives in the daemon's single
`TorClient` (one bootstrap, one guard set). Measured
`scripts/sec_browser_leak.ps1`: `NO_LEAK` — every TCP socket of the
browser PID (and its descendants) ends at the loopback proxy, zero
UDP, zero spawned child, ephemeral profile wiped at exit.

**Residuals — listed honestly, not claimed solved:**

- **Passive fingerprinting toward the `.onion` service**: Dillo's
  `User-Agent`/`Accept` headers and rendering quirks identify *a
  Dillo browser* — not OnionBit specifically. Tor Browser solves
  this by uniformization across its whole user base; Dillo's
  footprint is distinct and rare. Assumed, not mitigated.
- **Loopback proxy is observable by local processes**: any process
  running as the same user can see — and could use — the proxy
  listener. `tor.browser_proxy.require_auth` (default off, token
  per session delivered via the generated `dillorc`, never argv)
  raises this to "local process must also have read the ephemeral
  profile". A same-user compromise remains out of scope.
- **`allow_clearnet_exit` exposes the destination to the exit
  relay**: `https-only` keeps content encrypted but the exit learns
  `CONNECT host:443`; `on` additionally lets plain HTTP leave the
  exit. Default `off` — `.onion` only. User's choice, not silently
  enabled.
- **`.onion` subdomains are transmitted to the service**: arti
  routes by the last two labels; the full hostname (`www.`+base)
  reaches the destination service, which learns which of its
  subdomains was asked. Subdomains share the isolation token —
  matching Tor semantics, not a bug.
- **Per-origin isolation, not per-tab**: circuits are grouped per
  `.onion` service (first-party model) — `browser_proxy.
  isolation_entries` in `GET /api/tor/status` makes the separation
  observable. Two pages of the same service share circuits; Tor
  Browser isolates finer (per first-party URL bar origin, similar).
- **Persistent trace is opt-in only**: `keep_profile=false`
  (default) wipes `state/browser/run-*` at exit; `true` keeps
  HSTS/dpi keys/history **in cleartext** under `state/browser/` —
  these files live outside the encrypted database.
- **Same lane as search**: browsing and external search share the
  `TorClient` and guard set — an observer watching guard traffic
  volume/timing could correlate the two activities, though they
  ride distinct circuits.

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
