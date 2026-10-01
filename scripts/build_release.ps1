# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# build_release.ps1 — build release reproductible du daemon + CLI.
#
# Usage :
#   powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\build_release.ps1
#   powershell ... -Target x86_64-unknown-linux-gnu        (cross, si toolchain installee)
#   powershell ... -Target aarch64-pc-windows-msvc -SkipCheck
#
# Produit `dist/<target>/` contenant les binaires + manifest de build
# (version, commit, rustc, date UTC) — reproductibilite traceable.
#
# Cibles prevues (roadmap etape 17) :
#   x86_64-pc-windows-msvc      Windows x64   (cette machine)
#   aarch64-pc-windows-msvc     Windows ARM64 (toolchain MSVC ARM64 requise)
#   x86_64-unknown-linux-gnu    Linux x64     (rustup target + cc croise)
#   aarch64-apple-darwin        macOS ARM64   (SDK Apple requis — macOS only)
#
# Note : `cargo check --target` sur une cible non native ne suffit pas
# pour rusqlite `bundled` (compile sqlite3.c pour la cible — toolchain
# C croisee requise). Le build reel des cibles non-Windows est prevu
# en CI matricielle.

param(
    [string]$Target = "",
    [switch]$SkipCheck,
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root

try {
    $targets = @("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc",
                 "x86_64-unknown-linux-gnu", "aarch64-apple-darwin")
    if ($Target -eq "") { $Target = (rustc -vV | Select-String "host:").ToString().Split(":")[1].Trim() }
    if ($Target -notin $targets) {
        Write-Warning "cible '$Target' hors matrice prevue ($($targets -join ', '))"
    }

    # Verification prealable (compile la cible hote).
    if (-not $SkipCheck) {
        Write-Host "== cargo check pre-release ==" -ForegroundColor Cyan
        cargo check --workspace --all-targets --all-features
    }

    Write-Host "== cargo build --profile $Profile --target $Target ==" -ForegroundColor Cyan
    # cargo n'a pas de profil « debug » : c'est « dev » (mais la sortie
    # reste sous target\<target>\debug).
    $cargoProfile = if ($Profile -eq "release") { "release" } else { "dev" }
    cargo build --profile $cargoProfile -p onionbit-daemon -p onionbit-cli --target $Target

    $out = Join-Path $root "dist\$Target"
    New-Item -ItemType Directory -Force -Path $out | Out-Null
    $suffix = if ($Target -like "*windows*") { ".exe" } else { "" }
    $srcDir = if ($Profile -eq "release") { "release" } else { "debug" }
    $src = Join-Path $root "target\$Target\$srcDir"
    foreach ($bin in @("onionbit-daemon", "onionbit-cli")) {
        Copy-Item "$src\$bin$suffix" -Destination $out -Force
    }

    $manifest = @{
        target   = $Target
        profile  = $Profile
        version  = (cargo pkgid -p onionbit-daemon).Split("#")[-1]
        commit   = (git rev-parse --short HEAD 2>$null)
        rustc    = (rustc -V)
        built_utc = (Get-Date).ToUniversalTime().ToString("o")
    }
    $manifest | ConvertTo-Json | Set-Content (Join-Path $out "build-manifest.json")
    Write-Host "Build OK -> dist\$Target" -ForegroundColor Green
    Get-ChildItem $out | Format-Table Name, Length
} finally {
    Pop-Location
}
