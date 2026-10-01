# Préparation étape 19 — Surface FFI mobile (`tribler-mobile`)

Spécification de la façade FFI avant tout build mobile. Le crate
`tribler-mobile` **n'existe pas encore** — ce document fige le contrat
que l'étape 19 devra implémenter, sans déclarer l'étape commencée.

Références : `docs/plans/mobile_execution_model.md` (contraintes OS,
verdict « faisable avec un hôte FFI minimal »).

## Principes

- **Pas de HTTP** : la façade appelle `CoreSession` directement, dans
  le même processus que l'app hôte. Pas de socket de contrôle locale.
- **Payloads JSON UTF-8** : chaque fonction prend/rend des chaînes
  JSON — les structs de `onionbit-api` (`DownloadInfo`, etc.) sont déjà
  sérialisables ; pas de marshalling de types complexes à travers la
  frontière.
- **Une seule session** : `tribler_start` refuse un second appel tant
  que la session courante n'a pas été arrêtée par `tribler_stop`
  (code `ALREADY_RUNNING`).
- **Runtime Tokio interne** : le crate crée son propre runtime
  multi-thread au `start` ; les fonctions FFI sont synchrones ou
  bloquent sur `block_on` — jamais de future Rust exposée à l'hôte.
- **Propriété mémoire** : toute `*mut c_char` retournée doit être
  libérée par `tribler_free_string`. Pas de `&str` emprunté au-delà
  de l'appel.

## Fonctions

| Fonction | Signature | Sémantique |
| :--- | :--- | :--- |
| `tribler_start` | `(config_json: *const c_char) -> i32` | Crée la session (`CoreSession::start`). `config_json` = sous-ensemble de `CoreConfig` : `state_dir` (obligatoire, `getFilesDir`/`NSDocumentDirectory`), `ipv8.enabled`, `enable_anonymity` (défaut `false` sur mobile). Retourne 0 ou un code `Error`. |
| `tribler_stop` | `() -> i32` | `CoreSession::stop()` — attend la fin du flush fastresume. Idempotent. |
| `tribler_pause_all` | `() -> i32` | `CoreSession::pause_all()` — suspension (iOS `BGProcessingTask` expirant, Android `onTaskRemoved`, Doze). Les erreurs unitaires sont loguées, pas fatales. |
| `tribler_resume_all` | `() -> i32` | `CoreSession::resume_all()` — réactivation. Ne débloque pas une lane dont le kill switch est engagé (erreur collectée, autres moteurs repris). |
| `tribler_add_download` | `(json: *const c_char) -> *mut c_char` | Corps `{"uri"|"torrent","destination","paused","anon_hops","safe_seeding"}` — même schéma que `PUT /api/downloads`. Retourne `{"infohash": "…"}` ou `{"error": …}`. |
| `tribler_list_downloads` | `() -> *mut c_char` | `{"downloads": [DownloadInfo…]}` — mêmes champs que l'API REST (dont `eta` float, `num_seeds`, `hops`). |
| `tribler_download_action` | `(infohash, action: *const c_char) -> i32` | `action` ∈ `"pause"|"resume"|"remove"|"remove_with_data"`. |
| `tribler_set_event_callback` | `(cb: extern fn(*const c_char)) -> i32` | Enregistre le callback de notifications — voir §Notifications. `null` = désabonnement. |
| `tribler_free_string` | `(*mut c_char)` | Libère une chaîne retournée par la façade. |

## Notifications

Le `Notifier` interne est branché sur le callback FFI au `start` :
chaque `Notification` est sérialisée en
`{"topic": "<nom Python>", "kwargs": {…}}` — mêmes topics que les
trames SSE (`events_start`, `download_state_changed`,
`torrent_finished`, `torrent_health_updated`,
`tribler_shutdown_started`, `new_torrent_metadata_created`).

Règles :

- Le callback est invoqué depuis une tâche Tokio interne — **thread
  arbitraire** : l'hôte doit marshaler vers son thread UI (il ne peut
  pas toucher l'UI directement dans le callback).
- Le callback doit être rapide : il bloque le `Notifier`. Travail
  lourd = copier la chaîne et retourner.
- Aucun événement après `tribler_stop` (le callback est libéré avant
  la fin du `stop`).

## Règles de cycle de vie

| Événement OS | Action hôte | Action façade |
| :--- | :--- | :--- |
| Suspension (iOS background, Android `onStop` du service) | appeler `tribler_pause_all` | pause moteur, sockets inchangées (l'OS les gèle) |
| Réactivation | `tribler_resume_all` | reprise ; les circuits morts restent couverts par le kill switch scopé `circuits` — une lane sans circuit READY ne reprend pas |
| Destruction du service / expiration BGTask | `tribler_stop` | flush fastresume + arrêt des stacks ; borné à ~2 s |
| Kill sans préavis | — | fastresume déjà écrit périodiquement → restauration au prochain `start` |

- **Anonymat** : `enable_anonymity` défaut `false` sur mobile ; si
  activé, `resume_all` ne force jamais une lane sans circuit —
  comportement déjà garanti par le watchdog `circuits`.
- **Pas de seeding permanent ni d'exit node** (impossible sur mobile).

## Codes d'erreur

`i32` de retour : `0` = OK ; `-1` = JSON invalide ; `-2` =
`ALREADY_RUNNING`/`NOT_RUNNING` ; `-3` = erreur session
(détail dans la chaîne retournée quand la fonction rend du JSON,
sinon via `tribler_last_error()` — à décider à l'implémentation ;
le choix recommandé est la chaîne JSON `{"error": …}` partout).

## Binding

- **Android** : UniFFI (Kotlin) ou JNI direct — décision laissée à
  l'étape 19 ; la surface ci-dessus est compatible avec les deux
  (fonctions plates, chaînes JSON).
- **iOS** : `cbindgen`/`uniffi` → en-tête C, `.a` statique liée à
  l'app.

## Checklist de l'étape 19 (non commencée)

- [ ] Créer `crates/tribler-mobile` avec cette surface (stubs +
  tests loopback dans un processus de test, pas de device).
- [ ] `cargo-ndk`/`cargo build --target aarch64-linux-android` sur
  toolchain disponible.
- [ ] iOS : `aarch64-apple-ios` — nécessite macOS + Xcode.
- [ ] App hôte minimale (foreground-service Android / BGTask iOS).
