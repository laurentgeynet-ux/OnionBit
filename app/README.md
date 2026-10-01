# onionbit_ui

Interface Flutter d'OnionBit (Windows, Linux, macOS, Android, iOS, Web).

Elle consomme exclusivement l'API REST + SSE de `onionbit-daemon`
(jamais d'accès direct aux crates `onionbit-*`).

## Lancement (développement)

```powershell
# daemon (un autre terminal)
cargo run -p onionbit-daemon

# UI
cd app
flutter run -d windows
```

La clef d'API est résolue automatiquement depuis le répertoire d'état du
daemon (fichier `api/key` + `api/http_port_running`), comme la GUI
Tribler. Surcharge possible via `ONIONBIT_API` / `ONIONBIT_API_KEY`.
