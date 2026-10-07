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
#   OnionBit.exe + *.dll + data\           - interface Flutter Windows
#   web\                                  - interface web Flutter, servie
#                                           par le daemon sur http://127.0.0.1:<port>/
#   OnionBit Web.lnk                      - raccourci navigateur
#                                           (daemon --open-webui)
#   build-manifest.json                   - version, commit, rustc, date UTC
#
# Pas de script de lancement : `OnionBit.exe` demarre le daemon tout
# seul s'il ne tourne pas (daemon_launcher, etape 20) et le daemon vit
# en icone systray (etape 29). Pour arreter : « Quitter » du menu tray
# ou PUT /api/shutdown.
#
# `dist\state\` est cree par OnionBit.exe/onionbit-daemon.exe
# (--state-dir) et n'est JAMAIS efface par ce script - c'est la donnee
# utilisateur (base SQLite + telechargements). Seuls les artefacts de
# build connus sont rafraichis.

param(
    [switch]$SkipCheck,
    # `$Profile` est une variable automatique PowerShell (chemin du
    # profil utilisateur) — nom interne `BuildProfile`, `-Profile`
    # reste accepte en alias pour la doc existante.
    [Alias("Profile")]
    [ValidateSet("release", "debug")]
    [string]$BuildProfile = "release",
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
    Write-Host "== cargo build --profile $BuildProfile (daemon + cli) ==" -ForegroundColor Cyan
    # cargo n'a pas de profil « debug » : c'est « dev » (sortie
    # target\debug — inchangée pour la suite du script).
    $cargoProfile = if ($BuildProfile -eq "release") { "release" } else { "dev" }
    cargo build --profile $cargoProfile -p onionbit-daemon -p onionbit-cli
    if ($LASTEXITCODE -ne 0) { throw "cargo build a echoue ($LASTEXITCODE)" }

    # -- 2) Interface Flutter Windows -----------------------------------
    Write-Host "== flutter build windows ($BuildProfile) ==" -ForegroundColor Cyan
    # Cache CMake perime : `CMAKE_INSTALL_PREFIX` est gele dans
    # `CMakeCache.txt` au premier configure
    # (`CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT`) avec le
    # genexp `$<TARGET_FILE_DIR:<BINARY_NAME>>` evalue sous le nom
    # de l'epoque. Apres un renommage (ex. `onionbit_ui` ->
    # `OnionBit`), la cible figee n'existe plus et le generate
    # echoue ("No target ..."). Purge du build windows si la cible
    # cachee differe de `OnionBit`.
    $winBuildDir = Join-Path $app "build\windows"
    $cmakeCache = Join-Path $winBuildDir "x64\CMakeCache.txt"
    if (Test-Path $cmakeCache) {
        $m = Select-String -Path $cmakeCache -Pattern `
            'CMAKE_INSTALL_PREFIX:PATH=\$<TARGET_FILE_DIR:(\w+)>' |
            Select-Object -First 1
        $cachedTarget = if ($m) { $m.Matches.Groups[1].Value } else { "" }
        if ($cachedTarget -and $cachedTarget -ne "OnionBit") {
            Write-Host "   purge cache CMake perime (cible '$cachedTarget' != 'OnionBit')" -ForegroundColor Yellow
            Remove-Item $winBuildDir -Recurse -Force
        }
    }
    Push-Location $app
    try {
        flutter pub get | Out-Null
        $flag = if ($BuildProfile -eq "release") { "--release" } else { "--debug" }
        flutter build windows $flag
        if ($LASTEXITCODE -ne 0) { throw "flutter build windows a echoue ($LASTEXITCODE)" }
        # Interface web : servie par le daemon en same-origin (etape 33
        # — le meme codebase Flutter, transport Fetch pour le SSE).
        flutter build web $flag
        if ($LASTEXITCODE -ne 0) { throw "flutter build web a echoue ($LASTEXITCODE)" }
    } finally {
        Pop-Location
    }

    # -- 3) Assemblage dans dist\ ---------------------------------------
    $cargoOut = Join-Path $root "target\$BuildProfile"
    $flutterOut = Join-Path $app "build\windows\x64\runner\$(@{$true='Release';$false='Debug'}[$BuildProfile -eq 'release'])"
    if (-not (Test-Path "$cargoOut\onionbit-daemon.exe")) {
        throw "onionbit-daemon.exe introuvable dans $cargoOut"
    }
    if (-not (Test-Path "$flutterOut\OnionBit.exe")) {
        throw "OnionBit.exe introuvable dans $flutterOut"
    }

    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    foreach ($bin in @("onionbit-daemon.exe", "onionbit-cli.exe")) {
        Copy-Item (Join-Path $cargoOut $bin) -Destination $dist -Force
    }
    # Payload Flutter : exe + DLLs + data\ (l'etat utilisateur n'y est pas).
    Copy-Item (Join-Path $flutterOut "*") -Destination $dist -Recurse -Force

    # Interface web : dist\web\ est detecte automatiquement par le
    # daemon (`<exe>/web`) et servi sur http://127.0.0.1:<port>/.
    $webOut = Join-Path $app "build\web"
    if (-not (Test-Path "$webOut\index.html")) {
        throw "build web introuvable dans $webOut"
    }
    $webDist = Join-Path $dist "web"
    if (Test-Path $webDist) { Remove-Item $webDist -Recurse -Force }
    Copy-Item $webOut -Destination $webDist -Recurse -Force

    # Raccourci navigateur « OnionBit Web.lnk » : cible directe sur
    # `onionbit-daemon.exe --open-webui` — le daemon demarre au besoin
    # (state_dir bundle-aware = <exe>\state) puis ouvre l'URL dans le
    # navigateur par defaut. Un .cmd affichait une fenetre de console ;
    # un .lnk n'en ouvre aucune (binaire en sous-systeme GUI).
    # Icone : le .ico embarque dans l'exe est aussi copie dans dist\
    # — un raccourci pointe plus fiablement un .ico qu'un index de
    # ressource d'exe.
    Copy-Item (Join-Path $root "crates\onionbit-daemon\resources\onionbit.ico") `
        -Destination (Join-Path $dist "onionbit.ico") -Force
    $wsh = New-Object -ComObject WScript.Shell
    $lnk = $wsh.CreateShortcut((Join-Path $dist "OnionBit Web.lnk"))
    $lnk.TargetPath = Join-Path $dist "onionbit-daemon.exe"
    $lnk.Arguments = "--open-webui"
    $lnk.WorkingDirectory = $dist
    $lnk.IconLocation = "$(Join-Path $dist 'onionbit.ico'),0"
    $lnk.Description = "Interface web OnionBit"
    $lnk.Save()

    # -- 4) Nettoyage des lanceurs historiques ---------------------------
    # demarrer/arreter n'ont plus lieu d'etre (lancement par l'UI, arret
    # via le systray ou PUT /api/shutdown) — retirer les restes des
    # builds precedents : binaires de l'ere tribler-*, l'ancien exe UI
    # `onionbit_ui` (renomme OnionBit) et le lanceur .cmd/.ps1 remplace
    # par le raccourci .lnk --open-webui.
    foreach ($f in @("demarrer.cmd", "demarrer.ps1", "arreter.cmd", "arreter.ps1",
                     "tribler-daemon.exe", "tribler-cli.exe", "tribler_ui.exe",
                     "tribler_ui.pdb", "onionbit_ui.exe", "onionbit_ui.pdb",
                     "OnionBit Web.cmd", "web-launch.ps1")) {
        Remove-Item (Join-Path $dist $f) -Force -ErrorAction SilentlyContinue
    }

    # -- 5) Manifest de build -------------------------------------------
    $manifest = @{
        profile   = $BuildProfile
        daemon    = (cargo pkgid -p onionbit-daemon).Split("#")[-1]
        ui        = "OnionBit.exe (Flutter windows $BuildProfile) + web/"
        api       = $listen
        commit    = (git rev-parse --short HEAD 2>$null)
        rustc     = (rustc -V)
        built_utc = (Get-Date).ToUniversalTime().ToString("o")
    }
    $manifest | ConvertTo-Json | Set-Content (Join-Path $dist "build-manifest.json")

    # -- 6) Bundle + zip de release GitHub (optionnel) ---------------------
    if ($ZipRelease) {
        $ver = (cargo pkgid -p onionbit-daemon).Split('#')[-1]
        # Suffixe d'archi depuis la cible hote — le runner windows-11-arm
        # produit nativement du aarch64-pc-windows-msvc.
        $hostTriple = (rustc -vV | Select-String "host:").ToString().Split(":")[1].Trim()
        $winArch = if ($hostTriple -like "aarch64*") { "arm64" } else { "x64" }
        Write-Host "== bundle release OnionBit-<ver>-windows-$winArch ==" -ForegroundColor Cyan
        $bundle  = Join-Path $dist "OnionBit-$ver-windows-$winArch"
        $zipPath = "$bundle.zip"
        if (Test-Path $bundle)  { Remove-Item $bundle -Recurse -Force }
        if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
        New-Item -ItemType Directory -Force -Path $bundle | Out-Null

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
    Write-Host "  Lancement  : dist\OnionBit.exe (demarre le daemon au besoin)"
    Write-Host "  UI web     : dist\`"OnionBit Web.lnk`" ou"
    Write-Host "               http://127.0.0.1:8085/ une fois le daemon lance"
    Write-Host "               (cle API injectee automatiquement)"
    Write-Host "  Arret      : systray « Quitter » ou PUT /api/shutdown"
    Write-Host "  Etat/datas : dist\state\ (conserve entre builds)"
    Get-ChildItem $dist -File | Format-Table Name, Length
} finally {
    Pop-Location
}
