# ADR-0013 — Débit servi : contrôle de congestion (remplace l'estimation de capacité)

Statut : Acceptée (2026-10-04). Extension Rust sans équivalent
pyipv8 — qui ne borne jamais le débit servi (`max_traffic` est un
quota cumulé de payout, pas un limiteur de débit). La borne reste
une extension locale : `tunnel_community/max_relayed_rate = -1`
(auto, défaut), `0` illimité, `>0` fixe.

## Contexte

Objectif inchangé : le trafic servi aux autres pairs (relai +
sortie) ne doit pas saturer l'upload de la machine — sans valeur
fixe (toutes les lignes diffèrent) et sans réglage manuel.

L'approche « mesurer la capacité puis en prendre une fraction »
(ADR antérieur, implémenté puis retiré) a été invalidée sur le
terrain :

- **UPnP** `WANCommonInterfaceConfig:GetLinkLayerMaxBitRates` :
  dépend d'un IGD qui répond — sur la production observée l'IGD ne
  répond pas du tout (même le mappage de port échoue).
- **Sonde HTTP** (`probe_up_urls`) : fonctionne (mesuré : ~111
  Mbit/s contre ~200 Kio/s estimés passivement — erreur ×56), mais
  POSTe à un tiers, ce qu'un daemon d'anonymat ne doit pas faire
  sans consentement — donc opt-in → inutilisé en pratique.
- **Pic passif** des compteurs endpoint : **censuré par son propre
  plafond** — une fois le cap appliqué, le débit observé ne peut
  plus jamais le dépasser ; l'estimation se verrouille à vie à la
  première valeur observée. C'est le défaut structurel qui rendait
  le système « caduque ».

## Décision

Ne plus mesurer la capacité — **détecter la congestion** (famille
LEDBAT / uTP / « Upload Speed Sense » d'eMule) : quand la file
d'attente montante du routeur gonfle, le RTT vers les pairs
s'inflate.

À chaque tick `bandwidth/sample_secs` (5 s) :

1. Rafale de `ping` Discovery (msg 3, trafic protocole normal) vers
   les `probe_peers` (8) pairs vérifiés les plus frais ;
   `DiscoveryCommunity` expose `set_pong_probe` qui notifie
   `(adresse, identifier)` de chaque `pong` — la `RttProbe` apparie
   aux pings émis (les pongs d'autres sous-systèmes sont ignorés).
2. Médiane des RTT collectés après `probe_wait_ms` (1200 ms).
3. Retard de file = médiane − baseline, baseline = min des médianes
   sur `base_window_secs` (600 s).
4. AIMD : retard ≤ `target_delay_ms` (50) →
   `cap += max(cap/8, 32 Kio/s)` ; retard > cible →
   `cap = cap × 0,75`, borné par `floor_bps` (64 Kio/s). Clamp dans
   `[floor_bps, max_bps]` (32 Mio/s).
5. Aucun échantillon → plafond inchangé ; aucun pair éligible →
   `fallback_bps` (512 Kio/s).

## Propriétés

- **Aucun tiers** : seul du trafic IPv8 chiffré/normal sort.
- **Aucune constante de capacité** : s'adapte à toute ligne et à
  son évolution ; cède aussi la place à la congestion causée par
  d'autres applications — ce qu'une fraction figée ne fait pas.
- **Converge vers la saturation** : au repos le plafond monte
  jusqu'au seuil de congestion réel (le réseau Tribler y gagne),
  puis oscille autour en cédant immédiatement dès qu'un usage
  concurrent apparaît.
- **RTT stable ≠ congestion** : un RTT élevé constant (pairs
  lointains) devient la baseline ; seul le *dépassement* déclenche
  le repli.
- **Limites connues** (USS d'eMule a montré les mêmes) : RTT bruité
  → médiane + fenêtre glissante ; pairs mutés → repli sur
  `fallback`, jamais de montée à l'aveugle.

## Conséquences

- `BandwidthConfig` ne contient plus que des paramètres du
  contrôleur ; `share`, `measure_upnp`, `probe_*`,
  `measure_interval_secs`, `warmup_secs` sont supprimés (les clés
  obsolètes d'un `configuration.json` antérieur sont ignorées par
  `serde(default)`).
- `/api/statistics/ipv8` `bandwidth` expose
  `effective_relay_bps`, `base_rtt_ms`, `median_rtt_ms`,
  `rtt_samples`, `relay_mode`, `relay_dropped` — les champs de
  mesure de capacité disparaissent (UI adaptée : « Signal RTT des
  pairs »).
