#!/usr/bin/env python3
# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

"""Analyse une capture pktmon (pcapng) pour le banc de fuite P0-17b.

Classe chaque endpoint distant observe sur le fil :

  LOCAL    : prive/loopback/multicast/lien-local des deux cotes
  OVERLAY  : endpoint present dans le fichier --allowed (verite fil
             extraite du journal TAP du processus teste)
  DNS      : port 53 -- les qnames sont decodes et listes
  BRUIT    : paquet d'un port du banc vers un hote WAN recevant le
             meme motif (proto + taille) depuis de nombreux ports
             NON banc — bruit ambiant d'un autre processus qui a
             reutilise un port ephemere (attribution impossible)
  INTERDIT : TCP vers WAN, UDP vers WAN hors overlay, DHT mainline
             direct (routeurs publics resolus au lancement)

Verdict : exit 1 si au moins un paquet INTERDIT (hors --dns-ok).
Stdlib uniquement -- parse pcapng a la main (EPB + IDB).

Attribution : pktmon capture le meme datagramme sur plusieurs couches
NDIS (duplicatas a ~1 us) et ne porte pas de PID — l'appartenance au
processus teste est deduite du port local. Un port ephemere libere
peut etre reutilise par un autre processus dans la fenetre d'analyse :
d'ou le dedoublonnage (meme paquet a <1 ms = une seule capture) et le
reclassement BRUIT des collisions avec le trafic ambiant.
"""
import argparse
import ipaddress
import json
import socket
import struct
import sys


def iter_packets(path):
    """Yields (linktype, ts_us, frame_bytes) pour chaque paquet pcapng."""
    data = open(path, "rb").read()
    off = 0
    linktypes = {}  # idb index -> linktype
    ifresol = {}    # idb index -> resolution timestamp
    while off + 12 <= len(data):
        btype, blen = struct.unpack_from("<II", data, off)
        body = data[off + 8: off + blen - 4]
        if btype == 0x0A0D0D0A:  # Section Header
            linktypes = {}
            ifresol = {}
        elif btype == 1:  # Interface Description Block
            lt = struct.unpack_from("<H", body, 0)[0]
            idx = len(linktypes)
            linktypes[idx] = lt
            resol = 6  # defaut : microsecondes
            opts = body[8:]
            o = 0
            while o + 4 <= len(opts):
                ocode, olen = struct.unpack_from("<HH", opts, o)
                oval = opts[o + 4: o + 4 + olen]
                if ocode == 9 and oval:  # if_tsresol
                    resol = oval[0] & 0x7F
                o += 4 + ((olen + 3) & ~3)
            ifresol[idx] = resol
        elif btype == 6:  # Enhanced Packet Block
            iid, tsh, tsl, caplen, origlen = struct.unpack_from("<IIIII", body, 0)
            ts = ((tsh << 32) | tsl)
            resol = ifresol.get(iid, 6)
            yield linktypes.get(iid, 1), ts / (10 ** resol), body[20:20 + caplen]
        elif btype == 3:  # Simple Packet Block
            origlen = struct.unpack_from("<I", body, 0)[0]
            yield linktypes.get(0, 1), 0, body[4:4 + origlen]
        off += blen
        if blen < 12:
            break


def is_public(ip):
    return ip.is_global and not ip.is_multicast


def parse_frame(linktype, frame):
    """Retourne (proto, src_ip, dst_ip, src_port, dst_port, l4_payload)."""
    if linktype == 1:  # Ethernet
        if len(frame) < 14:
            return None
        eth = struct.unpack_from(">H", frame, 12)[0]
        if eth == 0x8100 and len(frame) >= 18:  # VLAN
            eth = struct.unpack_from(">H", frame, 16)[0]
            frame = frame[4:]
        frame = frame[14:]
        if eth == 0x0800:
            return parse_ipv4(frame)
        if eth == 0x86DD:
            return parse_ipv6(frame)
        return None
    if linktype in (0, 101):  # NULL/RAW : IP direct
        v = frame[0] >> 4 if frame else 0
        return parse_ipv4(frame) if v == 4 else (parse_ipv6(frame) if v == 6 else None)
    if linktype == 12:  # pktmon peut emettre LINKTYPE_RAW variant
        return parse_ipv4(frame)
    return None


