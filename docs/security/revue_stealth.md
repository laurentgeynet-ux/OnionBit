# Revue de securite du transport stealth (ADR-0017)

Checklist remplie par le relecteur externe le **2026-10-08**. Chaque item
pointe le code et les tests qui font foi et ont ete verifies en execution.

## 1. Fil — forme morphée

- [x] Aucun octet constant inter-runs dans `hs1`/`hs2`/trames
      (`X'`/`Y'` Elligator2, MAC, padding) — `analyze`
      `oracle_no_constants`, `duplicate_datagrams == 0`
      (`stealth::tests::filtre_uniformite_tailles_et_marqueurs`,
      `bench_stealth_fingerprint.ps1`).
- [x] Domain separation `stealth/v1` vs `ext-obf/v1` prouvee par
      rejet croise (`stealth::tests::separation_de_domaine_obf_stealth`,
      `domaines_distincts_cles_different`).
- [x] Entropie fil ~8 bits/octet sur capture reelle
      (`oracle_entropy` = 7,99 bits/o dans `report.json`), pas de
      motif de taille fixe exploitable au-dela de la distribution
      paddee documentee.
- [x] `hs1` rejoue exact → zero reponse supplementaire
      (`stealth_transport::tests::hs1_rejoue_une_seule_reponse`) ; `X'`
      vu = filtre deux fenetres borne
      (`stealth::tests::xprime_filter_deux_fenetres_et_saturation`,
      `xprime_set_max` = 50 000).

## 2. Silence uniforme (anti-probing)

- [x] Rejet identique quelle que soit la cause (mauvaise cle,
      timestamp, replay, table saturee) — aucune response,
      aucun delta d'observable (`probing_garbage_silence_total_et_zero_amplification`,
      `stealth::tests::troncature_toutes_bornes_rejet_uniforme`,
      `hs1_garbage_mauvaise_cle_rejeu_silencieux`).
- [x] Amplification ≤ 1 : jamais de reponse a un datagramme n'ayant
      pas complete `hs1` — ratio 0,0000 sous probing actif
      (`oracle_amplification_le_1` dans `report.json`).
- [x] Budget pre-DH borne : `hs1_per_ip_per_sec` + `hs1_global_per_sec`
      (le par-IP seul est battable par spoofing UDP), files
      `pre_hs_queue_*` bornees (`file_pre_handshake_bornee_et_dest_inconnu_drop`).

## 3. Fail-closed

- [x] `stealth.enabled × ipv8.enabled` refuse au demarrage sur tous
      les chemins (`stealth_stack::tests::stealth_x_ipv8_legacy_refuse`,
      `Session::start`, `start_offline`, stack).
- [x] Aucun fallback `RawUdpTransport` en stealth — le
      `DatagramTransport` stealth ne possede de chemin clair pour
      rien (destinataire inconnu → drop, `send_to` hors session
      → queue `Pending` bornee, test `stealth_aucun_marqueur_legacy_sur_le_fil`).
- [x] Secrets hors config : `bridge_sk` dans `stealth_bridge.key`
      (0600), jamais dans `configuration.json`, jamais loggue,
      jamais expose par `/api/stealth` (compteurs bornes uniquement,
      test `stealth_pont_lien_invitation`, `stealth_endpoints_et_liens_hostiles`).
- [x] `client_allowlist` invalide ou lien `onionbit-bridge://`
      malforme → refus de demarrer (fail-closed, pas d'ignore,
      test `stealth_validation_fail_closed`, `liens_bridge_parse_serialize_hostile`).

## 4. Anti-scraping INTRO

- [x] Sequence graduee verifiable : seed sans ledger, expansion
      gatee reputation, push reserve reciproques ; pairs inconnus
      refuses en silence (`ext::handle_intro`, test `stealth_intro_decouverte_ponts`).
- [x] Table partitionnee : bornes par requete (max 2-3 ponts) / par pair
      / globale, TTL, aucune persistance des intros non sollicitees.
- [x] Intro forgee inerte : le pair annonce doit prouver
      `bridge_pk` au handshake (`stealth_transport::tests::intro_forgee_handshake_inerte`).
- [x] Enumeration bornee : taux de fuite plafonne par
      `intro_seed_max`/`intro_expand_max` par demande + `intro_per_peer_max` en cumul agrege par demandeur.

## 5. Resilience sans oracle

- [x] Restart pont → re-dial en `dial_cooldown_secs` (pas de
      boucle de retries reguliere = motif fingerprintable).
- [x] Rebinding port source client → nouvelle session sans
      panique du serveur (`stealth_transport::tests::nat_rebinding_client_nouvelle_session`).
- [x] Perte/dup/reordonnancement a 5 % → session vit (fenetre de
      rejeu `replay_window` tolerante, tests `trame_rejeu_desordre_et_fenetre`,
      `rejeu_exact_de_trame_et_reordonnancement`).
- [x] Expiration idle/pending → purge silencieuse, `queue_dropped`
      compte, zero emission en clair (`tick` : purge pending/idle, `queue_dropped`).

## 6. Surface residuelle documentee

- [x] `docs/security/fingerprinting.md` §Mode furtif : limites
      volume/timing/bootstrap/IP-blocking/UDP-throttle listees et
      assumees.
- [x] `gateway` = exception BitTorrent publique documentee
      (ADR §roles, `stealth_moteur_direct_refuse_hors_gateway`) —
      la passerelle n'est pas furtive cote Internet.
- [x] Derive d'horloge : alerte diagnostic UI quand `hs1` echouent
      en boucle (NTP filtre/spoofe en zone censuree) —
      `stealthClockWarning` dans `app/lib/l10n/app_en.arb` et `stealth_section.dart`.

## Verdict

- [x] Les six sections cochees + `bench_stealth_fingerprint.ps1`
      PASS + revue de code des diffs etapes 49-55 et tests unitaires
      verts sur tous les crates (`crypto`, `ipv8`, `core`, `api`, `app`)
      → **L'ADR-0017 est validée et passe à « Acceptée »**.

