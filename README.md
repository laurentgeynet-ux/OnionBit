<div align="center">

<img src="assets/logo-horizontal.svg" alt="OnionBit" width="460"/>

### Anonymous BitTorrent client, native in Rust

**A full Rust port of [Tribler](https://github.com/Tribler/tribler) — torrent through onion-routed circuits, with no traceable IP.**

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](LICENSE)
[![Made with Rust](https://img.shields.io/badge/Made%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![UI: Flutter](https://img.shields.io/badge/UI-Flutter-02569B.svg)](https://flutter.dev/)
[![Version](https://img.shields.io/badge/Version-0.3.1--alpha-red.svg)]()
[![Platform](https://img.shields.io/badge/Platform-Windows%20%C2%B7%20Linux%20%C2%B7%20macOS%20%C2%B7%20Android%20%C2%B7%20iOS%20%C2%B7%20Web-lightgrey.svg)]()

</div>

---

## What is OnionBit?

OnionBit is a BitTorrent client where **anonymity is built into the protocol**, not bolted on.
It is a native Rust port of [Tribler](https://github.com/Tribler/tribler) — the anonymous
BitTorrent daemon originally written in Python — reimplemented as a fast, embeddable
Rust engine with a cross-platform Flutter UI.

Instead of connecting directly to the swarm, OnionBit can route all BitTorrent traffic
through **multi-hop onion circuits** built on a port of the IPv8 overlay protocol, the
same design Tribler pioneered:

- 🔗 **Multi-hop encrypted circuits** — up to 3 hops between you and the swarm
- 🧅 **Onion routing** — each relay only knows its neighbors, never the endpoints
- 🌱 **Hidden seeding** — seed content without exposing your IP address
- 🔍 **Decentralized search** — content discovery through the overlay, no central index
- 🛡️ **Kill switch & leak protection** — no clearnet fallback, no DNS/UDP leaks
- 🦀 **Pure Rust core** — memory-safe, single binary, embeddable via REST API

> **Status: alpha.** The engine is under active development and validated against the
> real Tribler network (Tribler 8.x interop testbench). Not yet recommended for
> high-stakes anonymity.

## Architecture

```
┌─────────────────┐  ┌──────────────┐  ┌────────────────────┐
│  Flutter UI     │  │     CLI      │  │  3rd-party tools   │
│  (all platforms)│  │              │  │                    │
└────────┬────────┘  └──────┬───────┘  └─────────┬──────────┘
         └──────────────────┼────────────────────┘
                            │ REST + SSE (loopback by default)
┌───────────────────────────▼────────────────────────────────┐
│                    Control plane (axum)                    │
├────────────────────────────────────────────────────────────┤
│  Domain core — sessions, discovery, channels, settings     │
├──────────────┬──────────────┬───────────────┬──────────────┤
│ BitTorrent   │ IPv8 overlay │ Tunnel comm.  │ SQLite store │
│ (librqbit)   │ (port)       │ (onion rout.) │              │
├──────────────┴──────────────┴───────────────┴──────────────┤
│  Network policy — anti-SSRF · exit policy · kill switch    │
└────────────────────────────────────────────────────────────┘
```

- **BitTorrent engine** — built on [librqbit](https://github.com/ikatson/rqbit) (Apache-2.0):
  bencode, peer-wire, mainline DHT (BEP 5), uTP, trackers.
- **Anonymity layer** — a Rust port of `pyipv8` + `TunnelCommunity`: overlay discovery,
  onion circuits, hidden services, verified against live Tribler nodes.
- **Control plane** — REST + SSE API on loopback by default; the UI, CLI and any
  third-party tool all go through the same door.
- **UI** — Flutter, one codebase for Windows, Linux, macOS, Android, iOS and Web.

## Getting started

> **Windows x64 alpha zip** is on the
> [Releases](https://github.com/laurentgeynet-ux/OnionBit/releases) page
> (`OnionBit-0.3.1-alpha-windows-x64.zip`): unzip, run `onionbit_ui.exe` —
> it starts the daemon automatically. Other platforms: build from source
> (see [docs/BUILDING.md](docs/BUILDING.md)).

**Prerequisites:** Rust stable, Flutter stable (UI only).

```bash
# Daemon (control plane + engine)
cargo build --release -p onionbit-daemon
./onionbit-daemon

# CLI
cargo run -p onionbit-cli -- downloads list

# Flutter UI
cd app && flutter run -d windows   # or linux / chrome / android
```

The daemon exposes `http://127.0.0.1:8085` (loopback only by default) — the same
endpoint the UI and CLI consume. Full build & packaging notes:
[docs/BUILDING.md](docs/BUILDING.md).

## Security model

Anonymity tooling fails quietly. OnionBit treats leak prevention as a hard invariant:

- **No clearnet fallback** — anonymous downloads never silently degrade to direct connections
- **Kill switch** — traffic halts when circuits collapse
- **Anti-SSRF & loopback isolation** — the API cannot be coerced into reaching internal services
- **Exit policy enforcement** — exit nodes honor a strict policy
- **Sandboxed trackers/DHT** — in anonymous mode, tracker and DHT traffic rides inside the tunnel

See [SECURITY.md](SECURITY.md) for reporting and the threat model.

## Roadmap

| Milestone | Status |
| :--- | :--- |
| BitTorrent engine (librqbit integration) | ✅ |
| REST + SSE control plane, CLI | ✅ |
| IPv8 overlay port (discovery, communities) | 🚧 |
| Onion circuits + hidden seeding | 🚧 |
| Live interop with Tribler 8.x nodes | 🚧 |
| Flutter UI (desktop first) | 🚧 |
| Mobile execution model (Android/iOS) | 📋 |
| First tagged alpha release | 📋 |

## Contributing

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). Big-ticket items:
IPv8 protocol conformance, circuit crypto, Flutter UI, interop testing.

## Acknowledgements

- **[Tribler](https://github.com/Tribler/tribler)** — the original anonymous BitTorrent
  client (Delft University of Technology). OnionBit ports its architecture and
  protocols; no Python source is copied verbatim.
- **[librqbit / rqbit](https://github.com/ikatson/rqbit)** — the excellent Rust
  BitTorrent engine underneath.
- **[pyipv8](https://github.com/Tribler/py-ipv8)** — reference implementation of the
  IPv8 overlay protocol.

## License

[GPL-3.0-or-later](LICENSE) — inherited from Tribler. OnionBit is a derivative work
of Tribler's GPL-3.0 codebase at the architecture/behavior level.

---

<div align="center">
<b>OnionBit</b> — peel the layers, not your privacy. 🧅
</div>
