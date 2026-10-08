# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# verify_all.ps1 — validation complete du workspace avant de considerer
# une etape terminee. Cf. AGENTS.md, section "Commandes de validation".
#
# Usage : pwsh -NoProfile -ExecutionPolicy RemoteSigned -File scripts\verify_all.ps1
#
# NOTE : $ErrorActionPreference n'intercepte PAS les codes de sortie
# des commandes natives (cargo) — chaque etape est verifiee via
# $LASTEXITCODE et le script echoue des la premiere non nulle.


# Encodage : sous Windows PowerShell 5.1, forcer UTF-8 (console +
# lectures Get-Content ; les ecritures gardent leur -Encoding explicite
# ou le defaut de l'hote). pwsh 7 est deja UTF-8 : bloc sans effet.
if ($PSVersionTable.PSVersion.Major -lt 7) {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $OutputEncoding = [System.Text.Encoding]::UTF8
    if ($null -eq $PSDefaultParameterValues) { $PSDefaultParameterValues = @{} }
    $PSDefaultParameterValues['Get-Content:Encoding'] = 'UTF8'
}

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

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

Invoke-Step "check_i18n (aucun litteral FR dans app/lib)" {
    pwsh -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot\check_i18n.ps1"
}

# Cibles de l'UI Flutter : l'analyseur et les tests garantissent le
# code commun ; `build web` verrouille la cible web (imports
# conditionnels dart.library.io, transport Fetch, pickers).
Invoke-Step "flutter analyze (app)" {
    Push-Location (Join-Path $root "app")
    try { flutter analyze } finally { Pop-Location }
}

Invoke-Step "flutter test (app)" {
    Push-Location (Join-Path $root "app")
    try { flutter test } finally { Pop-Location }
}

Invoke-Step "flutter build web (app)" {
    Push-Location (Join-Path $root "app")
    try { flutter build web } finally { Pop-Location }
}

Write-Host "Validation complete OK." -ForegroundColor Green
