# Fixtures d'interop IPv8 — provenance

Paquets filaires **réels** enregistrés le 2026-09-27 pendant l'échange
orchestré par `scripts/interop_ipv8.ps1` (loopback `127.0.0.1`, ports
11090/11091, durée 9 s). Rejoués en CI par `tests/interop_replay.rs`.

## Référence ayant produit les paquets

- pyipv8 : sous-module de `D:\Projet\Tribler_sources\tribler`, commit
  `4a294ed1e98eb113e664f96dc0e988739ab23bec` (2026-07-08) ; dépôt Tribler
  parent `3ac2f4b461f789d33d3acfc4edc0489bfc93aabf` (2026-09-16).
- Interpréteur : venv `D:\Projet\Tribler_sources\.venv-interop`
  (chemins réglables via `TRIBLER_PYIPV8` / `TRIBLER_INTEROP_PY`).
- Noeud Python : `scripts/interop/py_node.py` (`UDPEndpoint` +
  `DiscoveryCommunity`, clé `curve25519`).
- Noeud Rust : `crates/onionbit-ipv8/examples/interop_node.rs`.

Note : cette capture provient du **venv pyipv8**, pas de l'application
`Tribler.exe` installée (8.4.3) — cible d'interop distincte, non encore
exercée.

## Contenu

Community `DiscoveryCommunity` (`7e313685c1912a141279f8248fc8db5899c5df5a`),
une ligne hex = un datagramme UDP complet
(`0x00` + version `0x02` + community_id 20o + msg_id + `varlenH` pubkey +
global_time `Q` + payload + signature Ed25519 64o).

| Fichier | Sens | Messages (msg_id) |
| :--- | :--- | :--- |
| `pyipv8_discovery.hex` | TX Python → RX Rust | 2× 246 (introduction-request ancien format), 1× 1 (similarity-request), 3× 245 (introduction-response) — 6 paquets |
| `rust_discovery.hex` | TX Rust → RX Python | 4× 246, 1× 2 (similarity-response), 1× 245 — 6 paquets |

Résultat attendu au rejeu : `Packet::parse` accepte chaque ligne et
`pkt.signed == true` (la signature a déjà été vérifiée en direct par le
`default_eccrypto` pyipv8 pendant l'essai — 39/39 paquets acceptés dans
les deux sens, pairs mutuellement enregistrés).

## Régénération

`powershell -File scripts\interop_ipv8.ps1` — journaux bruts dans
`target/interop/` (`py_packets.log` / `rust_packets.log`) ; recopier les
lignes hex ici pour figer une nouvelle capture.
