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
| `ext_packet` | 30 min | trame d'enveloppe communautaire etendue |
| `stealth_frame` | 30 min | trames de couverture stealth |
| `messaging_frame` | 30 min | trame messagerie e2e `Frame::seal/open` |
| `messaging_window` | 30 min | fenetre de replay/sequencement messagerie |
| `obd_file` | 30 min | fichiers prives `OBD` (enveloppe + AEAD) |
| `bencode` | 30 min | parseur bencode : trames, `.torrent`, `mdblob`, trackers |
| `mdblob` | 30 min | catalogue `mdblob` brut + LZ4-frame (chemin `.onion`) + verif signature |
| `onion_link` | 30 min | preambule `OBMS` + trames `u32` BE du lien messagerie `.onion` |
| `onion_http` | 30 min | parseurs de requetes HTTP des services `.onion` (canaux + coffre) |
| `crypto_blobs` | 30 min | `obm::open_manifest`, `pair_open_in` (OBV1/obox), `PeerAddress`/`onion_host` |

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
| 2026-10-01→02 | `9bc4a9d`* | campagne de reference complete (~5 h) | **6,28 Md execs, 0 crash, 0 timeout, 0 OOM** |
| 2026-10-18 | `7c8582b` | smoke 60 s x5 nouvelles cibles ADR-0029 (MSVC, `-s none`) | **23,6 M execs, 0 crash** |

Campagne de reference — detail par cible (journal machine :
`fuzz_journal.csv`) :

| Cible | Duree | Executions | Corpus final |
| :--- | :--- | :--- | :--- |
| `raw_datagram` | 3600 s | 170 434 846 | 8 |
| `tunnel_cell` | 3600 s | 1 832 162 135 | 9 |
| `unsigned_dispatch` | 3600 s | 575 435 542 | 372 |
| `tunnel_payloads` | 3600 s | 590 848 896 | 636 |
| `ipv8_packet` | 1800 s | 1 495 599 165 | 17 |
| `utp_datagram` | 1800 s | 1 616 991 390 | 198 |

*Commit journalise = HEAD au lancement du script. `raw_datagram` a
ete fuzzee sur le binaire `9bc4a9d` exact ; les cibles suivantes ont
ete relinkees a HEAD lors de leur build (`f5d503c`→`9141bf9` : code
guard desactive par defaut, hors surfaces fuzzees — comportement des
entrees fuzzees identique). Cf. note de provenance ci-dessous.

Incident 2026-10-01 : une instance du script lancee en double a ete
tuee avant son premier run ; sa ligne CSV parasite
(`f5d503c,raw_datagram,exit=-1`) a ete retiree — aucun run valide
n'existait pour elle. La campagne d'origine n'a pas ete interrompue.
Note de provenance : les cibles lancees apres `77e2b2e`/`f5d503c`
sont relinkees sur le HEAD courant (`cargo fuzz run` recompile si les
sources changent) ; le code guard ajoute est desactive par defaut et
inaccessible aux surfaces fuzzees — comportement identique, mais le
commit journalise (capture au lancement) reste `9bc4a9d`.

## Extension ADR-0029 (2026-10-18)

Cinq cibles ajoutees pour couvrir les surfaces creees par le chantier
`.onion` (etapes 122-127) et les parseurs hostiles historiques non
encore fuzzes :

| Cible | Executions (60 s) | Corpus | Resultat |
| :--- | :--- | :--- | :--- |
| `bencode` | 3 602 724 | 496 | 0 crash |
| `mdblob` | 102 181 | 139 | 0 crash |
| `onion_link` | 10 887 297 | 16 | 0 crash |
| `onion_http` | 9 263 237 | 139 | 0 crash |
| `crypto_blobs` | 215 295 | 241 | 0 crash |

Notes :

- `mdblob` et `crypto_blobs` ont un debit plus faible — decompression
  LZ4 et verification Ed25519/AEAD par execution ; la couverture reste
  atteinte (ft 494/832) car le corpus converge vite sur les formes
  valides.
- `onion_http` lie `onionbit-core` (parseurs non gates `hs` — le corps
  de requete est purement textuel).
- Windows/MSVC : `sccache` echoue a spawner rustc sur le `librqbit`
  vendored (ligne de commande trop longue) — lancer avec
  `RUSTC_WRAPPER=` vide si le build release s'interrompt sur
  `librqbit`.
- `-Smoke` couvre desormais les 16 cibles ; la campagne de reference
  monte a ~10 h.
