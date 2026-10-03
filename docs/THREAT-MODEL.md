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

## Alpha disclaimer

The tunnel implementation is being validated against live Tribler 8.x nodes.
Until the interop milestones are complete, treat anonymity as best-effort.
