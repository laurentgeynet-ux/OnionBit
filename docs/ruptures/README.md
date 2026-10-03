# Bancs de rupture — philosophie fail-closed

Ce dossier documente comment OnionBit prouve qu'une panne ne produit
pas de fuite. L'objectif n'est pas de montrer que le système « gère
l'erreur » : c'est de démontrer, par observation **indépendante** (OS,
pas compteurs applicatifs), qu'aucun octet n'emprunte un chemin non
autorisé pendant la fenêtre de défaillance.

## Le principe fondateur

> **Silence admis, sortie directe bloquante.**
>
> Pendant une panne (circuit détruit, proxy mort, interface coupée,
> processus tué), il est acceptable que le trafic s'arrête — le tunnel
> est un chemin à sens unique. Il est **inacceptable** qu'un paquet
> tente une sortie directe vers le WAN, même d'un seul datagramme.

Cette propriété est celle qui distingue un anonymat sérieux d'un
anonymat déclaratif : un tunnel qui « tombe en direct » fuit.

## Ce qu'un banc de rupture doit prouver

```
INTERDIT(t_failure, t_reprise) = 0
```

- `t_failure` : instant d'injection de la panne (ou de constat).
- `t_reprise` : instant où le chemin légitime redevient utilisable
  (nouveau circuit READY, ou fin de fenêtre si pas de reprise).
- `INTERDIT` : tout paquet émis depuis le processus/lane testée vers
  une destination WAN non overlay — payload BT/uTP direct, DHT
  mainline en clair, DNS de tracker/magnet, TCP WAN.

Le silence total n'est pas un échec : c'est la preuve attendue.

## Recette d'un banc de rupture

Tout nouveau banc de rupture suit ce squelette :

1. **Établir un vrai chemin** — un download anonyme réel sur une route
   multi-sauts observée (`route observee` dans le journal), avec des
   octets vérifiés en cours. Une panne injectée au repos ne prouve rien.
2. **Observer indépendamment** — capture OS (`pktmon`, tcpdump,
   Wireshark) ; les compteurs applicatifs sont la chose sous test, pas
   la mesure.
3. **Injecter la panne à un instant connu** — `t_failure` horodaté au
   millième ; l'injection doit être réelle (kill -9, règle pare-feu,
   NIC down), pas simulée.
4. **Attribuer correctement** — pktmon ne porte pas de PID : on scope
   par port local du banc + fenêtre temporelle + endpoints overlay
   attendus (TAP = vérité fil) + signature IPv8 pour le trafic
   structurel du nœud. Le bruit hôte (`Tribler.exe`, OS) tombe en
   `AUTRE`, jamais dans le verdict.
5. **Fenêtrer l'oracle** — `INTERDIT` compté dans
   `[t_failure, t_fin_capture]`, pas sur toute la capture : le trafic
   normal avant la panne est admis par définition.
6. **Observer la reprise** — si le chemin légitime revient, le
   transfert doit reprendre (`t_first_new_READY`, `t_resume_payload`).
   Pas de reprise = acceptable si le silence est total ; reprise par
   un chemin direct = faute bloquante.
7. **Archiver** — pcapng, manifeste (commit, PID, fenêtre, ports,
   route), rapport JSON, journaux du banc. Une capture sans corrélation
   temporelle est ininterprétable.

## Les cinq classes de panne (P0-17c)

| Classe | Injection réalisée | Couvre |
|--------|--------------------|--------|
| Mort de circuit | Pare-feu sur les premiers sauts observés (proxy vivant) | 17c-1 |
| Mort du worker/infrastructure | taskkill du bootstrap Tribler en plein transfert | 17c-2* |
| Coupure WAN | `Disable-NetAdapter` + rétablissement | 17c-3 |
| Mort du processus | taskkill -F du banc en plein transfert | 17c-4 |
| Réinstanciation de lane | — non injectable sans hook daemon | 17c-5 (trou) |

\* Le worker proxy est in-process dans le banc : la mort du bootstrap +
la mort du processus complet en sont les approximations validées.

## Pièges classiques (déjà rencontrés)

- **`--offline` = `IpPolicy::permissive()`** : un banc anti-SSRF ou de
  politique lancé en offline ne teste rien — le harnais doit vérifier
  la politique effective par une sonde témoin (`sec_anti_ssrf_live.ps1`).
- **Faux positifs d'attribution** : le socket IPv8 du nœud émet du
  trafic structurel (discovery, tentatives de circuit) vers des pairs
  que le TAP ne liste pas — signature IPv8 (`00 02` + community-id)
  = overlay admis, jamais `INTERDIT`.
- **Injection après complétion** : si le transfert finit avant
  `-FailAtBytes`, la panne n'est pas injectée — le verdict
  « transfert actif à l'injection » doit rester vert.
- **Timing de persistence** : un magnet non résolu n'est pas persisté
  (résolution BEP9 bloquante) — un kill juste après ajout ne doit rien
  restaurer, et ce n'est pas un bug.
- **Ne pas confondre inbound et outbound** : du bruit WAN frappant un
  port ouvert n'est pas une fuite ; seul le sens sortant (ou la
  réponse à une sollicitation entrante suspecte) compte.

## Fichiers de référence

- `scripts/sec_leak_capture.ps1` — orchestration : élévation, capture
  pktmon, injection `-Scenario {normal,kill,block,wan,kill-bootstrap}`,
  manifeste.
- `scripts/analyze_leak_capture.py` — classification pcapng :
  `LOCAL/OVERLAY/DNS/AUTRE/INTERDIT`, fenêtrage, qnames DNS.
- `docs/plans/bancs_tests.md` — catalogue complet et journal §7.
- `docs/P0-transport-manifest.md` — preuve figée et limites assumées.
