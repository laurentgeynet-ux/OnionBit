# Building OnionBit

## Prerequisites

- **Rust** stable (≥ 1.80) — https://rustup.rs
- **Flutter** stable — https://flutter.dev (UI only)
- Windows: Visual Studio Build Tools (C++ workload) for the Flutter runner

## Daemon + CLI

```sh
cargo build --release -p onionbit-daemon -p onionbit-cli
# target/release/onionbit-daemon.exe  (daemon, API on 127.0.0.1:8085)
# target/release/onionbit-cli.exe     (control CLI)
```

## Desktop UI (Windows)

```sh
cd app
flutter build windows --release
# app/build/windows/x64/runner/Release/onionbit_ui.exe
```

The UI looks for `onionbit-daemon.exe` **next to itself** (or via the
`ONIONBIT_DAEMON_EXE` env var) and starts it automatically. Copy the daemon
into the Release folder for a standalone bundle.

## Packaging (Windows x64)

```sh
# Stage: UI bundle + daemon + cli + LICENSE + LISEZMOI.txt
# -> dist/onionbit-<version>-windows-x64/
Compress-Archive -Path dist/onionbit-<version>-windows-x64 `
                 -DestinationPath dist/OnionBit-<version>-windows-x64.zip
```

## Run

- `onionbit_ui.exe` — graphical app (auto-starts the daemon)
- `onionbit-daemon.exe` — headless daemon; state goes to `.onionbit/` next to
  the binary (configuration.json, api key, SQLite db, logs)
- `onionbit-cli.exe --help` — CLI control

Environment overrides: `ONIONBIT_API_KEY`, `ONIONBIT_API`, `ONIONBIT_DAEMON_EXE`.

## Other platforms

Linux / macOS / Android / iOS / Web builds will follow — the code targets
them but only Windows x64 is validated and packaged for now.
