# build_dist.ps1 - Assemble `dist\` : daemon + CLI + UI Windows + lanceur.
#
# Usage :
#   powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\build_dist.ps1
#   powershell ... -SkipCheck                        (sans le cargo check prealable)
#   powershell ... -Profile debug                    (binaires debug)
#
# Produit `dist\` a la racine du depot (dossier portable, deja ignore par
# git) :
#   tribler-daemon.exe, tribler-cli.exe   - backend Rust (plan de controle)
#   tribler_ui.exe + *.dll + data\        - interface Flutter Windows
#   demarrer.cmd                          - lance daemon puis UI
#   arreter.cmd                           - PUT /api/shutdown + filet taskkill
#   build-manifest.json                   - version, commit, rustc, date UTC
#
# `dist\state\` est cree par demarrer.cmd (--state-dir) et n'est JAMAIS
# efface par ce script - c'est la donnee utilisateur (base SQLite +
# telechargements). Seuls les artefacts de build connus sont rafraichis.

param(
    [switch]$SkipCheck,
    [ValidateSet("release", "debug")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"
$root   = Split-Path -Parent $PSScriptRoot
$app    = Join-Path $root "app"
$dist   = Join-Path $root "dist"
$listen = "127.0.0.1:8085"   # DEFAULT_LISTEN de tribler-daemon

Push-Location $root
try {
    # -- 1) Daemon + CLI ------------------------------------------------
    if (-not $SkipCheck) {
        Write-Host "== cargo check pre-build ==" -ForegroundColor Cyan
        cargo check --workspace --all-targets --all-features
    }
    Write-Host "== cargo build --profile $Profile (daemon + cli) ==" -ForegroundColor Cyan
    cargo build --profile $Profile -p tribler-daemon -p tribler-cli

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
    if (-not (Test-Path "$cargoOut\tribler-daemon.exe")) {
        throw "tribler-daemon.exe introuvable dans $cargoOut"
    }
    if (-not (Test-Path "$flutterOut\tribler_ui.exe")) {
        throw "tribler_ui.exe introuvable dans $flutterOut"
    }

    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    foreach ($bin in @("tribler-daemon.exe", "tribler-cli.exe")) {
        Copy-Item (Join-Path $cargoOut $bin) -Destination $dist -Force
    }
    # Payload Flutter : exe + DLLs + data\ (l'etat utilisateur n'y est pas).
    Copy-Item (Join-Path $flutterOut "*") -Destination $dist -Recurse -Force

    # -- 4) Lanceurs -----------------------------------------------------
    # Les .cmd sont des wrappers minimalistes vers les .ps1 : toute la
    # logique vit en PowerShell (la syntaxe `for /f` + quoting emboite
    # de cmd est trop fragile pour lire configuration.json).
    # Le port reel et la cle API sont relus depuis
    # `state\configuration.json` (api/http_port_running, api/key) :
    # compatible avec http_port=0 (port aleatoire, parite Python) et
    # l'authentification X-Api-Key exigee meme en loopback.
    $demarrerCmd = @"
@echo off
rem demarrer.cmd -- lance le daemon Tribler-Rust puis l'interface.
rem Delegue toute la logique a demarrer.ps1 (port reel + cle API lus
rem dans state\configuration.json).
cd /d "%~dp0"
title Tribler-Rust
powershell -NoProfile -ExecutionPolicy RemoteSigned -File "%~dp0demarrer.ps1"
if errorlevel 1 pause
"@
    Set-Content -Path (Join-Path $dist "demarrer.cmd") -Value $demarrerCmd -Encoding ASCII

    $demarrerPs1 = @'
# demarrer.ps1 -- lance tribler-daemon puis tribler_ui.
# Le port API reel (api/http_port_running) et la cle (api/key) vivent
# dans state\configuration.json - relus a chaque sonde car le port
# demande peut etre 0 = aleatoire (parite Python). Toute reponse HTTP,
# y compris 401, signifie "API en vie".
$dist = Split-Path -Parent $MyInvocation.MyCommand.Path
$stateDir = Join-Path $dist 'state'
$configPath = Join-Path $stateDir 'configuration.json'
$defaultPort = 8085   # port historique du daemon (fallback sans config)

function Get-ApiPort {
    $port = $defaultPort
    if (Test-Path $configPath) {
        try {
            $api = (Get-Content $configPath -Raw | ConvertFrom-Json).api
            if ([int]$api.http_port_running -gt 0) { $port = [int]$api.http_port_running }
            elseif ([int]$api.http_port -gt 0) { $port = [int]$api.http_port }
        } catch { }
    }
    return $port
}

function Test-ApiAlive([int]$port) {
    try {
        Invoke-WebRequest -Uri "http://127.0.0.1:$port/api/events/info" `
            -TimeoutSec 2 -UseBasicParsing | Out-Null
        return $true
    } catch {
        return ($null -ne $_.Exception.Response)
    }
}

$port = Get-ApiPort
if (-not (Test-ApiAlive $port)) {
    Write-Host 'Demarrage du daemon - console minimisee, fermer la fenetre = arreter.'
    Start-Process -FilePath (Join-Path $dist 'tribler-daemon.exe') `
        -ArgumentList '--state-dir', "`"$stateDir`"" -WindowStyle Minimized
} else {
    Write-Host "Daemon deja actif sur 127.0.0.1:$port."
}

$deadline = (Get-Date).AddSeconds(30)
$alive = $false
while ((Get-Date) -lt $deadline) {
    $port = Get-ApiPort
    if (Test-ApiAlive $port) { $alive = $true; break }
    Start-Sleep -Milliseconds 500
}
if (-not $alive) {
    Write-Host ''
    Write-Host "ERREUR : le daemon ne repond pas (port API $port)."
    Write-Host 'Consultez la console tribler-daemon pour la cause.'
    exit 1
}

Start-Process -FilePath (Join-Path $dist 'tribler_ui.exe')
'@
    Set-Content -Path (Join-Path $dist "demarrer.ps1") -Value $demarrerPs1 -Encoding ASCII

    $arreterCmd = @"
@echo off
rem arreter.cmd -- arrete proprement le daemon (PUT /api/shutdown
rem authentifie via state\configuration.json), puis filet taskkill.
powershell -NoProfile -ExecutionPolicy RemoteSigned -File "%~dp0arreter.ps1"
timeout /t 3 /nobreak >nul
taskkill /IM tribler-daemon.exe /F >nul 2>&1
echo Daemon arrete.
"@
    Set-Content -Path (Join-Path $dist "arreter.cmd") -Value $arreterCmd -Encoding ASCII

    $arreterPs1 = @'
# arreter.ps1 -- PUT /api/shutdown authentifie (X-Api-Key lue dans
# state\configuration.json, port reel api/http_port_running).
$dist = Split-Path -Parent $MyInvocation.MyCommand.Path
$configPath = Join-Path $dist 'state\configuration.json'
$port = 8085
$key = ''
if (Test-Path $configPath) {
    try {
        $api = (Get-Content $configPath -Raw | ConvertFrom-Json).api
        $key = [string]$api.key
        if ([int]$api.http_port_running -gt 0) { $port = [int]$api.http_port_running }
    } catch { }
}
try {
    Invoke-RestMethod -Method Put -Uri "http://127.0.0.1:$port/api/shutdown" `
        -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 3 | Out-Null
} catch { }
'@
    Set-Content -Path (Join-Path $dist "arreter.ps1") -Value $arreterPs1 -Encoding ASCII

    # -- 5) Manifest de build -------------------------------------------
    $manifest = @{
        profile   = $Profile
        daemon    = (cargo pkgid -p tribler-daemon).Split("#")[-1]
        ui        = "tribler_ui (Flutter windows $Profile)"
        api       = $listen
        commit    = (git rev-parse --short HEAD 2>$null)
        rustc     = (rustc -V)
        built_utc = (Get-Date).ToUniversalTime().ToString("o")
    }
    $manifest | ConvertTo-Json | Set-Content (Join-Path $dist "build-manifest.json")

    Write-Host ""
    Write-Host "Build OK -> dist\" -ForegroundColor Green
    Write-Host "  Lancement  : dist\demarrer.cmd"
    Write-Host "  Arret      : dist\arreter.cmd"
    Write-Host "  Etat/datas : dist\state\ (conserve entre builds)"
    Get-ChildItem $dist -File | Format-Table Name, Length
} finally {
    Pop-Location
}
