#!/usr/bin/env python3
"""Noeud d'interop pyipv8 (vrai DiscoveryCommunity Python) pour le jalon
Rust<->pyipv8. Enregistre chaque datagramme brut (rx+tx) en hex dans un
journal, envoie des introduction-request a une cible, puis verifie que
les paquets recus se decodent (signature incluse) via `ez_decode`.

Usage : py_node.py --port N --target 127.0.0.1:P --duration S --log FILE
Necessite : PYTHONPATH=<...>/pyipv8  et le venv interop (voir
scripts/interop_ipv8.ps1).
"""

from __future__ import annotations

import argparse
import asyncio
import sys
import time

from ipv8.community import CommunitySettings
from ipv8.keyvault.crypto import default_eccrypto
from ipv8.messaging.interfaces.udp.endpoint import UDPEndpoint, UDPv4Address
from ipv8.peer import Peer
from ipv8.peerdiscovery.community import DiscoveryCommunity
from ipv8.peerdiscovery.network import Network


class RecordingEndpoint(UDPEndpoint):
    """UDPEndpoint qui journalise chaque datagramme en hex."""

    def __init__(self, port: int, log):
        super().__init__(port=port, ip="127.0.0.1")
        self._log = log

    def send(self, socket_address, packet) -> None:
        self._log.write(f"TX|{socket_address[0]}:{socket_address[1]}|{packet.hex()}\n")
        self._log.flush()
        super().send(socket_address, packet)

    def datagram_received(self, datagram, addr) -> None:
        self._log.write(f"RX|{addr[0]}:{addr[1]}|{datagram.hex()}\n")
        self._log.flush()
        super().datagram_received(datagram, addr)


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--target", required=True, help="host:port du noeud Rust")
    parser.add_argument("--duration", type=float, default=8.0)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()

    host, port_s = args.target.rsplit(":", 1)
    target = UDPv4Address(host, int(port_s))

    with open(args.log, "w", encoding="ascii") as log:
        endpoint = RecordingEndpoint(args.port, log)
        await endpoint.open()

        key = default_eccrypto.generate_key("curve25519")
        my_peer = Peer(key, UDPv4Address("127.0.0.1", args.port))
        network = Network()
        settings = CommunitySettings()
        settings.my_peer = my_peer
        settings.endpoint = endpoint
        settings.network = network
        community = DiscoveryCommunity(settings)

        deadline = time.time() + args.duration
        # Envoie des introduction-request (ancien format : loopback IPv4)
        # jusqu'a ce que le pair Rust soit verifie.
        while time.time() < deadline:
            if community.get_peers():
                break
            packet = community.create_introduction_request(target)
            endpoint.send(target, packet)
            await asyncio.sleep(0.5)

        await asyncio.sleep(max(0.0, deadline - time.time()))
        n_peers = len(community.get_peers())
        sys.stderr.write(f"python node peers verifies : {n_peers}\n")
        await community.unload()
        endpoint.close()
        return 0 if n_peers >= 1 else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
