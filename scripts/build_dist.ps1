# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# build_dist.ps1 - Assemble `dist\` : daemon + CLI + UI Windows.
#
# Usage :
#   powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\build_dist.ps1
#   powershell ... -SkipCheck                        (sans le cargo check prealable)
#   powershell ... -Profile debug                    (binaires debug)
#   powershell ... -ZipRelease                       (+ bundle+zip GitHub :
#                                                     dist\OnionBit-<ver>-windows-x64.zip)
#
# Produit `dist\` a la racine du depot (dossier portable, deja ignore par
# git) :
#   onionbit-daemon.exe, onionbit-cli.exe   - backend Rust (plan de controle)
#   onionbit_ui.exe + *.dll + data\        - interface Flutter Windows
#   build-manifest.json                   - version, commit, rustc, date UTC
#
# Pas de script de lancement : `onionbit_ui.exe` demarre le daemon tout
# seul s'il ne tourne pas (daemon_launcher, etape 20) et le daemon vit
# en icone systray (etape 29). Pour arreter : « Quitter » du menu tray
# ou PUT /api/shutdown.
#
# `dist\state\` est cree par onionbit_ui.exe/onionbit-daemon.exe
# (--state-dir) et n'est JAMAIS efface par ce script - c'est la donnee
# utilisateur (base SQLite + telechargements). Seuls les artefacts de
# build connus sont rafraichis.

param(
    [switch]$SkipCheck,
    [ValidateSet("release", "debug")]
    [string]$Profile = "release",
    [switch]$ZipRelease
)

$ErrorActionPreference = "Stop"
$root   = Split-Path -Parent $PSScriptRoot
$app    = Join-Path $root "app"
$dist   = Join-Path $root "dist"
$listen = "127.0.0.1:8085"   # DEFAULT_LISTEN de onionbit-daemon

