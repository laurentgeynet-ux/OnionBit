# ADR-0002 — IPv8/TunnelCommunity inclus dès la V1 du daemon

Statut : Acceptée (2026-09-27).

## Contexte

Le réseau d'anonymisation IPv8 (`pyipv8` + `tribler.core.tunnel`) est la
fonctionnalité la plus différenciante de Tribler ("Bittorrent anonyme et
impossible à arrêter") mais aussi, de loin, la plus complexe à porter :
protocole propriétaire, pas d'équivalent Rust complet (`ipv8-rust-tunnels`
ne couvre que le plan de données des tunnels, pas le protocole de
contrôle). Deux options ont été présentées à l'utilisateur :

1. V1 sans IPv8 (BitTorrent classique d'abord), IPv8 en V2.
2. IPv8 inclus dès la V1.

## Décision

Option 2 : IPv8/TunnelCommunity fait partie du périmètre de la V1 du
daemon (choix explicite de l'utilisateur).

## Justification

- L'anonymat est au coeur de l'identité du produit Tribler ; un daemon
  "V1 sans IPv8" serait un client BitTorrent générique sans valeur
  différenciante par rapport aux nombreuses alternatives Rust existantes
  (`rqbit`, etc.).

## Conséquences

- `docs/plans/roadmap.md` place le portage IPv8 (étapes 9-11) et
  TunnelCommunity (étape 12) **avant** la bascule vers l'UI Flutter,
  conformément à la règle "backend à 100 % avant l'UI".
- Le risque et le volume de travail du projet sont dominés par cette
  phase (cf. `plan_faisabilite.md` §5-6) : le développement doit être
  incrémental, avec des jalons d'interopérabilité contre de vrais pairs
  IPv8 Python le plus tôt possible, pour détecter les écarts de
  protocole avant d'avoir tout construit dessus.
- Le sous-ensemble minimal à porter en premier (étape 9) est la
  discovery + une community triviale, afin de valider l'interopérabilité
  binaire avant d'attaquer la cryptographie de circuit et
  `TunnelCommunity` (étape 12).
