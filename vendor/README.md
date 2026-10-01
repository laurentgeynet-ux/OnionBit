# Vendored dependencies

This directory contains source dependencies vendored for **controlled patching
and reproducible builds**. They are wired in via `[patch.crates-io]` in the
root `Cargo.toml` and are *not* workspace members.

Upstream for all components: [rqbit](https://github.com/ikatson/rqbit)
(BitTorrent engine by ikatson), fetched from crates.io.

## Inventory

| Component | Version | License | Local patches |
|---|---|---|---|
| `librqbit` | 9.0.1 | Apache-2.0 | `ConnectionOptions.utp_socket` (`UtpConnector` trait, blanket impl on `UtpSocket`), `DhtSessionConfig.socket`, `SessionOptions.udp_tracker_socket`, injected listener socket, SOCKS5 proxy used for peers only when `enable_tcp`, handshake instrumentation logging |
| `librqbit-dht` | 9.0.1 | Apache-2.0 | `DhtConfig.socket` / `PersistentDht::create`: injectable `DatagramSocket` instead of a real UDP bind |
| `librqbit-tracker-comms` | 9.0.1 | Apache-2.0 | `UdpTrackerClient::new_with_socket`: injectable datagram socket |
| `librqbit-utp` | 0.7.0 | Apache-2.0 | re-export of `UtpEnvironment` / `DefaultUtpEnvironment` (private upstream — needed to name `UtpSocket<T, _>`) |
| `librqbit-dualstack-sockets` | 0.7.0 | Apache-2.0 | new object-safe `DatagramSocket` trait (`send_to`/`recv_from`/`bind_addr`) + `UdpSocket` impl, `Debug` bound |

## Why vendored

Upstream librqbit cannot route its UDP traffic (uTP peers, DHT, UDP trackers)
through a proxy — its SOCKS5 support is TCP-only and it binds real UDP sockets.
Anonymous lanes must emit *no* real UDP datagrams at all: the vendored sockets
are replaced by `TunnelUdpSocket`, which carries traffic inside `data` cells
over onion circuits. Full rationale and wire-fidelity analysis:
[ADR-0007](../docs/architecture/decisions/0007-librqbit-vendored-udp-tunnel.md).

## Patch policy

- Every local patch is marked inline — search for `vendored patch` in source
  comments (markers carry the historical tag `Tribler-Rust-Torrent`, the
  former project name).
- Patches are minimal API surfaces: injectable sockets, re-exports, one
  proxy-routing condition. No protocol-logic divergence from upstream.
- Updating a vendored component requires reviewing and reapplying — or
  intentionally dropping — every patch above, then re-running the interop
  benches ([`docs/interop/`](../docs/interop/README.md)).
- Upstream license notices are preserved; all components remain Apache-2.0.

## Security and maintenance

Vendored copies do not receive automatic upstream fixes — `cargo audit` /
Dependabot findings on `librqbit*` must be evaluated against this directory.
Track upstream releases manually and follow the patch policy on upgrade.
