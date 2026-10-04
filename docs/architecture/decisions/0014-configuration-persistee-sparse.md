# ADR-0014 — `configuration.json` sparse : seuls les écarts aux défauts sont persistés

Statut : Acceptée (2026-10-04). Extension Rust — `TriblerConfig`
Python sérialise l'arbre complet, ce qui y gèle les défauts de
l'époque sans recours.

## Contexte

Jusqu'ici `DaemonConfig::write()` sérialisait l'arbre complet
(`serde_json::to_string_pretty(self)`). Conséquence : au premier
lancement, **toutes les valeurs par défaut** étaient écrites dans
`configuration.json`. Un défaut corrigé dans une version ultérieure
ne se propageait jamais — le fichier figeait l'ancienne valeur à vie
(cas réel : `tunnel_community/bandwidth/target_delay_ms` né à 50 ms,
corrigé à 25 ms, restait à 50 dans toutes les configs existantes).

Le mécanisme `config_version` + `migrate_legacy_tree` permettait de
réaligner au cas par cas, mais exigeait une entrée de migration par
défaut modifié — fragile et facile à oublier.

## Décision

Le fichier ne persiste que les **écarts aux défauts**
(`deep_diff(to_value(cfg), to_value(DaemonConfig::default()))`) :

- une clé absente du fichier prend le défaut **de la version en
  cours** via `#[serde(default)]` au chargement — un défaut corrigé
  dans une release se propage automatiquement à tous les fichiers ;
- `config_version` est toujours écrit (pilote les migrations) ;
- `api.key` et `api/http_port_running` figurent naturellement
  (non-défauts par construction) ;
- les clés inconnues (`extra` flatten) et la section libre `ui`
  sont préservées (elles n'existent pas dans le défaut → différant).

`load` est inchangé : `#[serde(default)]` au niveau conteneur remplit
les clés absentes à chaque niveau. `POST /api/settings` est inchangé
sémantiquement : un patch égal au défaut courant n'est simplement pas
persisté — l'effet observé est identique.

Les tables de migrations restent nécessaires pour les fichiers écrits
par les versions « denses » (valeur gelée = ancien défaut → réalignée,
choix explicite préservé) : v1→v2 réaligne `target_delay_ms` 50→25.

## Conséquences

- `configuration.json` d'une installation fraîche ≈
  `{config_version, api:{key, http_port_running}}` + sections touchées.
- Le fichier n'est plus auto-documenté — la référence des défauts est
  le code (`*Config::default()`) et `GET /api/settings` (arbre
  effectif complet).
- Limite assumée : une valeur explicitement réglée **égale au défaut
  courant** n'est pas distinguée du « jamais réglé » — si le défaut
  change plus tard, elle suivra le nouveau défaut.
- Consommateurs du fichier brut (`onionbit-cli`, tests) : lisent via
  `pointer()` avec repli sur les défauts — compatible.
