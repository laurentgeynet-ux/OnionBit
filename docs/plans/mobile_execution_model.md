# Étape 18 — Modèle d'exécution mobile (Android / iOS)

Étude des contraintes d'exécution avant tout build mobile (roadmap
Étape 18). Conclusion : le daemon Rust **ne peut pas** tourner en
permanence en arrière-plan sur mobile — le modèle à viser est un
**service de premier plan sous contrôle de l'OS**.

## Contraintes OS

| | Android | iOS |
| :--- | :--- | :--- |
| Processus d'arrière-plan | tué par Doze/App Standby ; `FOREGROUND_SERVICE` obligatoire pour du travail continu | `BGTaskScheduler` (fenêtres courtes, ~30 s) + modes `background` limités ; pas de daemon permanent |
| Sockets | autorisés en `FOREGROUND_SERVICE` avec notification | sockets fermés hors premier plan ; `voip`/`fetch` réservés aux cas d'usage déclarés |
| Réseau mobile | quotas + batterie : le tunnel IPv8 multi-sauts est coûteux | idem ; `allowsExpensiveNetworkAccess` à gérer |
| Build | `cargo ndk` → `.so` chargé par une app JNI | `cargo build --target aarch64-apple-ios` → `.a` lié dans une app Xcode / `cargo-bundle` |

## Architecture recommandée

```
App Flutter (UI)
   │  JNI/FFI (Android) · FFI (iOS)
   ▼
tribler-mobile (nouveau crate, façade FFI)
   │  appels directs (pas de HTTP — même processus)
   ▼
onionbit-core / onionbit-daemon(mode embedded)
```

- **Pas de serveur HTTP local** sur mobile : l'API REST/SSE devient
  une façade FFI (`start`, `stop`, `list_downloads`, `events` →
  callback Dart). `CoreSession` est déjà `Send`/`Clone` — réutilisable
  tel quel.
- **Service Android** : `FOREGROUND_SERVICE` + `PARTIAL_WAKE_LOCK`
  optionnel ; réception des kills par `onTaskRemoved` →
  `CoreSession::stop()` propre.
- **iOS** : `BGProcessingTask` pour les fenêtres de téléchargement ;
  suspension = `session.pause_all()` (nouvel appel à ajouter) +
  reprise sur réactivation. Pas de seeding permanent.
- **Tunnels anonymes** : les maintenir sur mobile coûte trop
  (batterie + données) — `ipv8.enable_anonymity` par défaut `false`
  sur mobile, activable par l'utilisateur (cf. Tribler mobile).

## Adaptations requises du backend

| Changement | Effort | Note |
| :--- | :--- | :--- |
| `tribler-mobile` crate façade FFI (UniFFI ou `cbindgen`) | moyen | expose `CoreSession` + `Notification` en callbacks — contrat figé dans `docs/plans/mobile_ffi_surface.md` |
| `CoreSession::pause_all`/`resume_all` | **fait** | implémentés (tous moteurs, erreurs collectées, kill switch respecté) — test `lifecycle::pause_all_resume_all` |
| Callbacks FFI pour `Notifier` (au lieu de SSE) | faible | même bus d'événements |
| `state_dir` mobile (`getFilesDir`/`NSDocumentDirectory`) | trivial | paramètre de config |
| `ipv8` offline par défaut sur mobile | trivial | `Ipv8Config::enabled = false` |
| uTP-only optionnel (mobile réseau NAT) | fait | `EngineConfig::utp_only` existe déjà |

## Ce qui ne peut pas être porté

- **Seeding permanent** (les OS tuent le process).
- **Exit node / relais** (pas de socket d'écoute stable entrante).
- **Watch-folder** (pas de démon de fond) — remplacé par un scan
  ponctuel à l'ouverture.

## Checklist avant Étape 19 (builds mobiles)

- [ ] `cargo-ndk` + NDK installé ; `aarch64-linux-android` target
- [ ] macOS + Xcode pour `aarch64-apple-ios` (cross impossible depuis Windows)
- [ ] façade FFI `tribler-mobile` créée et testée sur loopback —
  surface figée dans `mobile_ffi_surface.md` (fonctions plates JSON,
  callback notifications, règles suspension/arrêt)
- [ ] app hôte minimale (Android foreground-service / iOS BGTask)

## Verdict

Le moteur BitTorrent + overlay IPv8 compilables pour mobile sont
**faisables** ; le *comportement* change (pas de daemon permanent, pas
d'anonymat par défaut, pas de seeding continu). L'étape 19 se fera
avec un hôte FFI minimal, pas avec `onionbit-daemon` tel quel.