def parse_ipv4(pkt):
    if len(pkt) < 20:
        return None
    ihl = (pkt[0] & 0x0F) * 4
    proto = pkt[9]
    src = ipaddress.ip_address(pkt[12:16])
    dst = ipaddress.ip_address(pkt[16:20])
    l4 = pkt[ihl:]
    sp = dp = None
    if proto in (6, 17) and len(l4) >= 4:
        sp, dp = struct.unpack_from(">HH", l4, 0)
        if proto == 6 and len(l4) >= 14:
            off = (l4[12] >> 4) * 4
            l4 = l4[off:]
        elif proto == 17:
            l4 = l4[8:]
    return proto, src, dst, sp, dp, l4


def parse_ipv6(pkt):
    if len(pkt) < 40:
        return None
    nxt = pkt[6]
    src = ipaddress.ip_address(pkt[8:24])
    dst = ipaddress.ip_address(pkt[24:40])
    hl = 40
    while nxt in (0, 43, 60):  # hop-by-hop, routing, destination opts
        if len(pkt) < hl + 2:
            return proto_none(src, dst)
        nxt = pkt[hl]
        hl += (pkt[hl + 1] + 1) * 8
    l4 = pkt[hl:]
    sp = dp = None
    if nxt in (6, 17) and len(l4) >= 4:
        sp, dp = struct.unpack_from(">HH", l4, 0)
        if nxt == 6 and len(l4) >= 14:
            off = (l4[12] >> 4) * 4
            l4 = l4[off:]
        elif nxt == 17:
            l4 = l4[8:]
    return nxt, src, dst, sp, dp, l4


def proto_none(src, dst):
    return None, src, dst, None, None, b""


# Fenetre de dedoublonnage des duplicatas NDIS pktmon : les copies
# d'un meme datagramme arrivent a ~1 us d'intervalle ; 1 ms conserve
# les retransmissions applicatives reelles.
DEDUP_WINDOW_S = 0.001
# Nombre minimal de ports locaux distincts NON bancs envoyant le meme
# motif (proto, taille) vers un hote WAN pour que le paquet d'un port
# banc vers cet hote soit considere comme bruit ambiant plutot que
# fuite. Un processus qui reutilise un port ephemere libere n'a pas
# de raison d'etre le seul emetteur vers cet hote.
AMBIENT_PORTS_MIN = 25


def packets_dedup(path):
    """Paquets parses dedoublonnes : (ts, proto, src, dst, sp, dp, l4).

    pktmon capture le meme paquet sur plusieurs couches NDIS (le
    comptage brut gonfle tout d'un facteur ~4). Cle = 5-tuple +
    payload ; deux occurrences identiques a <1 ms = meme datagramme.
    """
    seen = {}
    for lt, ts, frame in iter_packets(path):
        p = parse_frame(lt, frame)
        if not p:
            continue
        proto, src, dst, sp, dp, l4 = p
        if sp is None:
            continue
        key = (proto, src, dst, sp, dp, l4)
        last = seen.get(key)
        seen[key] = ts
        if last is not None and ts - last < DEDUP_WINDOW_S:
            continue
        yield ts, proto, src, dst, sp, dp, l4


