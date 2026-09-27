#!/usr/bin/env python3
"""Verifie un journal hex de paquets IPv8 (`DIR|addr|hex` par ligne) en
le decodant avec le vrai pyipv8 : structure + signature Ed25519 via
`default_eccrypto`. Echoue (exit 1) si un paquet signe est invalide.

Usage : verify_packets.py FILE [--community-id HEX]
"""

from __future__ import annotations

import argparse
import struct
import sys

from ipv8.keyvault.crypto import default_eccrypto

SIGNATURE_LEN = 64
PREFIX_LEN = 22


def verify_packet(data: bytes) -> tuple[bool, str]:
    """Decode `prefix|msg_id|varlenH(pubkey)|global_time|payload|sig`."""
    if len(data) < PREFIX_LEN + 1 + 2 + 8 + SIGNATURE_LEN:
        # Peut-etre un paquet non signe (puncture-request etc.) :
        # prefix|msg_id|global_time(Q)|payload — pas de signature.
        if len(data) >= PREFIX_LEN + 1 + 8:
            msg_id = data[PREFIX_LEN]
            return True, f"non-signe msg_id={msg_id}"
        return False, "trop court"
    msg_id = data[PREFIX_LEN]
    # varlenH pubkey
    klen = struct.unpack_from(">H", data, PREFIX_LEN + 1)[0]
    koff = PREFIX_LEN + 3
    pubkey_bin = data[koff : koff + klen]
    # global_time
    gt = struct.unpack_from(">Q", data, koff + klen)[0]
    signed_part = data[: len(data) - SIGNATURE_LEN]
    signature = data[len(data) - SIGNATURE_LEN :]
    try:
        pk = default_eccrypto.key_from_public_bin(pubkey_bin)
        ok = pk.verify(signature, signed_part)
    except Exception as exc:  # noqa: BLE001
        return False, f"cle/signature illisible: {exc}"
    if not ok:
        return False, f"signature INVALIDE msg_id={msg_id}"
    mid = __import__("hashlib").sha1(pubkey_bin).hexdigest()[:8]
    return True, f"signe msg_id={msg_id} gt={gt} mid={mid}…"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("--community-id", default=None)
    args = ap.parse_args()

    cid = bytes.fromhex(args.community_id) if args.community_id else None
    total = ok = 0
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
            good, msg = verify_packet(data)
            ok += good
            status = "OK " if good else "BAD"
            sys.stderr.write(f"{status} {_dir} {_addr} {msg}\n")
    sys.stderr.write(f"{ok}/{total} paquets valides\n")
    return 0 if total > 0 and ok == total else 1


if __name__ == "__main__":
    sys.exit(main())
