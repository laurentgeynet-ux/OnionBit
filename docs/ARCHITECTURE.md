# OnionBit — Architecture overview

Clean-layered port of Tribler to Rust. Dependencies point inward; the UI and CLI
never touch the engine directly — everything goes through the REST + SSE API.

## Layers

| Layer | Role |
| :--- | :--- |
| **Control plane** (axum) | REST + SSE API, loopback-only by default. Sole entry point for UI, CLI, third parties. Functional parity with `tribler.core.restapi`. |
| **Domain core** | Session orchestration, notifier/events, discovery, channels, RSS, watch folder, torrent health checking. |
| **BitTorrent engine** | Thin wrapper over [librqbit](https://github.com/ikatson/rqbit): bencode, peer-wire, mainline DHT (BEP 5), uTP, HTTP/UDP trackers. |
| **IPv8 overlay** | Rust port of `pyipv8`: peer discovery, communities, attestation-ready identities, DHT overlay. |
| **Tunnel community** | Rust port of `TunnelCommunity`: onion circuits (1–3 hops), hidden seeding, exit nodes, SOCKS5 ingress. |
| **Persistence** | SQLite — torrents, channels, votes, settings. |
| **Network policy** | Cross-cutting guardrails: anti-SSRF, exit-node policy, kill switch, SOCKS5 egress rules. Non-negotiable. |
| **UI** | Flutter app consuming only the REST API. Targets Windows, Linux, macOS, Android, iOS, Web. |

## Trust boundaries

- The REST API binds to `127.0.0.1` by default; remote mode requires explicit opt-in.
- In anonymous mode, **all** peer-facing traffic (peer-wire, uTP, UDP trackers,
  DHT) is injected through tunnel sockets — the engine cannot emit a raw packet
  to a remote peer.
- The daemon and UI share state exclusively through the API — no shared memory,
  no FFI between them on desktop (mobile uses an FFI facade to the daemon core).

## Design decisions

Architecture decisions are recorded as ADRs under `docs/architecture/decisions/`
in the development repository — key ones: reuse librqbit rather than rewrite the
BitTorrent stack; port IPv8/TunnelCommunity rather than adopt an unrelated mixnet;
GPL-3.0 licensing inherited from Tribler.