Push-Location $root
try {
    # -- 1) Daemon + CLI ------------------------------------------------
    if (-not $SkipCheck) {
        Write-Host "== cargo check pre-build ==" -ForegroundColor Cyan
        cargo check --workspace --all-targets --all-features
    }
    Write-Host "== cargo build --profile $Profile (daemon + cli) ==" -ForegroundColor Cyan
    # cargo n'a pas de profil « debug » : c'est « dev » (sortie
    # target\debug — inchangée pour la suite du script).
    $cargoProfile = if ($Profile -eq "release") { "release" } else { "dev" }
    cargo build --profile $cargoProfile -p onionbit-daemon -p onionbit-cli
    if ($LASTEXITCODE -ne 0) { throw "cargo build a echoue ($LASTEXITCODE)" }

    # -- 2) Interface Flutter Windows -----------------------------------
    Write-Host "== flutter build windows ($Profile) ==" -ForegroundColor Cyan
    Push-Location $app
    try {
        flutter pub get | Out-Null
        $flag = if ($Profile -eq "release") { "--release" } else { "--debug" }
        flutter build windows $flag
        if ($LASTEXITCODE -ne 0) { throw "flutter build windows a echoue ($LASTEXITCODE)" }
    } finally {
        Pop-Location
    }

    # -- 3) Assemblage dans dist\ ---------------------------------------
    $cargoOut = Join-Path $root "target\$Profile"
    $flutterOut = Join-Path $app "build\windows\x64\runner\$(@{$true='Release';$false='Debug'}[$Profile -eq 'release'])"
    if (-not (Test-Path "$cargoOut\onionbit-daemon.exe")) {
        throw "onionbit-daemon.exe introuvable dans $cargoOut"
    }
    if (-not (Test-Path "$flutterOut\onionbit_ui.exe")) {
        throw "onionbit_ui.exe introuvable dans $flutterOut"
    }

    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    foreach ($bin in @("onionbit-daemon.exe", "onionbit-cli.exe")) {
        Copy-Item (Join-Path $cargoOut $bin) -Destination $dist -Force
    }
    # Payload Flutter : exe + DLLs + data\ (l'etat utilisateur n'y est pas).
    Copy-Item (Join-Path $flutterOut "*") -Destination $dist -Recurse -Force

    # -- 4) Nettoyage des lanceurs historiques ---------------------------
    # demarrer/arreter n'ont plus lieu d'etre (lancement par l'UI, arret
    # via le systray ou PUT /api/shutdown) — retirer les restes des
    # builds precedents, dont les binaires de l'ere tribler-* (avant le
    # renommage produit en onionbit-*).
    foreach ($f in @("demarrer.cmd", "demarrer.ps1", "arreter.cmd", "arreter.ps1",
                     "tribler-daemon.exe", "tribler-cli.exe", "tribler_ui.exe",
                     "tribler_ui.pdb")) {
        Remove-Item (Join-Path $dist $f) -Force -ErrorAction SilentlyContinue
    }

    # -- 5) Manifest de build -------------------------------------------
    $manifest = @{
        profile   = $Profile
        daemon    = (cargo pkgid -p onionbit-daemon).Split("#")[-1]
        ui        = "onionbit_ui (Flutter windows $Profile)"
        api       = $listen
        commit    = (git rev-parse --short HEAD 2>$null)
        rustc     = (rustc -V)
        built_utc = (Get-Date).ToUniversalTime().ToString("o")
    }
    $manifest | ConvertTo-Json | Set-Content (Join-Path $dist "build-manifest.json")

    # -- 6) Bundle + zip de release GitHub (optionnel) ---------------------
    if ($ZipRelease) {
        Write-Host "== bundle release OnionBit-<ver>-windows-x64 ==" -ForegroundColor Cyan
        $ver     = (cargo pkgid -p onionbit-daemon).Split('#')[-1]
        $bundle  = Join-Path $dist "OnionBit-$ver-windows-x64"
        $zipPath = "$bundle.zip"
        if (Test-Path $bundle)  { Remove-Item $bundle -Recurse -Force }
        if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
        New-Item -ItemType Directory -Force -Path $bundle | Out-Null

        # Payload = contenu de dist\ (exe, dll, data\) — jamais l'etat
        # utilisateur ni les artefacts de release eux-memes.
        Get-ChildItem $dist |
            Where-Object {
                $_.Name -ne 'state' -and
                -not ($_.PSIsContainer -and $_.Name -like 'OnionBit-*') -and
                $_.Name -notlike '*.zip' -and
                $_.Name -notlike 'LISEZMOI*' -and
                $_.Name -ne 'build-manifest.json'
            } | Copy-Item -Destination $bundle -Recurse -Force

        Copy-Item (Join-Path $root 'LICENSE') -Destination $bundle
        Copy-Item (Join-Path $dist 'build-manifest.json') -Destination $bundle
        # LISEZMOI : gabarit versionne (placeholder {{VERSION}}).
        (Get-Content (Join-Path $PSScriptRoot 'dist_lisezmoi.txt') -Raw -Encoding UTF8).
            Replace('{{VERSION}}', $ver) |
            Set-Content (Join-Path $bundle 'LISEZMOI.txt') -Encoding UTF8

        Compress-Archive -Path $bundle -DestinationPath $zipPath -CompressionLevel Optimal
        Write-Host "  Zip release : $zipPath" -ForegroundColor Green
        Write-Host "  SHA-256     : $((Get-FileHash $zipPath -Algorithm SHA256).Hash)"
    }

    Write-Host ""
    Write-Host "Build OK -> dist\" -ForegroundColor Green
    Write-Host "  Lancement  : dist\onionbit_ui.exe (demarre le daemon au besoin)"
    Write-Host "  Arret      : systray « Quitter » ou PUT /api/shutdown"
    Write-Host "  Etat/datas : dist\state\ (conserve entre builds)"
    Get-ChildItem $dist -File | Format-Table Name, Length
} finally {
    Pop-Location
}
