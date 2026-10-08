# Revue de securite du transport stealth (ADR-0017)

Checklist a remplir par le relecteur externe **avant** que l'ADR
passe a « Acceptee ». Chaque item pointe le code et les tests qui
font foi ; un item non verifiable ouvert = ADR maintenue en
« Proposee ».

## 1. Fil — forme morphée

- [ ] Aucun octet constant inter-runs dans `hs1`/`hs2`/trames
      (`X'`/`Y'` Elligator2, MAC, padding) — `analyze`
      `oracle_no_constants`, `duplicate_datagrams == 0`.
- [ ] Domain separation `stealth/v1` vs `ext-obf/v1` prouvee par
      rejet croise (`stealth::tests::separation_de_domaine_obf_stealth`).
- [ ] Entropie fil ~8 bits/octet sur capture reelle
      (`oracle_entropy`), pas de motif de taille fixe exploitable
      au-dela de la distribution paddée documentee.
- [ ] `hs1` rejoue exact → zero reponse supplementaire
      (`hs1_rejoue_une_seule_reponse`) ; `X'` vu = filtre deux
      fenetres borne (`xprime_set_max`).

## 2. Silence uniforme (anti-probing)

- [ ] Rejet identique quelle que soit la cause (mauvaise cle,
      timestamp, replay, table saturee) — aucune response,
      aucun delta d'observable (`probing_garbage_silence_total_et_zero_amplification`,
      `troncature_toutes_bornes_rejet_uniforme`).
- [ ] Amplification ≤ 1 : jamais de reponse a un datagramme n'ayant
      pas complete `hs1` — `probe` report.json.
- [ ] Budget pre-DH borne : `hs1_per_ip_per_sec` + `hs1_global_per_sec`
      (le par-IP seul est battable par spoofing UDP), files
      `pre_hs_queue_*` bornees.

## 3. Fail-closed

- [ ] `stealth.enabled × ipv8.enabled` refuse au demarrage sur tous
      les chemins (`Session::start`, `start_offline`, stack).
- [ ] Aucun fallback `RawUdpTransport` en stealth — le
      `DatagramTransport` stealth ne possede de chemin clair pour
      rien (destinataire inconnu → drop, `send_to` hors session
      → queue `Pending` bornee).
- [ ] Secrets hors config : `bridge_sk` dans `stealth_bridge.key`
      (0600), jamais dans `configuration.json`, jamais loggue,
      jamais expose par `/api/stealth` (compteurs bornes uniquement).
- [ ] `client_allowlist` invalide ou lien `onionbit-bridge://`
      malforme → refus de demarrer (fail-closed, pas d'ignore).

## 4. Anti-scraping INTRO

- [ ] Sequence graduee verifiable : seed sans ledger, expansion
      gatee reputation, push reserve reciproques ; pairs inconnus
      refuses en silence (`intro_*` tests).
- [ ] Table partitionnee : bornes par requete / par pair / globale,
      TTL, aucune persistance des intros non sollicitees.
- [ ] Intro forgee inerte : le pair annonce doit prouver
      `bridge_pk` au handshake (`intro_forgee_handshake_inerte`).
- [ ] Enumeration bornee : taux de fuite plafonne par
      `intro_per_ask_max` + quotas agrege par demandeur.

## 5. Resilience sans oracle

- [ ] Restart pont → re-dial en `dial_cooldown_secs` (pas de
      boucle de retries reguliere = motif fingerprintable).
- [ ] Rebinding port source client → nouvelle session sans
      panique du serveur (`nat_rebinding_client_nouvelle_session`).
- [ ] Perte/dup/reordonnancement a 5 % → session vit (fenetre de
      rejeu `replay_window` tolerante).
- [ ] Expiration idle/pending → purge silencieuse, `queue_dropped`
      compte, zero emission en clair.

## 6. Surface residuelle documentee

- [ ] `docs/security/fingerprinting.md` §Mode furtif : limites
      volume/timing/bootstrap/IP-blocking/UDP-throttle listees et
      assumees.
- [ ] `gateway` = exception BitTorrent publique documentee
      (ADR §roles) — la passerelle n'est pas furtive cote Internet.
- [ ] Derive d'horloge : alerte diagnostic UI quand `hs1` echouent
      en boucle (NTP filtre/spoofe en zone censuree) —
      `stealthClockWarning`.

## Verdict

- [ ] Les six sections cochees + `bench_stealth_fingerprint.ps1`
      PASS + revue de code des diffs etapes 49-55 → l'ADR peut
      passer « Acceptee » ; sinon elle reste « Proposee ».
