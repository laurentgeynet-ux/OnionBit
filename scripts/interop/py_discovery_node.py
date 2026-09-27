#!/usr/bin/env python3
"""Noeud d'interop discovery pyipv8 (etape 11) : introductions
new-style (234 -> 233) et punctures (250/232 -> 249/231) contre le
noeud Rust `discovery_interop_node`.

Chaque handler pyipv8 passe par `lazy_wrapper` : decode payload +
verification de signature Ed25519 — un log `PY_*_DECODED` prouve donc
les deux. Le Python pilote aussi sa direction : requete 234 (attend
233), puis requetes de puncture 232 et 250 (attend 231 et 249).

Usage : py_discovery_node.py --port N --target 127.0.0.1:P
        --duration S --log FILE
Prerequis : PYTHONPATH=<pyipv8> + venv interop (interop_discovery.ps1).
"""

from __future__ import annotations

import argparse
import asyncio
import logging
import sys
import time

from ipv8.community import CommunitySettings
from ipv8.keyvault.crypto import default_eccrypto
from ipv8.messaging.interfaces.udp.endpoint import UDPEndpoint, UDPv4Address
from ipv8.messaging.payload import (
    NewIntroductionRequestPayload,
    NewIntroductionResponsePayload,
    NewPuncturePayload,
    NewPunctureRequestPayload,
    PuncturePayload,
    PunctureRequestPayload,
)
from ipv8.lazy_community import lazy_wrapper
from ipv8.messaging.payload_headers import GlobalTimeDistributionPayload
from ipv8.messaging.payload import (
    IntroductionRequestPayload,
    IntroductionResponsePayload,
)
from ipv8.peer import Peer
from ipv8.peerdiscovery.community import DiscoveryCommunity
from ipv8.peerdiscovery.network import Network

logging.basicConfig(level=logging.WARNING, stream=sys.stderr)


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


class LoggingDiscoveryCommunity(DiscoveryCommunity):
    """DiscoveryCommunity qui logue les payloads decodes (preuve de
    decode + signature cote Python) et denombre par type."""

    def __init__(self, settings):
        super().__init__(settings)
        self.counts = {
            "new_intro_req": 0,
            "old_intro_req": 0,
            "new_intro_resp": 0,
            "old_intro_resp": 0,
            "new_puncture_req": 0,
            "old_puncture_req": 0,
            "new_puncture": 0,
            "old_puncture": 0,
        }

    def introduction_request_callback(self, peer, dist, payload) -> None:
        cls = type(payload).__name__
        if isinstance(payload, NewIntroductionRequestPayload):
            self.counts["new_intro_req"] += 1
        else:
            self.counts["old_intro_req"] += 1
        sys.stderr.write(f"PY_RECV_INTRO_REQ|{cls}|id={payload.identifier}\n")

    def introduction_response_callback(self, peer, dist, payload) -> None:
        cls = type(payload).__name__
        if isinstance(payload, NewIntroductionResponsePayload):
            self.counts["new_intro_resp"] += 1
        else:
            self.counts["old_intro_resp"] += 1
        sys.stderr.write(
            f"PY_RECV_INTRO_RESP|{cls}|id={payload.identifier}|"
            f"dst={payload.destination_address}|"
            f"intro_new_style={payload.intro_supports_new_style}\n"
        )

    def on_puncture_request(self, source_address, dist, payload, new_style=False) -> None:
        if isinstance(payload, NewPunctureRequestPayload):
            self.counts["new_puncture_req"] += 1
        else:
            self.counts["old_puncture_req"] += 1
        sys.stderr.write(
            f"PY_RECV_PUNCTURE_REQ|{type(payload).__name__}|id={payload.identifier}|"
            f"lan={payload.lan_walker_address}|wan={payload.wan_walker_address}\n"
        )
        super().on_puncture_request(source_address, dist, payload, new_style)

    @lazy_wrapper(GlobalTimeDistributionPayload, PuncturePayload)
    def on_puncture(self, peer, dist, payload) -> None:
        self.counts["old_puncture"] += 1
        sys.stderr.write(
            f"PY_OLD_PUNCTURE_DECODED|id={payload.identifier}|"
            f"lan={payload.source_lan_address}|wan={payload.source_wan_address}\n"
        )


    @lazy_wrapper(GlobalTimeDistributionPayload, NewPuncturePayload)
    def on_new_puncture(self, peer, dist, payload) -> None:
        self.counts["new_puncture"] += 1
        sys.stderr.write(
            f"PY_NEW_PUNCTURE_DECODED|id={payload.identifier}|"
            f"lan={payload.source_lan_address}|wan={payload.source_wan_address}\n"
        )