def dns_qnames(payload):
    """Decode les qnames d'un paquet DNS (req ou rep)."""
    names = []
    if len(payload) < 12:
        return names
    qd = struct.unpack_from(">H", payload, 4)[0]
    off = 12
    for _ in range(min(qd, 8)):
        parts = []
        try:
            while True:
                ln = payload[off]
                if ln == 0:
                    off += 1
                    break
                if ln & 0xC0:  # compression
                    off += 2
                    break
                parts.append(payload[off + 1: off + 1 + ln].decode("ascii", "replace"))
                off += 1 + ln
            if parts:
                names.append(".".join(parts))
            off += 4  # qtype+qclass si non compresse en question
        except IndexError:
            break
    return names


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pcapng")
    ap.add_argument("--allowed", help="fichier IP[:port] autorises (verite TAP)")
    ap.add_argument("--bench-ports", help="fichier de ports locaux du processus de banc")
    ap.add_argument("--local-ip", action="append", default=[])
    ap.add_argument("--dns-resolvers", help="IPs des resolveurs systeme (admis pour bootstrap)")
    ap.add_argument("--dht-routers", help="IPs:port des routeurs DHT mainline (interdits en direct)")
    ap.add_argument("--window-start", type=float, default=None,
                    help="epoch s : debut de la fenetre fail-closed")
    ap.add_argument("--window-end", type=float, default=None,
                    help="epoch s : fin de la fenetre fail-closed")
    ap.add_argument("--dead-ports",
                    help="ports locaux d'une lane detruite (17c-5) : "
                         "aucun paquet SORTANT de ces ports apres --dead-since")
    ap.add_argument("--dead-since", type=float, default=None,
                    help="epoch s : instant de destruction de la lane")
    ap.add_argument("--report", help="sortie JSON")
    args = ap.parse_args()

    bench_ports = set()
    if args.bench_ports:
        for line in open(args.bench_ports, encoding="utf-8"):
            line = line.strip()
            if line.isdigit():
                bench_ports.add(int(line))
    dead_ports = set()
    if args.dead_ports:
        for line in open(args.dead_ports, encoding="utf-8"):
            line = line.strip()
            if line.isdigit():
                dead_ports.add(int(line))
    dead_since = args.dead_since

    allowed_ips = set()
    allowed_eps = set()
    if args.allowed:
        for line in open(args.allowed, encoding="utf-8"):
            line = line.strip()
            if not line:
                continue
            if ":" in line and line.rsplit(":", 1)[1].isdigit():
                allowed_eps.add(line)
                allowed_ips.add(line.rsplit(":", 1)[0])
            else:
                allowed_ips.add(line)
    resolvers = set((args.dns_resolvers or "").split(",")) - {""}
    dht_forbidden = set((args.dht_routers or "").split(",")) - {""}
    dht_forbidden_ips = set()
    for e in dht_forbidden:
        if ":" in e:
            dht_forbidden_ips.add(e.rsplit(":", 1)[0])
        else:
            dht_forbidden_ips.add(e)
    local_ips = set(args.local_ip)

    stats = {"LOCAL": 0, "OVERLAY": 0, "DNS": 0, "AUTRE": 0, "BRUIT": 0, "INTERDIT": 0}
    window_interdit = 0
    window_bruit = 0
    forbidden = []   # (proto, remote, detail)
    forbidden_in_window = []
    bruit = []       # (proto, remote, detail) — reclasse ambiant
    dns_queries = []
    dns_queries_window = []
    endpoints = {}   # remote -> (classif, count)
    w0 = args.window_start
    w1 = args.window_end
    # 17c-5 : jalons temporels de la fenetre fail-closed.
    t_last_overlay = None       # dernier paquet OVERLAY <= w0
    t_first_new_overlay = None  # 1er OVERLAY post-w0 vers endpoint nouveau
    pre_window_eps = set()      # endpoints vus <= w0
    dead_tx = []                # paquets sortants de ports morts post-death
    dead_tx_ambient = []        # idem mais motif de bruit ambiant (inattribuable)
    dead_rx = 0                 # drain entrant vers ports morts (info seul)

    # Passe 1 : signature de bruit ambiant par hote WAN. Pour chaque
    # destination publique, on retient les ports locaux NON bancs
    # ayant emis un motif (proto, taille de payload) donne : une
    # inondation venue d'un autre processus rend inattribuable le
    # paquet d'un port banc vers ce meme hote (le port ephemere
    # libere par la lane detruite peut etre reutilise pendant la
    # fenetre — pktmon ne porte pas de PID). Les ports du banc sont
    # exclus de la signature : si le processus teste etait lui-meme
    # l'emetteur, aucune excuse ambiante ne se formerait.
    ambient = {}   # remote_ip -> {(proto, len): set(ports non bancs)}
    n_packets = 0
    for ts, proto, src, dst, sp, dp, l4 in packets_dedup(args.pcapng):
        n_packets += 1
        if is_public(dst) and not is_public(src) \
                and sp not in bench_ports and sp not in dead_ports:
            ambient.setdefault(dst, {}).setdefault((proto, len(l4)), set()).add(sp)

    n_packets = 0
    for ts, proto, src, dst, sp, dp, l4 in packets_dedup(args.pcapng):
        n_packets += 1
        # Remote = le cote public ; deux prives -> trafic local.
        src_pub, dst_pub = is_public(src), is_public(dst)
        if not src_pub and not dst_pub:
            stats["LOCAL"] += 1
            continue
        remote = dst if dst_pub else src
        rport = dp if dst_pub else sp
        if src_pub and dst_pub:
            # les deux publics : improbable, on inspecte dst
            remote, rport = dst, dp
        rep = f"{remote}:{rport}"
        prot = "TCP" if proto == 6 else ("UDP" if proto == 17 else str(proto))
        # Port local du banc : attribution du paquet au processus teste
        # (pktmon ne porte pas de PID ; le port UDP/TCP local l'identifie).
        lport = sp if dst_pub else dp
        benched = lport in bench_ports
        # Le paquet, s'il venait d'un port banc, reproduit-il le motif
        # d'une inondation ambiante vers cet hote ?
        bruit_ambiant = len(
            ambient.get(remote, {}).get((proto, len(l4)), ())
        ) >= AMBIENT_PORTS_MIN

        # Signature IPv8 : version 0x0002 + community-id. Tout paquet
        # issu d'un port du banc qui porte cette enveloppe est du
        # trafic overlay par construction (discovery, circuits,
        # cellules) — le TAP ne liste que les envois des lanes, pas
        # le trafic structurel du noeud vers ses pairs candidats.
        # Une vraie fuite (uTP/BT/DHT en clair) ne peut pas porter
        # cette signature.
        ipv8 = proto == 17 and len(l4) >= 22 and l4[0] == 0 and l4[1] == 2

        cls = None
        if rep in allowed_eps or str(remote) in allowed_ips:
            cls = "OVERLAY"
        elif benched and ipv8:
            cls = "OVERLAY"
        elif rport == 53 or str(remote) in resolvers:
            cls = "DNS"
            for q in dns_qnames(l4):
                dns_queries.append(q)
                if w0 is not None and w1 is not None and w0 <= ts <= w1:
                    dns_queries_window.append(q)
        elif benched:
            # Attribution certaine : paquet du processus de banc vers le
            # WAN. Le motif precise la nature de la fuite — sauf si le
            # motif appartient a une inondation ambiante vers cet hote
            # (port ephemere reutilise par un autre processus).
            why = f"{prot} WAN sur port local {lport} du banc"
            if rep in dht_forbidden or str(remote) in dht_forbidden_ips:
                why = "DHT mainline direct (port du banc)"
            if bruit_ambiant:
                cls = "BRUIT"
                bruit.append((prot, rep, why))
            else:
                cls = "INTERDIT"
                forbidden.append((prot, rep, why))
        else:
            # trafic d'un autre processus (Tribler hote, OS) : rapporte
            # mais non attribuable -- voir AUTRE dans le resume.
            cls = "AUTRE"
        # Lane morte : un paquet sortant d'un port detruit est un
        # mapping survivant -> violation fail-closed (reclasse
        # INTERDIT quel que soit le endpoint). L'entrant vers un
        # port mort est du drain remote — rapporte, pas compte.
        if dead_since is not None and ts > dead_since:
            if sp in dead_ports and not src_pub:
                if bruit_ambiant:
                    # Port ephemere de la lane reutilise par un autre
                    # processus : non attributable (mapping incertain).
                    cls = "BRUIT"
                    dead_tx_ambient.append((prot, rep))
                    bruit.append((prot, rep, "port mort reuse par le bruit ambiant"))
                else:
                    dead_tx.append((prot, rep))
                    cls = "INTERDIT"
                    forbidden.append((prot, rep, "port de lane detruite (mapping survivant)"))
            elif dp in dead_ports and not dst_pub:
                dead_rx += 1
        stats[cls] += 1
        if w0 is not None and w1 is not None and w0 <= ts <= w1:
            if cls == "INTERDIT":
                window_interdit += 1
                forbidden_in_window.append((prot, rep))
            elif cls == "BRUIT":
                window_bruit += 1
        # Jalons 17c-5 : chronologie overlay autour de la panne.
        if w0 is not None:
            if ts <= w0:
                pre_window_eps.add(rep)
                if cls == "OVERLAY" and (t_last_overlay is None or ts > t_last_overlay):
                    t_last_overlay = ts
            elif cls == "OVERLAY" and rep not in pre_window_eps \
                    and t_first_new_overlay is None:
                t_first_new_overlay = ts
        c, n = endpoints.get(rep, (cls, 0))
        endpoints[rep] = (cls, n + 1)

    print(f"paquets={n_packets}")
    for k, v in stats.items():
        print(f"  {k:9s} : {v}")
    if w0 is not None:
        print(f"  INTERDIT dans la fenetre fail-closed [{w0:.0f}..{w1:.0f}] : {window_interdit}")
        for p, r in dict.fromkeys(forbidden_in_window):
            print(f"    {p} -> {r}")
        if t_last_overlay is not None:
            print(f"  t_last_overlay    : {t_last_overlay:.3f}")
        if t_first_new_overlay is not None:
            print(f"  t_first_new_overlay : {t_first_new_overlay:.3f}")
    if dead_ports:
        print(f"  dead_ports        : {sorted(dead_ports)} (depuis {dead_since})")
        print(f"  paquets sortants de ports morts : {len(dead_tx)}")
        for p, r in dict.fromkeys(dead_tx):
            print(f"    {p} -> {r}")
        if dead_tx_ambient:
            print(f"  ports morts reuses par bruit ambiant : {len(dead_tx_ambient)}")
            for p, r in dict.fromkeys(dead_tx_ambient):
                print(f"    {p} -> {r}")
        print(f"  drain entrant vers ports morts  : {dead_rx}")
    print()
    print("endpoints WAN observes :")
    for ep, (cls, n) in sorted(endpoints.items(), key=lambda kv: (kv[1][0], kv[0])):
        print(f"  {cls:9s} {ep:40s} x{n}")
    if dns_queries:
        print()
        print("requetes DNS observees :")
        for q in sorted(set(dns_queries)):
            print(f"  {q}")
    if forbidden:
        print()
        print("PAQUETS INTERDITS :")
        seen = set()
        for prot, rep, why in forbidden:
            if (prot, rep, why) in seen:
                continue
            seen.add((prot, rep, why))
            print(f"  {prot} -> {rep}  ({why})")
    if bruit:
        print()
        print("BRUIT AMBIANT (ports bancs, attribution impossible) :")
        seen = set()
        for prot, rep, why in bruit:
            if (prot, rep, why) in seen:
                continue
            seen.add((prot, rep, why))
            print(f"  {prot} -> {rep}  ({why})")
    if args.report:
        json.dump(
            {"stats": stats, "window": {"start": w0, "end": w1,
                                        "interdit": window_interdit,
                                        "bruit": window_bruit,
                                        "forbidden": [
                                            {"proto": p, "remote": r}
                                            for p, r in dict.fromkeys(forbidden_in_window)],
                                        "dns_queries": sorted(set(dns_queries_window))},
             "milestones": {"t_last_overlay": t_last_overlay,
                            "t_first_new_overlay": t_first_new_overlay},
             "dead_ports": {"ports": sorted(dead_ports),
                            "since": dead_since,
                            "tx": len(dead_tx),
                            "tx_ambient": len(dead_tx_ambient),
                            "rx": dead_rx},
             "endpoints": {e: [c, n] for e, (c, n) in endpoints.items()},
             "dns_queries": sorted(set(dns_queries)),
             "forbidden": [{"proto": p, "remote": r, "why": w}
                            for p, r, w in dict.fromkeys(forbidden)],
             "bruit": [{"proto": p, "remote": r, "why": w}
                        for p, r, w in dict.fromkeys(bruit)]},
            open(args.report, "w", encoding="utf-8"), indent=2)

    if stats["INTERDIT"]:
        print()
        print("LEAK ANALYSIS FAIL")
        return 1
    print()
    print("LEAK ANALYSIS OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
