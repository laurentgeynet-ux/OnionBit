# Manifeste de preuve P0 — surface transport (2026-10-03)

Ce document fige l'état de preuve de la surface transport OnionBit à
l'issue de la passe P0 complète. Il sert de référence pour toute revue
future : les oracles, artefacts et limites y sont arrêtés — une
régression se mesure contre cette série, pas contre un souvenir.

## Décision

**P0 transport : CLOS.**

Le chemin heureux est capturé au niveau OS, le fail-closed est prouvé
sous quatre pannes distinctes, l'anti-SSRF est vérifié en live sur
daemon strict, et l'interop protocolaire tient contre pyipv8 et
Tribler.exe réels. Le transport n'est plus une zone de risque : c'est
un socle. Les efforts suivants sont de la validation de maturité.

## Scénarios de preuve

| ID | Preuve | Oracle | Résultat |
|----|--------|--------|----------|
| P0-17a | Anti-SSRF + auth live (`sec_anti_ssrf_live.ps1`) | Précondition politique stricte par sonde témoin `127.0.0.1` ; 401 sans/mauvaise clé ; refus fermé loopback/localhost/link-local/privé/CGNAT/unspecified | vert — 11/11 verdicts ; sonde prouvée discriminante contre un daemon `--offline` |
| P0-17b | Capture OS chemin heureux (`sec_leak_capture.ps1` + `analyze_leak_capture.py`) | 0 paquet `INTERDIT` depuis les ports du banc pendant un download anonyme réel | vert — 1 113 975 o vérifiés, route publique 2 sauts, 181 770 paquets, `INTERDIT=0`, fenêtre post-arrêt 20 s |
| P0-17c | Fail-closed observé par l'OS, 4 sous-runs | `INTERDIT(t_failure → t_fin) = 0`, capture et manifeste neufs par run | vert ×4 : `kill` (taskkill à 262 Kio, post-mortem propre) ; `block` (pare-feu premiers sauts à 327 Kio — tout fallback aurait été visible — reprise à 982 Kio) ; `wan` (NIC coupée 45 s à 589 Kio, reprise à 4,9 Mio) ; `kill-bootstrap` (Tribler.exe tué à 851 Kio, download complété à 1,6 Mio) |

Oracle commun : « absence temporaire de trafic admise ; une sortie
directe, même brève, est bloquante ». L'exemption IPv8 par signature
(`00 02` + community-id) garantit que le trafic structurel du nœud vers
ses pairs candidats n'est pas compté — toute trame uTP/BT/DHT en clair
resterait `INTERDIT`.

## Appui de la passe P0 associée

- Interop pyipv8 PY-1..PY-6 verts (discovery, DHT signée, crypto de
  tunnel, relais, sortie `EXIT_BT` 200 Ko octet-à-octet).
- Tribler.exe : hidden download/seed avec guards × hops 1/2/3,
  SHA-256 exact, `hors_set=0`.
- Fingerprint mesh : ping/pong ≈ 0,36 msg/s, pas de tempête ;
  longévité 60 min sans dérive (0,366 → 0,362 msg/s).
- Public 2 sauts : 1 638 263 o vérifiés au premier essai.
- Fuzz smoke : ~152 M exécutions, 0 crash.
- Produit : release reproductible + smoke, lanceur web, crash-recovery
  ×5, 50 torrents en rafale.

## Série de commits de référence

```
60342bc  correctif tempête PING/PONG + durcissements DHT/conntrack
920501e  invariant wire-format, séparation PING/PONG
fa24b6b  passe P0 loopback + catalogue de bancs
693673e  passe P0 réelle : pyipv8, Tribler, mesh, public, fuzz
5f23a3b  anti-SSRF strict + statut PY-7 environnemental
a989b96  capture OS du chemin anonyme normal (P0-17b)
03a5307  fail-closed OS ×4 + analyseur signature IPv8 (P0-17c)
fdc41f5  release reproductible, lanceur web, crash-recovery
402753e  CH-1 (50 torrents en rafale)
272ad7c  longévité bornée 60 min
```

## Limites assumées (trous de couverture)

- **17c-5** : réinstanciation d'une lane anonyme sous capture OS — non
  injectable sans hook daemon/API de reset.
- **17c-2 au sens strict** : le worker proxy est in-process — la mort
  du bootstrap + mort du processus complet sont les approximations
  validées.
- **PY-7** : `BLOCKED / ENVIRONMENTAL` — baseline Tribler↔Tribler sans
  réponse DHT publique en 38 min ; à rejouer quand la baseline
  officielle est verte.
- **PR-4/PR-5** : parcours GUI desktop et multi-navigateurs — manuels.
- **PR-8** : matrice OS (ARM64-Windows, Linux, macOS) — exige une CI
  matricielle.
- **NAT inter-NAT réel** : exige deux accès réseau distincts.
- **CH-2/CH-3** : disque presque plein, swarm dense — bancs lourds ou
  publics, non rejoués.
- **Magnet non résolu non persisté** : un kill avant résolution BEP9
  perd l'ajout (comportement cohérent avec le checkpoint Tribler).

## Bascule P1 — maturité

Prochaine valeur ajoutée, par ordre :

1. **CI matricielle** — au minimum Windows x64 + ARM64, puis Linux/macOS
   (débloque PR-8 et la reproductibilité multi-plateforme).
2. **Longévité automatisée** — le run 60 min local devient un job CI
   périodique ; extension vers 6–24 h avec churn de connectivité.
3. **Documentation publique des scénarios de rupture** — la philosophie
   fail-closed (oracles, fenêtrage, signature IPv8) doit être lisible
   par les contributeurs : `docs/ruptures/README.md` et ce manifeste en
   sont la base (le catalogue détaillé `docs/plans/bancs_tests.md` §6.1
   est interne, hors publication).

Les bancs lourds restants (CH-2, CH-3, endurance > 1 h, NAT inter-NAT)
s'ordonnancent dans ce cadre P1 — aucun ne bloque la clôture de P0.
