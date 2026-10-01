# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# verify_all.ps1 — validation complete du workspace avant de considerer
# une etape terminee. Cf. AGENTS.md, section "Commandes de validation".
#
# Usage : powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\verify_all.ps1
#
# NOTE : $ErrorActionPreference n'intercepte PAS les codes de sortie
# des commandes natives (cargo) — chaque etape est verifiee via
# $LASTEXITCODE et le script echoue des la premiere non nulle.

$ErrorActionPreference = "Stop"

function Invoke-Step {
    param(
        [Parameter(Mandatory)] [string] $Title,
        [Parameter(Mandatory)] [scriptblock] $Command
    )
    Write-Host "== $Title ==" -ForegroundColor Cyan
    & $Command
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ECHEC ($Title) - code $LASTEXITCODE" -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

Invoke-Step "cargo check (workspace, tous les targets/features)" {
    cargo check --workspace --all-targets --all-features
}

Invoke-Step "cargo clippy (workspace, warnings interdits)" {
    cargo clippy --workspace --all-targets --all-features -- -D warnings
}

Invoke-Step "cargo fmt --check" {
    cargo fmt --all -- --check
}

Invoke-Step "cargo test (workspace)" {
    cargo test --workspace --all-features
}

Write-Host "Validation complete OK." -ForegroundColor Green
