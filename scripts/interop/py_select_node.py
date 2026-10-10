#!/usr/bin/env python3
# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

"""Noeud d'interop remote-select/health/version (etape 95) : vraie
`tribler.core.content_discovery.community.ContentDiscoveryCommunity`
sur un vrai `MetadataStore` pony en memoire, contre le noeud Rust
`select_interop_node`.

Chaque handler passe par `lazy_wrapper` : decode payload +
verification de signature Ed25519. Les marqueurs `PY_*` prouvent
donc les deux.

Directions testees :
  - Python -> Rust : `send_remote_select` (reponses decodees et
    parsees par `process_compressed_mdblob` — preuve mdblob+LZ4),
    `VersionRequest` (reponse decodee), `HealthRequest` (reponse
    decodee).
  - Rust -> Python : `on_remote_select` reel contre le
    MetadataStore ensemence (chunks LZ4 servis), `VersionRequest`,
    `HealthRequest`.

Usage : py_select_node.py --port N --target 127.0.0.1:P
        --duration S --log FILE
Prerequis : PYTHONPATH=tribler/src;pyipv8 + deps (pony, lz4) —
            voir scripts/interop_select.ps1.
"""

from __future__ import annotations

import argparse
import asyncio
import os
import sys
import tempfile
from binascii import hexlify
from datetime import datetime

from ipv8.community import CommunitySettings
from ipv8.keyvault.crypto import default_eccrypto
from ipv8.messaging.interfaces.udp.endpoint import UDPEndpoint, UDPv4Address
from ipv8.peer import Peer
from ipv8.peerdiscovery.network import Network
from pony.orm import db_session

from ipv8.lazy_community import lazy_wrapper

from tribler.core.content_discovery.community import ContentDiscoveryCommunity, ContentDiscoverySettings
from tribler.core.content_discovery.payload import (
    HealthPayload,
    HealthRequestPayload,
    RemoteSelectPayload,
    SelectResponsePayload,
    VersionRequest,
    VersionResponse,
)
from tribler.core.database.store import MetadataStore, ObjState, ProcessingResult
from tribler.core.torrent_checker.healthdataclasses import HealthInfo

logging_level = "WARNING"
import logging

logging.basicConfig(level=getattr(logging, logging_level), stream=sys.stderr)

# Infohashes ensemencees (deterministes — le test Rust peut filtrer).
IH_A = b"\xaa" * 20
IH_B = b"\xbb" * 20


class LoggingContentDiscovery(ContentDiscoveryCommunity):
    """ContentDiscoveryCommunity qui logue chaque evenement filaire
    decode (preuve de decode+signature cote Python)."""

    def __init__(self, settings):
        super().__init__(settings)
        self.counts = {
            "select_req": 0,
            "select_resp_new": 0,
            "version_req": 0,
            "version_resp": 0,
            "health_req": 0,
            "health_resp": 0,
        }
        self.served_chunks = 0

    @lazy_wrapper(RemoteSelectPayload)
    async def on_remote_select(self, peer, request_payload: RemoteSelectPayload):
        self.counts["select_req"] += 1
        sys.stderr.write(f"PY_RECV_SELECT_REQ|id={request_payload.id}|json={request_payload.json!r}\n")
        await super().on_remote_select.__wrapped__(self, peer, request_payload)

    def send_db_results(self, peer, request_payload_id, db_results):
        super().send_db_results(peer, request_payload_id, db_results)
        sys.stderr.write(f"PY_SENT_RESULTS|id={request_payload_id}|entries={len(db_results)}\n")

    @lazy_wrapper(SelectResponsePayload)
    async def on_remote_select_response(self, peer, response_payload: SelectResponsePayload):
        results = await super().on_remote_select_response.__wrapped__(self, peer, response_payload)
        if results:
            new = sum(1 for r in results if r.obj_state == ObjState.NEW_OBJECT)
            self.counts["select_resp_new"] += new
            sys.stderr.write(
                f"PY_SELECT_RESP|id={response_payload.id}|results={len(results)}"
                f"|new={new}\n"
            )
        else:
            sys.stderr.write(f"PY_SELECT_RESP|id={response_payload.id}|results=0\n")
        return results

    @lazy_wrapper(VersionRequest)
    async def on_version_request(self, peer, payload):
        self.counts["version_req"] += 1
        sys.stderr.write("PY_RECV_VERSION_REQ\n")
        await super().on_version_request.__wrapped__(self, peer, payload)

    @lazy_wrapper(VersionResponse)
    async def on_version_response(self, peer, payload):
        self.counts["version_resp"] += 1
        sys.stderr.write(f"PY_VERSION_RESP|version={payload.version}|platform={payload.platform}\n")

    @lazy_wrapper(HealthRequestPayload)
    async def on_health_request(self, peer, payload: HealthRequestPayload):
        self.counts["health_req"] += 1
        sys.stderr.write(f"PY_RECV_HEALTH_REQ|type={payload.request_type}\n")
        await super().on_health_request.__wrapped__(self, peer, payload)

    @lazy_wrapper(HealthPayload)
    async def on_health(self, peer, payload: HealthPayload):
        self.counts["health_resp"] += 1
        infos = payload.get_health_info()
        sys.stderr.write(
            f"PY_HEALTH_RESP|type={payload.response_type}|n={len(infos)}"
            + "".join(
                f"|{hexlify(h.infohash).decode()[:8]}:{h.seeders}/{h.leechers}" for h in infos
            )
            + "\n"
        )
        await super().on_health.__wrapped__(self, peer, payload)


