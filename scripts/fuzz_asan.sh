#!/usr/bin/env bash
# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# fuzz_asan.sh — campagne sanitizers Linux (WSL/CI) du harnais fuzz.
#
# Windows/MSVC n'a aucun runtime ASan : le shim sancov ne couvre que la
# couverture guidee (`-s none`). Linux a les runtimes clang complets —
# cette campagne rejoue les corpus avec AddressSanitizer (inclut Leak)
# et UndefinedBehaviorSanitizer pour traquer UAF/overflows/UB que le
# fuzz coverage-only ne detecte pas.
#
# Prerequis (Linux/WSL) :
#   rustup toolchain install nightly --profile minimal
#   rustup component add --toolchain nightly rust-src
#   cargo install cargo-fuzz ; clang (runtime sanitizer)
#
# Usage :
#   ./scripts/fuzz_asan.sh                     # toutes cibles, 10 min chacune
#   ./scripts/fuzz_asan.sh messaging_window    # une cible, 10 min
#   SAN=address ./scripts/fuzz_asan.sh         # ASan seul (defaut : les deux)
#   SEC=600 ./scripts/fuzz_asan.sh -           # 10 min par cible
#
# Journal : docs/security/fuzz_journal_san.csv (hors git si prive —
# verifier .gitignore avant publication).
set -u
cd "$(dirname "$0")/.."

SEC="${SEC:-600}"
SAN_LIST="${SAN:-address undefined}"
ONLY="${1:-}"
COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo '?')"
JOURNAL="docs/security/fuzz_journal_san.csv"
mkdir -p "$(dirname "$JOURNAL")" fuzz/artifacts
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
        execs="$(grep -oE '#[0-9]+ +(DONE|pulse|REDUCE|NEW)' "$log" |
                 tail -1 | grep -oE '#[0-9]+' | tr -d '#')"
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
