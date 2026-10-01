# Interoperability and resilience evidence

## Scope

OnionBit has been tested against **unmodified Tribler 8.4.3** in controlled
loopback meshes and on the public Tribler network.

These results demonstrate *protocol interoperability* — wire formats, circuit
construction and recovery, complete transfers, and content integrity. They do
**not** constitute a guarantee of anonymity, security, performance, or
availability on every network. Anonymity is a broader property that requires a
threat model; see [`../THREAT-MODEL.md`](../THREAT-MODEL.md).

## Verified scenarios

Controlled loopback mesh: isolated nodes on localhost, Tribler 8.4.3 running
its real `ipv8-rust-tunnels` stack. Each transfer is integrity-checked twice:
by BitTorrent piece hashing during download, then by an independent SHA-256 of
the received file against the source.

| Scenario | Seeder | Downloader | Hops | Result | Integrity |
|---|---|---|---:|---|---|
| Hidden-service interop A | OnionBit | Tribler 8.4.3 | 1 | PASS | pieces + SHA-256 |
| Hidden-service interop B | Tribler 8.4.3 | OnionBit | 1 | PASS | pieces + SHA-256 |
| Multi-hop interop A | OnionBit | Tribler 8.4.3 | 2 | PASS | pieces + SHA-256 |
| Multi-hop interop B | Tribler 8.4.3 | OnionBit | 2 | PASS | pieces + SHA-256 |
| Multi-hop interop A | OnionBit | Tribler 8.4.3 | 3 | PASS | pieces + SHA-256 |
| Multi-hop interop B | Tribler 8.4.3 | OnionBit | 3 | PASS | pieces + SHA-256 |
| Seeder kill + restart | both directions | both directions | mesh | PASS | resume to 100 %, SHA-256 |
| Introduction-point kill | both directions | both directions | mesh | PASS | intro rebuilt + DHT re-announce, SHA-256 |
| Bootstrap-anchor kill | both directions | both directions | mesh | PASS¹ | SHA-256 |
| Public-network download | public swarm | OnionBit | 2–3 | PASS² | verified BitTorrent pieces |

¹ When the killed anchor is the *only* `EXIT_BT`-capable node in the mesh, DATA
circuit reconstruction is logically impossible (`select_exit` has no
candidate). This is a bench-topology limitation, not a protocol breakage — the
mesh was given a second exit node, mirroring the real network.

² Public runs are non-deterministic by nature. A first 3-hop attempt timed out
on circuit construction (relay scarcity); the retry succeeded. All runs —
successes **and** failures — are logged with exact parameters in
[`public_dht_runs.md`](public_dht_runs.md).

## What is exercised on the wire

- **Tunnel handshake & circuits** — `create`/`created`/`extend`/`extended`,
  multi-hop circuit construction through pyipv8 relays.
- **Hidden services (e2e)** — introduction points (`establish-intro`), DHT
  announcement and lookup (`DHTIntroPointPayload`), rendezvous
  (`create-e2e`/`created-e2e`, `link-e2e`/`linked-e2e`, `RendezvousInfo` as
  `NestedPayload`), `peers-request`/`peers-response` framed lists.
- **Data path** — `data` cells to exit UDP sockets, uTP carried inside linked
  e2e circuits, BitTorrent handshake and piece transfer over uTP.
- **DHT over tunnel** — public DHT bootstrap (`router.bittorrent.com` …)
  routed through exit nodes; mainline DHT queries via `TunnelUdpSocket`.
- **Discovery** — IPv8 walk introductions exchanged with real Tribler peers.

## Failure semantics verified

- **Seeder kill** — bounded drain measured, strict dead window, restart, swarm
  re-discovery, resume to 100 %.
- **Introduction-point kill** — established e2e flows survive; the seeder
  detects the dead peer (churn), rebuilds an intro point on a *different* node,
  re-announces on the DHT; the verifier checks the announcement is attributable
  to the seeder key and the rebuilt circuit is genuinely new.
- **Bootstrap-anchor kill** — established flows survive; peer discovery
  continues without the bootstrap node.

## Reproduce

All benches are driven by scripts in `scripts/` (Windows, PowerShell):

| Script | What it proves |
|---|---|
| `interop_hidden_tribler_download.ps1` | Tribler 8.4.3 downloads from an OnionBit anonymous seeder (direction A) |
| `interop_hidden_tribler_seed.ps1` | OnionBit downloads from a Tribler 8.4.3 anonymous seeder (direction B) |
| `interop_hidden_killseeder.ps1` | `-KillTarget seeder\|intro\|anchor` resilience scenarios |
| `interop_public_dht.ps1` | real-network download over live Tribler relays + public DHT |
| `live_hidden_upload.ps1` | all-Rust 3-daemon hidden seeding bench (8 verdicts) |

Run artifacts (logs, verdict files, SHA-256 reports) are written under
`target/interop-*/`. The benches require a local Tribler 8.4.3 installation;
paths are environment-overridable — see [`../BUILDING.md`](../BUILDING.md).
