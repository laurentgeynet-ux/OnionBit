#!/usr/bin/env python3
# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

"""Verifie un journal hex de paquets IPv8 (`DIR|addr|hex` par ligne) en
le decodant avec le vrai pyipv8 : structure + signature Ed25519 via
`default_eccrypto`.

Echoue (exit 1) si :
- un paquet signe a une signature invalide ou une structure illisible ;
- un `msg_id` n'est pas dans la whitelist `--allow-msg-id` (quand
  fournie) — un paquet inattendu dans la capture EST un echec ;
- la capture filtree est vide.

Compteurs separes par `msg_id` et par type (signe / non-signe) pour
qu'un flux parasite ne soit pas noye dans un total global.

Layouts conformes a pyipv8 :
- signe avec `dist` (intro/puncture : 246, 245, 234, 233, 249, 231) :
  `prefix|msg_id|varlenH(pubkey)|global_time(Q)|payload|sig`
- signe sans `dist` (ez_send : DHT, content-discovery, destroy…) :
  `prefix|msg_id|varlenH(pubkey)|payload|sig`
- non signe (puncture-request 250/232) : `prefix|msg_id|global_time(Q)
  |payload` — pas de signature.

Usage : verify_packets.py FILE [--community-id HEX] [--allow-msg-id LISTE]
"""

from __future__ import annotations

import argparse
import collections
import struct
import sys

from ipv8.keyvault.crypto import default_eccrypto

SIGNATURE_LEN = 64
PREFIX_LEN = 22

# Messages signes dont le payload est precede de `global_time` (Q)
# — `create_introduction_*`/`create_puncture*` pyipv8. Tous les autres
# messages signes (ez_send : DHT, content-discovery, tunnel DESTROY)
# n'ont PAS ce champ sur le fil.
DIST_MSG_IDS = frozenset({246, 245, 234, 233, 249, 231})


def verify_packet(data: bytes) -> tuple[bool, str, int | None, str]:
    """Retourne (ok, detail, msg_id, kind) ; kind = signe|non-signe|invalide."""
    msg_id = data[PREFIX_LEN] if len(data) > PREFIX_LEN else None
    if len(data) < PREFIX_LEN + 1 + 2 + SIGNATURE_LEN:
        # Paquet non signe (puncture-request etc.) :
        # prefix|msg_id|global_time(Q)|payload — pas de signature.
        if msg_id is not None and len(data) >= PREFIX_LEN + 1 + 8:
            return True, "non-signe", msg_id, "non-signe"
        return False, "trop court", msg_id, "invalide"
    # varlenH pubkey
    klen = struct.unpack_from(">H", data, PREFIX_LEN + 1)[0]
    koff = PREFIX_LEN + 3
    pubkey_bin = data[koff : koff + klen]
    signed_part = data[: len(data) - SIGNATURE_LEN]
    signature = data[len(data) - SIGNATURE_LEN :]
    try:
        pk = default_eccrypto.key_from_public_bin(pubkey_bin)
        ok = pk.verify(signature, signed_part)
    except Exception as exc:  # noqa: BLE001
        return False, f"cle/signature illisible: {exc}", msg_id, "invalide"
    if not ok:
        return False, "signature INVALIDE", msg_id, "invalide"
    # global_time n'est present que pour les messages `dist` — le
    # decode ici pour l'affichage, sans incidence sur la signature.
    detail = "signe"
    if msg_id in DIST_MSG_IDS:
        gt = struct.unpack_from(">Q", data, koff + klen)[0]
        detail = f"signe gt={gt}"
    mid = __import__("hashlib").sha1(pubkey_bin).hexdigest()[:8]
    return True, f"{detail} mid={mid}…", msg_id, "signe"


def parse_allow(raw: str | None) -> frozenset[int] | None:
    if not raw:
        return None
    return frozenset(int(x, 0) for x in raw.split(",") if x.strip())


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("--community-id", default=None)
    ap.add_argument(
        "--allow-msg-id",
        default=None,
        help="Liste CSV de msg_id autorises (ex. '246,245,234,233'). "
        "Tout autre msg_id est compte comme echec.",
    )
    args = ap.parse_args()

    cid = bytes.fromhex(args.community_id) if args.community_id else None
    allow = parse_allow(args.allow_msg_id)
    total = ok = 0
    stats: collections.Counter[tuple[int, str, str]] = collections.Counter()
    with open(args.file, encoding="ascii") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            _dir, _addr, hexdata = line.split("|", 2)
            data = bytes.fromhex(hexdata)
            if cid and not data[2:22] == cid:
                continue
            total += 1
            good, msg, msg_id, kind = verify_packet(data)
            if allow is not None and msg_id not in allow:
                good = False
                msg = f"{msg} — msg_id HORS WHITELIST"
                kind = "invalide"
            ok += good
            stats[(msg_id if msg_id is not None else -1, kind, "OK" if good else "BAD")] += 1
            status = "OK " if good else "BAD"
            sys.stderr.write(f"{status} {_dir} {_addr} msg_id={msg_id} {msg}\n")
    for (mid, kind, st), n in sorted(stats.items()):
        sys.stderr.write(f"  msg_id={mid:<4} {kind:<10} {st:<3} x{n}\n")
    sys.stderr.write(f"{ok}/{total} paquets valides\n")
    return 0 if total > 0 and ok == total else 1


if __name__ == "__main__":
    sys.exit(main())