async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--target", required=True, help="host:port du noeud Rust")
    parser.add_argument("--duration", type=float, default=12.0)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()

    host, port_s = args.target.rsplit(":", 1)
    target = UDPv4Address(host, int(port_s))

    with open(args.log, "w", encoding="ascii") as log:
        endpoint = RecordingEndpoint(args.port, log)
        await endpoint.open()

        key = default_eccrypto.generate_key("curve25519")
        my_addr = UDPv4Address("127.0.0.1", args.port)
        my_peer = Peer(key, my_addr)
        network = Network()
        settings = CommunitySettings()
        settings.my_peer = my_peer
        settings.endpoint = endpoint
        settings.network = network
        community = LoggingDiscoveryCommunity(settings)

        # Direction Python -> Rust.
        async def drive() -> None:
            await asyncio.sleep(0.5)
            # 1. introduction-request new-style (234) -> attend 233.
            # Retries : le premier envoi peut partir avant le bind du
            # socket Rust (course de demarrage — UDP ne previent pas).
            for _ in range(10):
                if community.counts["new_intro_resp"] >= 1:
                    break
                endpoint.send(target, community.create_introduction_request(target, new_style=True))
                await asyncio.sleep(0.5)
            await asyncio.sleep(0.5)
            # 2. puncture-request new-style (232) -> attend 231.
            endpoint.send(
                target,
                community.create_puncture_request(my_addr, my_addr, 0xCA01, new_style=True),
            )
            await asyncio.sleep(1.0)
            # 3. puncture-request old-style (250) -> attend 249.
            endpoint.send(
                target,
                community.create_puncture_request(my_addr, my_addr, 0xCA02, new_style=False),
            )

        driver = asyncio.ensure_future(drive())
        await asyncio.sleep(args.duration)
        driver.cancel()

        c = community.counts
        sys.stderr.write(
            "PY_SUMMARY|new_intro_req={new_intro_req}|old_intro_req={old_intro_req}|"
            "new_intro_resp={new_intro_resp}|old_intro_resp={old_intro_resp}|"
            "new_punct_req={new_puncture_req}|old_punct_req={old_puncture_req}|"
            "new_punct={new_puncture}|old_punct={old_puncture}\n".format(**c)
        )
        # Flags de la direction Python -> Rust :
        # - notre 234 a recu un 233 decode (signature Rust verifiee)
        print_flag("PY_NEW_INTRO_RESP_OK", c["new_intro_resp"] >= 1)
        # - nos puncture-requests ont recu leurs reponses decodees
        print_flag("PY_NEW_PUNCTURE_OK", c["new_puncture"] >= 1)
        print_flag("PY_OLD_PUNCTURE_OK", c["old_puncture"] >= 1)
        # Direction Rust -> Python (decode+signature des requetes Rust) :
        print_flag("PY_RECV_NEW_INTRO_REQ", c["new_intro_req"] >= 1)
        print_flag("PY_RECV_NEW_PUNCTURE_REQ", c["new_puncture_req"] >= 1)
        print_flag("PY_RECV_OLD_PUNCTURE_REQ", c["old_puncture_req"] >= 1)

        await community.unload()
        endpoint.close()
    return 0


def print_flag(name: str, ok: bool) -> None:
    sys.stderr.write(f"{name} : {'OK' if ok else 'FAIL'}\n")


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
