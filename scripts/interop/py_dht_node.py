#!/usr/bin/env python3
# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

"""Noeud d'interop DHT (vrai DHTCommunity pyipv8) pour l'etape 10 :
prouve `find`/`store`/`find` aller-retour avec le DhtCommunity Rust,
acceptation + refus des tokens anti-spoofing, y compris apres rotation
des secrets cote Python.

Choreographie (delais fixes, loopback — cf. interop_dht.ps1) :
  t=1  on_node_discovered(rust) -> ping automatique vers Rust
  t=3  find_values(K_PY)     -> PY_FIND_OK (token Rust obtenu)
  t=6  store_value(K_PY, v)  -> PY_STORE_OK (token accepte par Rust)
  t=9  find_values(K_PY)     -> PY_VERIFY_OK (valeur relue chez Rust)
  t=11 store_request token bidon -> puis find -> PY_BADTOKEN_REJECTED
  t=20 find_values(K_RUST)   -> PY_READS_RUST_OK (valeur Rust relue)
Rotation : le 1er store-request accepte (venant de Rust) declenche
token_maintenance() x2 -> le secret ayant emis le token Rust est
evince -> TOKENS_ROTATED.

Usage : py_dht_node.py --port P --rust-addr 127.0.0.1:Q
        --rust-key HEX --key-file FILE --duration S
Necessite : PYTHONPATH=<...>/pyipv8 + venv interop.
"""

from __future__ import annotations

import argparse
import asyncio
import logging
import sys
import time
from binascii import unhexlify

logging.basicConfig(level=logging.WARNING, stream=sys.stderr)

from ipv8.community import CommunitySettings
from ipv8.dht.community import DHTCommunity
from ipv8.dht.payload import StoreRequestPayload
from ipv8.dht.routing import Node
from ipv8.keyvault.crypto import default_eccrypto
from ipv8.messaging.interfaces.udp.endpoint import UDPEndpoint, UDPv4Address
from ipv8.peer import Peer
from ipv8.peerdiscovery.network import Network

# Cibles fixes (deterministes) pour le banc.
K_PY = b"py" + b"\x00" * 18
K_BAD = b"bd" + b"\x00" * 18
K_RUST = b"rs" + b"\x00" * 18
PY_VALUE = b"python-value"
RUST_VALUE = b"rust-value"


def log(msg: str) -> None:
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--rust-addr", required=True)
    parser.add_argument("--rust-key", required=True, help="pubkey hex du noeud Rust")
    parser.add_argument("--key-file", required=True)
    parser.add_argument("--duration", type=float, default=28.0)
    args = parser.parse_args()

    rust_host, rust_port_s = args.rust_addr.rsplit(":", 1)
    rust_addr = UDPv4Address(rust_host, int(rust_port_s))
    rust_key_bin = unhexlify(args.rust_key)

    endpoint = UDPEndpoint(port=args.port, ip="127.0.0.1")
    await endpoint.open()

    key = default_eccrypto.generate_key("curve25519")
    with open(args.key_file, "w", encoding="ascii") as f:
        f.write(key.pub().key_to_bin().hex())

    my_addr = UDPv4Address("127.0.0.1", args.port)
    my_peer = Peer(key, my_addr)
    settings = CommunitySettings()
    settings.my_peer = my_peer
    settings.endpoint = endpoint
    settings.network = Network()
    community = DHTCommunity(settings)
    community.my_estimated_wan = my_addr
    community.my_estimated_lan = my_addr

    # Rotation des secrets des le premier store-request accepte
    # (venant de Rust) : le token qu'il presentait est evince.
    orig_check_token = community.check_token

    def check_token_and_rotate(node, token):
        ok = orig_check_token(node, token)
        if ok and not check_token_and_rotate.rotated:
            check_token_and_rotate.rotated = True
            community.token_maintenance()
            community.token_maintenance()
            log("TOKENS_ROTATED")
        return ok

    check_token_and_rotate.rotated = False
    community.check_token = check_token_and_rotate

    t0 = time.time()

    async def at(t, coro):
        await asyncio.sleep(max(0.0, t0 + t - time.time()))
        return await coro

    async def python_to_rust():
        # t=1 : apprend le noeud Rust (declenche un ping vers lui).
        await at(1.0, asyncio.sleep(0))
        community.on_node_discovered(rust_key_bin, rust_addr)

        # t=3 : find_values -> le Rust renvoie un token.
        vals = await at(3.0, community.find_values(K_PY))
        log(f"PY_FIND_OK|{len(vals)}")

        # t=6 : store_value **signe** -> Rust doit accepter le token,
        # stocker et verifier la signature Python a la relecture.
        stored = await at(6.0, community.store_value(K_PY, PY_VALUE, sign=True))
        log("PY_STORE_OK" if stored else "PY_STORE_FAIL")

        # t=9 : relecture — la valeur doit revenir du noeud Rust.
        # `find_values` renvoie des tuples `(data, public_key)`.
        vals2 = await at(9.0, community.find_values(K_PY))
        # `v[1]` non None = signature Python verifiee par le decodeur.
        found = any(v[0] == PY_VALUE and v[1] is not None for v in vals2)
        log("PY_VERIFY_OK" if found else f"PY_VERIFY_FAIL|{vals2!r}")

        # t=11 : store-request avec un token bidon -> Rust doit le
        # rejeter silencieusement ; la relecture prouve le refus.
        await at(11.0, asyncio.sleep(0))
        node = Node(rust_key_bin, rust_addr)
        bogus = StoreRequestPayload(424242, b"\x00" * 20, K_BAD, [b"forged"])
        community.ez_send(node, bogus)
        vals3 = await at(14.0, community.find_values(K_BAD))
        if any(v[0] == b"forged" for v in vals3):
            log("PY_BADTOKEN_ACCEPTED")  # echec : Rust a stocke malgre le mauvais token
        else:
            log("PY_BADTOKEN_REJECTED")

        # t=20 : la valeur stockee par Rust doit etre lisible.
        vals4 = await at(20.0, community.find_values(K_RUST))
        # `v[1]` non None = signature Rust verifiee par Python.
        found4 = any(v[0] == RUST_VALUE and v[1] is not None for v in vals4)
        log("PY_READS_RUST_OK" if found4 else f"PY_READS_RUST_FAIL|{vals4!r}")

        log("PY_DONE")

    async def run():
        try:
            await asyncio.wait_for(python_to_rust(), timeout=args.duration)
        except asyncio.TimeoutError:
            log("PY_TIMEOUT")

    await run()
    await asyncio.sleep(1.0)
    await community.unload()
    endpoint.close()
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
