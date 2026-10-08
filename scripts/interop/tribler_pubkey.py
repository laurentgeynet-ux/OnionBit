#!/usr/bin/env python3
# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

"""Extrait la cle publique IPv8 (`key_to_bin()` hex) d'un fichier de
cle privee pyipv8 (`LibNaCLSK:…`, ex. `ec_multichain.pem` de Tribler).

Usage : tribler_pubkey.py <pem_path> — ecrit "<pubkey_hex>" sur stdout.
Prerequis : PYTHONPATH=<...>/pyipv8 et le venv interop.
"""

import sys

from ipv8.keyvault.crypto import default_eccrypto


def main() -> int:
    sk = default_eccrypto.key_from_private_bin(open(sys.argv[1], "rb").read())
    print(sk.pub().key_to_bin().hex())
    return 0


if __name__ == "__main__":
    sys.exit(main())
