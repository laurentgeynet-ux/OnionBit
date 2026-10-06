# Banc ADR-0015 — extension OnionBit-only (Phase 9b/9d)

Journal de campagne pour la communauté `OnionbitExtCommunity`
(`hello` lazy, attestations de curation signées).

Références : ADR-0015 §2/§6, commits `77f7f6d` (9b), `9e49911`
(observabilité), `21ca907` (9d), `b1fc72a` (durcissement réception).

## Format du journal

`journal.jsonl` — une ligne JSON par scénario exécuté :

```json
{"ts", "commit", "scenario", "ext_config", "topology", "duration_ms",
 "counters": {node: {peer_count, attest_rx, attest_dropped,
                    attest_stored, attest_tx, hello_tx, hello_probed}},
 "endpoint_volume": {pkts, bytes, attest_pkts, hello_pkts},
 "cpu_rss": <mesuré par le harnais ps1, null en process>,
 "legacy_verdict": "PASS|FAIL|n/a",
 "db_before_after", "score_before_after", "oracles"}
```

## T2 — mesh de curation (exécuté)

`cargo test -p onionbit-ipv8 --test ext_bench t2_mesh_curation`

Topologie loopback : A curateur ; B/C suivent A ; D ne suit personne ;
E émetteur signé non suivi.

| Cas | Oracle | Résultat |
|---|---|---|
| Attestation A valide | B/C stockent + score +1 ; D vide | PASS |
| Rejeu identique | `rx+1`, `stored`/`tx` figés | PASS |
| Attestation E valide | `dropped+1`, jamais stockée | PASS |
| Conflit même `ts` | rejet, score/DB inchangés, C intact | PASS |
| Verdict plus récent | remplace, score −1, propagé à C | PASS |
| Unfollow (store partagé) | score 0 / `attestation_count` 1 ; re-suivi → −1 | PASS |

Journal T2 : compteurs par nœud (B : `rx=5 stored=2 dropped=3 tx=4`),
volume tap sur A (`pkts=7 bytes=2024`).

## T3 — flood contrôlé (exécuté)

`cargo test -p onionbit-ipv8 --test ext_bench t3_flood_controle`

- (a) 300 `ATTEST` d'une clé de transport en < 60 s :
  `rx=300 stored=256 dropped=44` — le budget 256/fenêtre borne.
- (b) 12 clés transport × 5 attestations, `attest_rate_table_max=8` :
  `rx=60 stored=40 dropped=20` — les émetteurs au-delà de la table
  pleine sont droppés sans insertion (Sybil bornée) ; un membre de la
  table reste servi (`stored=41`).

## T4 — cadence/extinction (exécuté en relatif)

`cargo test -p onionbit-ipv8 --test ext_bench t4_cadence_ext_s_eteint`

- `hello_tx` figé après convergence (ticks ultérieurs : 0 émission —
  pairs déjà connus-ext) ;
- publication unique → rafale bornée puis `attest_tx` figé (Δ 1 s = 0) ;
- rejeu post-convergence absorbé par dedup (`dropped+1`, 0 émission).

## T1 — legacy silence (exécuté : PASS 8/8)

`scripts\bench_ext_silence.ps1` — mesh fermé A1 (OnionBit ext, exit) +
D (OnionBit ext, `curators=[A1pk]`) + T (Tribler.exe officiel
Discovery+Tunnel). Run `target/bench-ext-silence-20261005-151405/`,
~109 s.

Résultats (commit `b1fc72a` + wc compteurs hello) :

| Oracle | Mesuré | Verdict |
|---|---|---|
| T jamais pair ext | `A1.ext.peer_count=1` (D seul) | PASS |
| hello opportuniste borné | `hello_probed=2`, `hello_tx=2` | PASS |
| gossip OnionBit-seul | `A1.stored=1 tx=1 ; D.stored=1 rx=1` | PASS |
| score suiveur | `D.trust.score=1` | PASS |
| discovery legacy | `A1 discovery peers=2` (D+T) | PASS |
| overlays de T | `[Discovery, DatabaseComponent, TriblerTunnel]` | PASS |
| peers legacy de T | ≥1 overlay peuplé | PASS |
| log T | aucune erreur paquet | PASS |

Conséquence : **aucun ATTEST n'est adressé à Tribler** (le gossip ne
cible que `ext_peers`, et T ne répond jamais au `hello`) ; le seul
trafic ext vers T est le `hello` opportuniste borné, silencieusement
ignoré — exactement le comportement documenté ADR-0015 §2.

Note exécution : la config Tribler doit utiliser
`tunnel_community.enabled` — un overlay `TriblerTunnelCommunity`
explicite dans `ipv8.overlays` est rejeté (`no associated Community`).

## T4 — run de 15 min (scripté, exécution terrain)

`scripts\fingerprint_mesh.ps1 -WithExt [-WithAnonDownload]`
— même harnais que les campagnes d'empreinte : A1+D+A2+A3 OnionBit
(ext enabled, `curators=[A1pk]`) + Tribler, échantillonnés par
`fingerprint_stats.ps1` (CSV `fp_onionbit_mesh.csv`,
`fp_tribler_mesh.csv` + nouveau `ext_onionbit.csv` avec les compteurs
`hello_*`/`attest_*` par tick). Une attestation est publiée par A1 à
t+60 s → observer pic puis extinction (`attest_tx → 0`).

Seuils de décision (relatifs) : après convergence, `attest_tx/s → 0`,
aucun trafic ext périodique non justifié, pas de croissance continue
`peers`/`hello_probed`.

Run de validation 3 min (`target/fingerprint-mesh-20261005-151602/`) :
publish à t+60 s → à l'échantillon 15:17:08 `rx=3 dropped=2 stored=1
tx=2`, `peer_count=3`, `hello_tx=4/probed=4`, puis **tous les compteurs
figés** sur les 13 échantillons suivants — extinction totale du gossip,
zéro trafic ext périodique (dropped=2 = replays absorbés par dedup).

**Run 15 min exécuté** (`target/fingerprint-mesh-20261005-151923/`,
90 échantillons, commit `72657e9`) : une seule transition dans tout le
CSV — tout à 0 jusqu'au publish t+60 s, puis `peer_count=3,
hello_tx=4/probed=4, rx=3 dropped=2 stored=1 tx=2` **figés sur les
84 échantillons restants (~14 min)**. Volumes endpoint 15 min :
OnionBit `up=3.42 MB / down=3.29 MB` vs Tribler `1.73 / 1.59 MB` —
l'écart reste celui des walks discovery déjà observés sur les baselines
(aucun trafic ext périodique ajouté : `attest_tx` n'a jamais bougé
après le pic). Seuils relatifs validés : `attest_tx/s → 0`, pas de
croissance `peers`/`hello_probed`.

## Reste terrain

- Expiration/purge DB : migration v16→v17 + redémarrage avec
  attestations persistantes — couvert partiellement par le test
  `attestation_store::store_db_aller_retour_latest_wins` ; un banc
  daemon (restart réel sur `onionbit.db`) reste à scripter.
- Runs longs optionnels : flood ext réel multi-processus (T3 process)
  et fingerprint comparative baseline `-WithExt` off/on.
