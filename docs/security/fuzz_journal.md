# Journal des campagnes de fuzzing OnionBit

Ce journal trace les campagnes libFuzzer. Le journal machine (une ligne
par cible) est dans `fuzz_journal.csv`, ecrit automatiquement par
`scripts/fuzz_campaign.ps1`.

## Comment lancer

Prerequis : nightly (`rustup toolchain install nightly --profile minimal`
+ `rustup component add --toolchain nightly rust-src`),
`cargo install cargo-fuzz`.

- **Windows/MSVC** : LLVM (`clang_rt.fuzzer-x86_64.lib`, detecte
  automatiquement) + Visual Studio (libs MSVC/SDK). rustc ne livre
  aucun runtime sanitizer pour `windows-msvc` ; la campagne tourne en
  coverage-guiding sans ASan (`-s none`) grace au shim de bornes
  sancov `fuzz/sancov_shim.c` (compile par `fuzz/build.rs`).
- **Linux/WSL** : clang suffit ; ASan actif par defaut.

```powershell
.\scripts\fuzz_campaign.ps1            # campagne de reference (~5 h)
.\scripts\fuzz_campaign.ps1 -Smoke     # 60 s par cible (validation)
.\scripts\fuzz_campaign.ps1 -Target raw_datagram -Minutes 60
```

## Campagne de reference

| Cible | Duree | Surface couverte |
| :--- | :--- | :--- |
| `raw_datagram` | 60 min | dispatcher complet `on_raw_datagram` (communaute reelle) |
| `tunnel_cell` | 60 min | `Cell::parse`, `check_cell_flags`, encrypt/decrypt, `swap_circuit_id` |
| `unsigned_dispatch` | 60 min | messages non signes hors cellule (13/14/17/18 + inconnus) |
| `tunnel_payloads` | 60 min | tous les payloads e2e, `RendezvousInfo`, DHT intro-point |
| `ipv8_packet` | 30 min | enveloppe IPv8 signee/non signee |
| `utp_datagram` | 30 min | header uTP vendored |

## Cycle de traitement d'un crash

```
crash fuzz -> minimisation -> test de regression stable -> correctif
           -> nouvelle campagne
```

- Les crashes vont dans `fuzz/artifacts/<target>/`.
- `cargo +nightly fuzz tmin <target> <artifact>` minimise.
- Le cas minimal rejoint `fuzz_regression.proptest-regressions` (le
  harnais `tests/fuzz_regression.rs` reste le gate CI sur stable).
- Le fix commit le cas ET la reparration dans le meme commit.

## Journal des campagnes

| Date | Commit | Portee | Resultat |
| :--- | :--- | :--- | :--- |
| 2026-10-01 | `9bc4a9d` | smoke 30-60 s x6 cibles (MSVC, `-s none`) | ~98 M execs, 0 crash, corpus amorce |
| (en cours) | `9bc4a9d` | campagne de reference complete | — |
