#!/usr/bin/env bash
# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# fuzz_asan.sh — campagne sanitizers du harnais fuzz.
#
# Rejoue les corpus avec AddressSanitizer (inclut LeakSanitizer) pour
# traquer UAF/overflows/leaks que le fuzz coverage-only ne detecte
# pas. `undefined` n'est PAS une valeur de `--sanitizer` cargo-fuzz
# (address/leak/memory/thread/none) et rustc n'expose pas de UBSan
# general via -Zsanitizer — la jambe UBSan initiale echouait en exit 2
# avant tout fuzzing (CI 37341222562).
#
# Linux/WSL (ASan + UBSan, runtime fourni par rustup) :
#   rustup toolchain install nightly --profile minimal
#   rustup component add --toolchain nightly rust-src
#   cargo install cargo-fuzz ; clang
#
# Windows/MSVC (ASan seul, runtime externe — la toolchain rustup ne
# livre pas librustc-nightly_rt.asan.a sur cette cible ; UBSan
# indisponible). Recette validee 2026-10-05 (MCP compiler-team#702) :
#   RUSTFLAGS="-Zexternal-clangrt \
#     -Clink-arg=clang_rt.asan_dynamic-x86_64.lib \
#     -Clink-arg=clang_rt.asan_dynamic_runtime_thunk-x86_64.lib"
#   LIB/PATH incluant <LLVM>\lib\clang\<ver>\lib\windows
#   JAMAIS /WHOLEARCHIVE sur le thunk : doublon __start___sancov_*
#   avec sancov_shim.lib ; sans lui, lld ne tire que les objets
#   references. Le DLL clang_rt.asan_dynamic doit etre sur PATH.
#
# Usage :
#   ./scripts/fuzz_asan.sh                     # toutes cibles, 10 min chacune
#   ./scripts/fuzz_asan.sh messaging_window    # une cible, 10 min
#   SAN=address ./scripts/fuzz_asan.sh         # ASan seul (defaut : les deux)
#   SEC=600 ./scripts/fuzz_asan.sh -           # 10 min par cible
#
# Garde : le script echoue si le sanitizer n'est pas reellement arme
# (symboles __asan/__ubsan absents du binaire) — un build sans
# instrumentation ne vaut rien comme validation.
#
# Journal : fuzz/artifacts/fuzz_journal_san.csv (fuzz/artifacts est
# gitignore — remonte en artefact CI).
set -u
cd "$(dirname "$0")/.."

SEC="${SEC:-600}"
# `undefined` retire : non supporte par cargo-fuzz/rustc (voir entete).
SAN_LIST="${SAN:-address}"
ONLY="${1:-}"
COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo '?')"
JOURNAL="fuzz/artifacts/fuzz_journal_san.csv"
mkdir -p fuzz/artifacts
[ -f "$JOURNAL" ] || echo "date,commit,target,san,duree_s,execs,crashes,exit_code" > "$JOURNAL"

TARGETS="raw_datagram tunnel_cell tunnel_payloads ipv8_packet \
unsigned_dispatch utp_datagram messaging_frame messaging_window"
[ -n "$ONLY" ] && [ "$ONLY" != "-" ] && TARGETS="$ONLY"

fails=0
for t in $TARGETS; do
    for san in $SAN_LIST; do
        echo ""
        echo "=== fuzz $t  san=$san  ${SEC}s  commit $COMMIT ==="
        log="fuzz/artifacts/asan-last-run-$t-$san.log"
        cargo +nightly fuzz run -O -s "$san" "$t" \
            -- "-max_total_time=$SEC" "-print_final_stats=1" 2>&1 |
            tee "$log" | tail -12
        code=${PIPESTATUS[0]}
        # Garde « sanitizer arme » : le binaire doit referencer les
        # symboles du runtime. Sans ca, un build non instrumente
        # pourrait passer pour une campagne sanitizers reussie.
        bin="$(ls fuzz/target/*/release/"$t" 2>/dev/null | head -1)"
        case "$san" in
            address)   pat='__asan_' ;;
            undefined) pat='__ubsan_' ;;
            *)         pat="$san" ;;
        esac
        if [ "$code" -eq 0 ] && { [ -z "$bin" ] || \
            ! command -v nm >/dev/null 2>&1 || \
            ! nm "$bin" 2>/dev/null | grep -q "$pat"; }; then
            echo "!! $t/$san : symboles $pat absents — sanitizer non arme"
            code=42
        fi
        # `Done N runs` est emis une fois a la cloture ; les marqueurs
        # `#N\tDONE` utilisent une tabulation (pas un espace).
        execs="$(grep -oE 'Done [0-9]+ runs' "$log" | tail -1 |
                 grep -oE '[0-9]+')"
        crashes="$(find "fuzz/artifacts/$t" -type f 2>/dev/null | wc -l)"
        echo "$(date '+%F %T'),$COMMIT,$t,$san,$SEC,${execs:-0},$crashes,$code" \
            >> "$JOURNAL"
        echo "journal -> $t/$san : execs=${execs:-?} crashes=$crashes code=$code"
        [ "$code" -ne 0 ] && fails=$((fails + 1)) && \
            echo "!! CRASH $t sous $san — artefacts dans fuzz/artifacts/$t"
    done
done
echo ""
echo "Campagne san terminee ($fails echec(s)). Journal : $JOURNAL"
exit "$fails"
