# Polices auto-hébergées — provenance

ADR-0021 §3 : typographie zéro requête réseau. Les trois polices
variables ci-dessous sont récupérées une fois (à la conception) depuis
le dépôt [`google/fonts`](https://github.com/google/fonts) (branche
`main`, 2026-10-09) et embarquées comme assets statiques — **aucune
dépendance `google_fonts`, aucun appel réseau au runtime**.

Licence : **SIL Open Font License 1.1** (`OFL.txt` dans chaque
dossier) — distincte de la licence GPL-3.0-or-later du code OnionBit,
au même titre que `vendor/` (librqbit, Apache-2.0, AGENTS.md). Ces
fichiers ne portent donc **pas** l'en-tête GPL du projet.

| Police | Fichier | Usage (ADR-0021 §3) | Source |
| :--- | :--- | :--- | :--- |
| Inter | `Inter/Inter-Variable.ttf` | Texte d'interface (corps, labels) | `ofl/inter/Inter[opsz,wght].ttf` |
| Space Grotesk | `SpaceGrotesk/SpaceGrotesk-Variable.ttf` | Titres, identité visuelle | `ofl/spacegrotesk/SpaceGrotesk[wght].ttf` |
| JetBrains Mono | `JetBrainsMono/JetBrainsMono-Variable.ttf` | Hex, infohashes, clés, phrases de seed (ADR-0016) | `ofl/jetbrainsmono/JetBrainsMono[wght].ttf` |

Les trois sont des **polices variables** (axe `wght`, Inter porte en
plus `opsz`) — un seul fichier par famille. Déclarées dans
`pubspec.yaml` avec plusieurs entrées `weight:` pointant vers le même
fichier (technique standard Flutter pour exposer les graisses d'une
police variable à l'API `TextStyle(fontWeight:)` sans recourir à
`FontVariation` dans chaque site d'appel) — voir
`core/design/tokens/typography_tokens.dart`.

Mise à jour : retélécharger les fichiers depuis les mêmes chemins
`ofl/<famille>/` du dépôt `google/fonts` et remplacer tel quel ;
`OFL.txt` suit la même procédure.
