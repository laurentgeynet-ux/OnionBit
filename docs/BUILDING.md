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
# app/build/windows/x64/runner/Release/OnionBit.exe
```

The UI looks for `onionbit-daemon.exe` **next to itself** (or via the
`ONIONBIT_DAEMON_EXE` env var) and starts it automatically. Copy the daemon
into the Release folder for a standalone bundle.

## Packaging (Windows x64)

```sh
# Stage: portable bundle -> dist/OnionBit-<version>-windows-x64/
#   OnionBit.portable   marker file (portable mode)
#   windows/            OnionBit.exe + onionbit-daemon.exe + cli
#   state/  data/       created on first run (see below)
Compress-Archive -Path dist/OnionBit-<version>-windows-x64 `
                 -DestinationPath dist/OnionBit-<version>-windows-x64.zip
```

## Run

- `OnionBit.exe` — graphical app (auto-starts the daemon)
- `onionbit-daemon.exe` — headless daemon; state goes to `state/` next to
  the bundle root, `.onionbit/` elsewhere (configuration.json, api key,
  SQLite db, logs). `--open-webui` also opens the web UI in the default
  browser (used by `OnionBit Web.exe`, the portable root launcher)
- `onionbit-cli.exe --help` — CLI control

Environment overrides: `ONIONBIT_API_KEY`, `ONIONBIT_API`, `ONIONBIT_DAEMON_EXE`.

## Portable bundle (USB key / external drive — ADR-0018)

A packaged build is fully portable: copy the `OnionBit-<os>-<arch>/` folder
to a USB key or external drive and run it from there. All paths are
resolved relative to the bundle root — nothing is persisted as a
machine-specific absolute path (persisted values use `@root/…`,
`@public/…`, `@private/…` specs).

```
OnionBit/
  OnionBit.portable       marker — enables portable layout
  windows/  (linux/, macos/…)   executables per OS
  state/    configuration.json, onionbit.db, identity/, logs/, rqbit/
  data/
    public/   temp/  downloads/  torrents/     — cleartext
    private/  temp/  downloads/  torrents/     — encrypted (OBD),
              manifest.obm  rqbit/               identity-bound
```

Notes for nomad use:

- **Drive letters / mount points may change** — the bundle survives:
  `bench_portable.ps1` verifies copy → restart → identical API/downloads
  plus a zero-absolute-path oracle on persisted artifacts.
- **Private zone needs your identity.** Without the same `state/identity/`,
  `data/private/` is inert (undecryptable `OBM` manifest + `OBD` chunks).
  Keep `identity_seed.bin` safe — or seal it with `identity.at_rest`
  (Settings → Identity); the UI suggests it on removable/ACL-less media.
- **FAT32/exFAT USB keys**: no ACLs — treat the whole bundle as readable
  by whoever holds the key. Enable `identity.at_rest` so the seed is
  password-sealed (the private payload stays encrypted regardless).
- **Linux/macOS `noexec` mounts**: removable filesystems are often mounted
  `noexec` — either remount with `exec` (`sudo mount -o remount,exec …`)
  or copy the binaries (`linux/`, `macos/`) to a local disk and keep only
  `state/`+`data/` on the media via `--state-dir`.

## Other platforms

Linux / macOS / Android / iOS / Web builds will follow — the code targets
them but only Windows x64 is validated and packaged for now.