def seed(mds: MetadataStore, my_key) -> None:
    """Deux entrees FFA (non signees) + sante — servies par le
    vrai `on_remote_select`/`get_entries_threaded`."""
    with db_session:
        a = mds.TorrentMetadata.add_ffa_from_dict(
            {
                "infohash": IH_A,
                "title": "interop-select-alpha",
                "tags": "video",
                "size": 111111,
                "torrent_date": datetime(2026, 1, 1),
            }
        )
        b = mds.TorrentMetadata.add_ffa_from_dict(
            {
                "infohash": IH_B,
                "title": "interop-select-beta",
                "tags": "audio",
                "size": 222222,
                "torrent_date": datetime(2026, 1, 2),
            }
        )
        for t, (s, l) in ((a, (7, 3)), (b, (1, 0))):
            if t is not None and t.health is not None:
                t.health.seeders = s
                t.health.leechers = l
                t.health.last_check = int(datetime.now().timestamp())


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--target", required=True, help="host:port du noeud Rust")
    parser.add_argument("--duration", type=float, default=14.0)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()

    host, port_s = args.target.rsplit(":", 1)
    target = UDPv4Address(host, int(port_s))

    with open(args.log, "w", encoding="ascii") as log:
        endpoint = UDPEndpoint(port=args.port, ip="127.0.0.1")
        await endpoint.open()

        key = default_eccrypto.generate_key("curve25519")
        my_peer = Peer(key, UDPv4Address("127.0.0.1", args.port))
        network = Network()
        # Fichier temporaire, pas ":memory:" : `run_threaded`
        # deponde dans un executor — chaque connexion sqlite
        # ":memory:" est une base distincte (les requetes en
        # thread voyaient une base vide, "no such table").
        db_fd, db_path = tempfile.mkstemp(suffix=".db")
        os.close(db_fd)
        os.unlink(db_path)  # `create_db` exige que le fichier n'existe pas
        mds = MetadataStore(db_path, key)
        seed(mds, key)

        # Stub du TorrentChecker : `get_alive_checked_torrents`
        # lit `torrents_checked` — sans lui les HealthPayload
        # servis sont vides.
        now = int(datetime.now().timestamp())

        class StubChecker:
            torrents_checked = {
                IH_A: HealthInfo(IH_A, 7, 3, now),
                IH_B: HealthInfo(IH_B, 1, 0, now),
            }

        settings = ContentDiscoverySettings()
        settings.my_peer = my_peer
        settings.endpoint = endpoint
        settings.network = network
        settings.metadata_store = mds
        settings.torrent_checker = StubChecker()
        settings.notifier = None
        settings.random_torrent_interval = 3600  # pas de gossip parasite
        community = LoggingContentDiscovery(settings)

        async def drive() -> None:
            # Introduction new-style jusqu'a ce que le pair Rust soit
            # verifie (retries : course de demarrage UDP).
            for _ in range(20):
                if community.network.get_verified_by_address(target):
                    break
                endpoint.send(target, community.create_introduction_request(target, new_style=True))
                await asyncio.sleep(0.5)
            peer = community.network.get_verified_by_address(target)
            if peer is None:
                sys.stderr.write("PY_PEER_MISSING\n")
                return
            sys.stderr.write("PY_PEER_OK\n")
            await asyncio.sleep(0.5)
            # Python -> Rust : select plein (les 2 entrees FFA).
            community.send_remote_select(peer, first=1, last=50)
            # VersionRequest + HealthRequest vers Rust.
            community.ez_send(peer, VersionRequest())
            community.ez_send(peer, HealthRequestPayload(2))  # RANDOM

        driver = asyncio.ensure_future(drive())
        await asyncio.sleep(args.duration)
        driver.cancel()

        c = community.counts
        sys.stderr.write(
            "PY_SUMMARY|select_req={select_req}|select_resp_new={select_resp_new}|"
            "version_req={version_req}|version_resp={version_resp}|"
            "health_req={health_req}|health_resp={health_resp}\n".format(**c)
        )
        print_flag("PY_SELECT_RESP_OK", c["select_resp_new"] >= 1)
        print_flag("PY_RECV_SELECT_REQ", c["select_req"] >= 1)
        print_flag("PY_VERSION_RESP", c["version_resp"] >= 1)
        print_flag("PY_RECV_VERSION_REQ", c["version_req"] >= 1)
        print_flag("PY_RECV_HEALTH_REQ", c["health_req"] >= 1)

        await community.unload()
        endpoint.close()
        # Le fichier peut rester verrouille par la connexion pony.
        try:
            os.unlink(db_path)
        except OSError:
            pass
    return 0


def print_flag(name: str, ok: bool) -> None:
    sys.stderr.write(f"{name} : {'OK' if ok else 'FAIL'}\n")


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
