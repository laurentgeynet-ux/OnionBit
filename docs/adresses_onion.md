# Adresses `.onion` dans OnionBit (ADR-0029)

OnionBit parle **deux réseaux disjoints** :

| | Plan OnionBit | Plan Tor |
|---|---|---|
| Adresse | clé publique `ipv8` | `xxxx….onion` (v3) |
| Transport | circuits uTP multi-sauts | circuits Arti (TCP) |
| Joignabilité | asynchrone (boîtes aux lettres) | pair en ligne requis |
| Pastille UI | verte « OnionBit » | violette « Tor » |

La fusion se fait **uniquement dans l'interface** : jamais un flux
n'est traduit ou relayé d'un plan à l'autre. Une adresse `.onion` est
toujours jointe via Tor ; une clé OnionBit toujours via l'overlay.

## 1. Héberger son identité `.onion`

Opt-in dans `configuration.json` (ou Réglages → Réseau quand exposé) :

```json
"tor": { "enabled": true, "onion_service": { "enabled": true } }
```

Le daemon publie alors un service `.onion` **dérivé de votre identité**
(même phrase BIP39 → même adresse, sur toute machine). L'adresse
n'apparaît dans `GET /api/tor/status` (`onion_service.address`) que
lorsqu'elle est réellement publiée — jamais d'affichage anticipé.
Le service multiplexe les ports virtuels : messagerie (lien `OBMS`),
catalogue de canaux (HTTP port 80). Les ports non servis répondent le
`DONE` standard — indiscernable d'un service ordinaire.

> Session **invitée** : aucun service n'est jamais hébergé.

## 2. Associer un contact à son `.onion`

Page Messagerie → menu d'un contact → **« Adresse .onion… »** :

- saisir l'adresse `….onion` communiquée hors bande par le pair ;
- optionnel : coller sa **signature de liaison** (le pair signe son
  adresse avec sa clé OnionBit — l'association est alors marquée
  « vérifiée »). Sans signature l'association est acceptée mais
  affichée « manuelle » ;
- `DELETE` possible à tout moment ; supprimer le contact purge aussi
  la liaison.

L'association est ce qui rend le pair joignable par Tor : elle est
ajoutée à la politique d'accès (`peer_policy`) — aucun `.onion` non
associé n'est jamais contacté. Elle voyage dans le coffre `OBV1` :
restaurer le coffre sur un autre appareil transporte les liaisons.

## 3. Discuter via `.onion`

À l'envoi, la messagerie choisit : **circuit lié** (overlay) →
**`.onion` associé** → **boîte aux lettres** → échec. La pastille de
chaque message indique le plan réellement emprunté ; l'historique
persiste le plan par message. Exigence : le pair doit être **en
ligne** — sinon repli boîte aux lettres (ADR-0026).

## 4. Catalogues de canaux via `.onion`

Un canal suivi dont le créateur a une association `.onion` est
synchronisé en priorité par Tor (`GET /channels/<pk>` HTTP borné sur
le `.onion` d'identité du créateur), avec repli `RemoteSelect` IPv8.
Chaque entrée reste vérifiée par la signature du canal — Tor ne
confère aucune crédibilité supplémentaire.

## 5. Coffre nomade (restricted-discovery)

```json
"tor": { "onion_service": { "vault": true } }
```

publie un **second** service `.onion` dédié au coffre (adresse propre,
dérivée de l'identité). Son descripteur est chiffré pour la clef
client x25519 elle-même dérivée de l'identité : **seuls vos appareils
peuvent même découvrir qu'il existe**. Il sert `GET /vault` (export
`OBV1`) et `GET /manifest` (`manifest.obm`) — tous deux déjà chiffrés
pour votre clef maîtresse.

Sur l'appareil nomade (même identité restaurée) :

- `POST /api/messaging/vault/restore` — tente le coffre `.onion` puis
  replie sur les ponts `pull_store` IPv8 ;
- `POST /api/private/manifest/restore` — restaure le catalogue de la
  zone privée (AEAD vérifiée avant écriture, fusion avec `.bak`).

Révoquer l'accès = **rotation d'identité** (la suppression d'une clef
client ne coupe pas les points d'introduction déjà rejoints — limite
Arti documentée). La feature `restricted-discovery` d'Arti est
marquée expérimentale (API non semver).

## 6. Vie privée — à lire avant d'activer

- **Un `.onion` qui répond = vous êtes en ligne.** L'hébergement est
  une activité observable par quiconque connaît l'adresse (coffre
  excepté : son descripteur est indéchiffrable pour un tiers).
- **Servir le même contenu sur les deux plans permet la corrélation** :
  un canal publié en IPv8 et joignable en `.onion` peut être relié par
  un observateur des deux réseaux. La liaison signée (`bind_sig`) est
  la seule corrélation *voulue* — n'en faites pas pour un contact que
  vous voulez garder séparé.
- **Fait Tor visible chez le FAI** (consensus/gardes publics) —
  `tor.bridges` masque l'adresse de destination, pas le handshake
  (obfs4/Snowflake non intégrés).
- Voir `THREAT-MODEL.md` §Hosted `.onion` services pour le modèle
  complet.

## 7. Jalons E2E manuels `[i]` — checklist de validation réelle

Tests unitaires et loopback couverts par CI ; les points suivants
demandent un réseau Tor réel. Rejouer sur **Windows + Linux**.

| # | Scénario | Attendu |
|---|---|---|
| E1 | `onion_service.enabled=true`, bootstrap réel | `GET /api/tor/status` → `onion_service.state:"running"`, adresse 56 c. servie ; **même adresse** au redémarrage |
| E2 | Deux daemons, association `.onion` + `bind_sig` | message livré `plane:"onion"`, ACK, pastille violette ; pair hors ligne → repli mailbox `plane:"mailbox"` |
| E3 | Canal suivi, créateur en `.onion` | catalogue synchronisé via Tor (log `channel_sync`), repli `RemoteSelect` si créateur hors ligne |
| E4 | `onion_service.vault=true` sur l'appareil A | `onion_vault.state:"running"` ; descripteur **injoignable** sans clef client (test avec client Tor nu) |
| E5 | Appareil B, même identité restaurée | `POST /api/messaging/vault/restore` rend les contacts + liaisons ; `POST /api/private/manifest/restore` fusionne le catalogue privé |
| E6 | Kill conditions | Tor coupé → `state:"off"`, envois `.onion` replient mailbox, aucune erreur fatale ; reprise après réactivation |
| E7 | Anti-fuite | `tcpdump`/firewall : aucune socket non-Tor vers les `.onion` associés ; aucune socket UDP OnionBit via Tor |
