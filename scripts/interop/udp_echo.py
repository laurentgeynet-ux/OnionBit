# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

"""Echo UDP minimal (destination de sortie pour les tests d'interop).

Usage : python udp_echo.py --port P
"""
import argparse
import socket


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    args = ap.parse_args()
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("127.0.0.1", args.port))
    print(f"UDP echo on 127.0.0.1:{args.port}", flush=True)
    while True:
        data, addr = s.recvfrom(65535)
        s.sendto(data, addr)


if __name__ == "__main__":
    main()
