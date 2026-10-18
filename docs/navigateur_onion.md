# Navigateur `.onion` dans OnionBit (ADR-0030)

OnionBit embarque un **lecteur `.onion` minimal** — `onionbit-browser`
(port Rust de Dillo) — lancé par le daemon à travers un proxy HTTP
loopback servi par son client Tor intégré. Ce n'est **pas** un Tor
Browser : pas de JavaScript, CSS limité, pages statiques. Le but est
de lire les services `.onion` autorisés (annuaires, moteurs, sources
de recherche) sans sortir du produit.

## Prérequis

1. **Le binaire** : le bundle Windows embarque
   `windows\bin\onionbit-browser.exe` — dans ce cas, renseigner
   Réglages → Réseau → Navigateur → **chemin** =
   `bin\onionbit-browser.exe` (résolu relativement au dossier du
   daemon ; un chemin absolu fonctionne aussi).
2. **Tor activé** : Réglages → Réseau → Tor → « Activer Tor »
   (mode `embedded` recommandé — rien à installer).
3. **Le proxy navigateur** : même écran, section Navigateur →
   « Proxy navigateur ». La pastille affiche l'adresse loopback
   réelle (`127.0.0.1:port`) et le nombre de connexions actives.

## Lancer une navigation

Dans la section Réseau, chaque hôte `.onion` de l'allowlist porte
une icône d'ouverture — un clic lance `onionbit-browser` sur
`http://<hôte>/` à travers le proxy Tor. Le premier lancement attend
le bootstrap Tor (barre de progression dans la pastille Tor) ; les
suivants sont quasi immédiats (consensus en cache).

Ce qui est autorisé, par défaut : **`http`/`https` vers des
`.onion` v3 uniquement** — tout le reste (IP littérale, adresse
clearnet, port exotique) est refusé *avant* toute connexion.

## `allow_clearnet_exit` — à connaître avant d'y toucher

Réglages → Réseau → Navigateur → « Sortie clearnet » :

| Valeur | Effet | Ce que voit l'exit Tor |
|---|---|---|
| `off` (défaut) | `.onion` seulement | — |
| `https-only` | `https://` clearnet via exit | la destination (`CONNECT host:443`), contenu chiffré |
| `on` | `http`+`https` clearnet via exit | la destination, et le HTTP part **en clair** |

Le hint de l'UI le dit honnêtement — ce n'est pas un mode
« navigation normale », c'est un compromis documenté.

## Ce que le navigateur garantit — et ne garantit pas

Mesuré par `scripts/sec_browser_leak.ps1` (verdict **NO_LEAK** sur
session réelle) :

- **toutes** les connexions du navigateur finissent au proxy
  loopback — aucune socket sortante directe, aucun UDP (le nom
  `.onion` n'atteint jamais le resolver local) ;
- aucun processus enfant (plugins dpi désactivés : `no_dpi` +
  `dpidrc` restrictif généré dans le profil) ;
- chaque service `.onion` roule sur son propre groupe de circuits
  (isolation par origine, façon first-party) — observable :
  `browser_proxy.isolation_entries` dans `GET /api/tor/status` ;
- le profil est **jetable** : `state/browser/run-*` est supprimé à
  la fermeture du navigateur (HSTS, cookies, historique non
  persistés). Pour garder le profil : `tor.browser.keep_profile =
  true` — la trace reste alors **en clair**, hors base chiffrée.

Limites assumées (détail : `docs/THREAT-MODEL.md`, section
ADR-0030) : les en-têtes Dillo sont identifiables comme « un
navigateur Dillo » côté service ; le proxy loopback est observable
par les processus locaux (d'où `require_auth`) ; les
sous-domaines `.onion` sont transmis au service.

## Dépannage

| Erreur | Cause | Action |
|---|---|---|
| `browser_not_configured` | proxy désactivé ou `binary_path` vide | activer le proxy, renseigner le chemin |
| `tor_bootstrapping` | bootstrap Tor en cours | attendre `ready` dans la pastille Tor, relancer |
| `denied_target` | hôte hors politique | ajouter à `tor.allowed_hosts` si `.onion` v3 valide ; le clearnet exige `allow_clearnet_exit` |
| `tor_not_ready` / `tor_disabled` | Tor coupé ou dormant | activer Tor |
