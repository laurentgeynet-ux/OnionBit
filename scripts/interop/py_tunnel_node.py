#!/usr/bin/env python3
"""Noeud d'interop tunnels : vrai TunnelCommunity pyipv8 (relais + exit
IPv8) plus un echo UDP "compatible uTP" comme destination de sortie.

Le noeud ecrit sa cle publique (`key_to_bin()` hex) et son port dans
`--keyfile` pour que le noeud Rust enregistre le pair puis cree un
circuit. Chaque datagramme brut (RX/TX) est journalise en hex.

Usage :
    py_tunnel_node.py --port P --echo-port E --keyfile F --log L --duration S
Prerequis : PYTHONPATH=<...>/pyipv8 et le venv interop.
"""

from __future__ import annotations

import argparse
import asyncio
import logging
import sys
import time

from ipv8.keyvault.crypto import default_eccrypto
from ipv8.messaging.anonymization.community import TunnelCommunity, TunnelSettings
from ipv8.messaging.anonymization.tunnel import (
    PEER_FLAG_EXIT_BT,
    PEER_FLAG_EXIT_IPV8,
    PEER_FLAG_RELAY,
)
from ipv8.messaging.interfaces.udp.endpoint import UDPEndpoint, UDPv4Address
from ipv8.peer import Peer
from ipv8.peerdiscovery.network import Network

logging.basicConfig(level=logging.WARNING, stream=sys.stderr)
logging.getLogger("TunnelCommunity").setLevel(logging.INFO)
logging.getLogger("TunnelExitSocket").setLevel(logging.INFO)


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


class EchoProtocol(asyncio.DatagramProtocol):
    """Echo UDP : renvoie le datagramme tel quel a l'expediteur."""

    def __init__(self, log):
        self._log = log

    def connection_made(self, transport):
        self._transport = transport

    def datagram_received(self, data, addr):
        self._log.write(f"ECHO|{addr[0]}:{addr[1]}|{data.hex()}\n")
        self._log.flush()
        self._transport.sendto(data, addr)


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--echo-port", type=int, required=True)
    parser.add_argument("--keyfile", required=True)
    parser.add_argument("--log", required=True)
    parser.add_argument("--duration", type=float, default=10.0)
    args = parser.parse_args()

    with open(args.log, "w", encoding="ascii") as log:
        endpoint = RecordingEndpoint(args.port, log)
        await endpoint.open()

        key = default_eccrypto.generate_key("curve25519")
        my_peer = Peer(key, UDPv4Address("127.0.0.1", args.port))
        settings = TunnelSettings()
        settings.my_peer = my_peer
        settings.endpoint = endpoint
        settings.network = Network()
        # EXIT_BT necessaire : le DataChecker n'autorise du trafic
        # "forme uTP" qu'avec ce flag (IPv8 seul rejette).
        settings.peer_flags = {PEER_FLAG_RELAY, PEER_FLAG_EXIT_IPV8, PEER_FLAG_EXIT_BT}
        community = TunnelCommunity(settings)

        # Exporte la cle publique + port pour le noeud Rust.
        with open(args.keyfile, "w", encoding="ascii") as kf:
            kf.write(f"{key.pub().key_to_bin().hex()} {args.port}\n")

        loop = asyncio.get_running_loop()
        await loop.create_datagram_endpoint(
            lambda: EchoProtocol(log),
            local_addr=("127.0.0.1", args.echo_port),
        )

        sys.stderr.write(
            f"python tunnel node pret port={args.port} echo={args.echo_port}\n"
        )

        deadline = time.time() + args.duration
        # Dump les cles de session des exit sockets (debug interop).
        while time.time() < deadline:
            for cid, es in community.exit_sockets.items():
                k = es.hop.keys
                log.write(
                    f"KEYS|{cid}|{k.key_forward.hex()}|{k.key_backward.hex()}"
                    f"|{k.salt_forward.hex()}|{k.salt_backward.hex()}\n"
                )
                log.flush()
            await asyncio.sleep(0.5)

        n_exit = len(community.exit_sockets)
        n_relay = len(community.relay_from_to)
        sys.stderr.write(
            f"python tunnel : exit_sockets={n_exit} relays={n_relay}\n"
        )
        await community.unload()
        endpoint.close()
        return 0 if n_exit >= 1 or n_relay >= 2 else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
