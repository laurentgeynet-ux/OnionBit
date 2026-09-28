# build_dist.ps1 — Assemble `dist\` : daemon + CLI + UI Windows + lanceur.
#
# Usage :
#   powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\build_dist.ps1
#   powershell ... -SkipCheck                        (sans le cargo check prealable)
#   powershell ... -Profile debug                    (binaires debug)
#
# Produit `dist\` a la racine du depot (dossier portable, deja ignore par
# git) :
#   tribler-daemon.exe, tribler-cli.exe   — backend Rust (plan de controle)
#   tribler_ui.exe + *.dll + data\        — interface Flutter Windows
#   demarrer.cmd                          — lance daemon puis UI
#   arreter.cmd                           — PUT /api/shutdown + filet taskkill
#   build-manifest.json                   — version, commit, rustc, date UTC
#
# `dist\state\` est cree par demarrer.cmd (--state-dir) et n'est JAMAIS
# efface par ce script — c'est la donnee utilisateur (base SQLite +
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
    $demarrer = @"
@echo off
rem demarrer.cmd -- lance le daemon Tribler-Rust puis l'interface.
rem Le daemon partage son etat via %~dp0state (--state-dir) ; l'UI et ce
rem script lisent api.key + api.http_port_running dans
rem state\configuration.json (le port demande peut etre 0 = aleatoire).
setlocal
cd /d "%~dp0"
title Tribler-Rust

rem -- 0) Port API reel connu (configuration.json, sinon $listen) ---
for /f "usebackq delims=" %%P in (``powershell -NoProfile -Command "`$o='$listen'.Split(':')[1]; `$p='%~dp0state\configuration.json'; if(Test-Path `$p){ try { `$j=Get-Content `$p -Raw | ConvertFrom-Json; if([int]`$j.api.http_port_running -gt 0){ `$o=[int]`$j.api.http_port_running } elseif([int]`$j.api.http_port -gt 0){ `$o=[int]`$j.api.http_port } } catch { } }; Write-Output `$o"``) do set "APIPORT=%%P"

rem -- 1) Daemon deja actif ? (toute reponse HTTP = API en vie) ----
powershell -NoProfile -Command "try { Invoke-WebRequest -Uri ('http://127.0.0.1:' + `$env:APIPORT + '/api/events/info') -TimeoutSec 2 -UseBasicParsing | Out-Null; exit 0 } catch { if(`$null -ne `$_.Exception.Response){ exit 0 }; exit 1 }" >nul 2>&1
if errorlevel 1 (
  echo Demarrage du daemon - console minimisee, fermer la fenetre = arreter.
  start "tribler-daemon" /min tribler-daemon.exe --state-dir "%~dp0state"
) else (
  echo Daemon deja actif sur 127.0.0.1:%APIPORT%.
)

rem -- 2) Attente du plan de controle (30 s max) -------------------
rem     Le port est relu a chaque tour : en mode aleatoire
rem     (http_port=0) il n'est connu qu'apres le bind du daemon.
powershell -NoProfile -Command "`$t=Get-Date; while(((Get-Date)-`$t).TotalSeconds -lt 30){ `$o=`$env:APIPORT; `$p='%~dp0state\configuration.json'; if(Test-Path `$p){ try { `$j=Get-Content `$p -Raw | ConvertFrom-Json; if([int]`$j.api.http_port_running -gt 0){ `$o=[string][int]`$j.api.http_port_running } } catch { } }; try { Invoke-WebRequest -Uri ('http://127.0.0.1:' + `$o + '/api/events/info') -TimeoutSec 2 -UseBasicParsing | Out-Null; exit 0 } catch { if(`$null -ne `$_.Exception.Response){ exit 0 }; Start-Sleep -Milliseconds 500 } }; exit 1" >nul 2>&1
if errorlevel 1 (
  echo.
  echo ERREUR : le daemon ne repond pas (port API %APIPORT%).
  echo Consultez la console tribler-daemon pour la cause.
  pause
  exit /b 1
)

rem -- 3) Interface ------------------------------------------------
start "" tribler_ui.exe
endlocal
"@
    Set-Content -Path (Join-Path $dist "demarrer.cmd") -Value $demarrer -Encoding ASCII

    $arreter = @"
@echo off
rem arreter.cmd -- arrete proprement le daemon (PUT /api/shutdown avec
rem la cle api.key de state\configuration.json sur le port reel
rem http_port_running), puis filet de securite taskkill.
powershell -NoProfile -Command "`$o='$listen'.Split(':')[1]; `$k=''; `$p='%~dp0state\configuration.json'; if(Test-Path `$p){ try { `$j=Get-Content `$p -Raw | ConvertFrom-Json; `$k=`$j.api.key; if([int]`$j.api.http_port_running -gt 0){ `$o=[int]`$j.api.http_port_running } } catch { } }; try { Invoke-RestMethod -Method Put -Uri ('http://127.0.0.1:' + `$o + '/api/shutdown') -Headers @{'X-Api-Key'=`$k} -TimeoutSec 3 | Out-Null } catch { }"
timeout /t 3 /nobreak >nul
taskkill /IM tribler-daemon.exe /F >nul 2>&1
echo Daemon arrete.
"@
    Set-Content -Path (Join-Path $dist "arreter.cmd") -Value $arreter -Encoding ASCII

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
