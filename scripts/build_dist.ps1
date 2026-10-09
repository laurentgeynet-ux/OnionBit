# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# build_dist.ps1 - Assemble `dist\OnionBit\` : bundle portable ADR-0018.
#
# Usage :
#   pwsh -NoProfile -ExecutionPolicy RemoteSigned -File scripts\build_dist.ps1
#   pwsh ... -SkipCheck                        (sans le cargo check prealable)
#   pwsh ... -Profile debug                    (binaires debug)
#   pwsh ... -ZipRelease                       (+ bundle+zip GitHub :
#                                                     dist\OnionBit-<ver>-windows-x64.zip)
#
# Layout produit (etape 58 — une cle USB embarque tous les OS sur le
# meme etat) :
#   dist\OnionBit\
#     OnionBit.exe                    - lanceur UI desktop (principal)
#     OnionBit Daemon.exe             - lanceur backend seul (systray)
#     OnionBit Web.exe                - lanceur daemon + UI web
#                                       (--open-webui)
#     LISEZMOI.txt                    - notice d'utilisation
#     OnionBit.portable               - marqueur : state/ et data/ a la
#                                       racine du bundle
#     windows\onionbit-daemon.exe     - backend Rust (plan de controle)
#     windows\onionbit-cli.exe
#     windows\OnionBit.exe + *.dll + data\  - UI Flutter Windows
#     windows\web\                    - interface web servie sur
#                                       http://127.0.0.1:<port>/
#     build-manifest.json             - version, commit, rustc, date UTC
#     state\, data\                   - crees au premier lancement
#
# Pas de script de lancement : `windows\OnionBit.exe` demarre le daemon
# tout seul s'il ne tourne pas (daemon_launcher, etape 20) et le daemon
# vit en icone systray (etape 29). Pour arreter : « Quitter » du menu
# tray ou PUT /api/shutdown.
#
# `dist\OnionBit\state\` et `dist\OnionBit\data\` sont la donnee
# utilisateur (base SQLite + telechargements) — JAMAIS effaces par ce
# script. Un `dist\state\` historique (layout plat pre-ADR-0018) est
# migre vers `dist\OnionBit\state\` une fois.

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
$root   = Split-Path -Parent $PSScriptRoot
$app    = Join-Path $root "app"
$dist   = Join-Path $root "dist"
$bundleRoot = Join-Path $dist "OnionBit"   # racine portable ADR-0018
$osDir  = Join-Path $bundleRoot "windows"  # payload de l'OS hote
$listen = "127.0.0.1:8085"   # DEFAULT_LISTEN de onionbit-daemon

# Archi Windows de l'hote (x64 | arm64) — detectee depuis le triple
# rustc hote. `flutter build windows` produit nativement dans
# `build\windows\<arch>\` selon l'hote (pas de cross x64->arm64 dans
# l'outil Flutter) : un runner windows-11-arm sort donc du arm64.
$hostTriple = (rustc -vV | Select-String "host:").ToString().Split(":")[1].Trim()
$winArch = if ($hostTriple -like "aarch64*") { "arm64" } else { "x64" }

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
    # onionbit-launcher : les trois lanceurs portables de la racine
    # (copies renommees du meme mini-exe, resolution relative a sa
    # position — aucun chemin de build n'est fige).
    cargo build --profile $cargoProfile -p onionbit-daemon -p onionbit-cli -p onionbit-launcher
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
    $cmakeCache = Join-Path $winBuildDir "$winArch\CMakeCache.txt"
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
    $flutterOut = Join-Path $app "build\windows\$winArch\runner\$(@{$true='Release';$false='Debug'}[$BuildProfile -eq 'release'])"
    if (-not (Test-Path "$cargoOut\onionbit-daemon.exe")) {
        throw "onionbit-daemon.exe introuvable dans $cargoOut"
    }
    if (-not (Test-Path "$flutterOut\OnionBit.exe")) {
        throw "OnionBit.exe introuvable dans $flutterOut"
    }

    # -- 3) Assemblage du bundle portable `dist\OnionBit\` -------------
    # Migration douce du layout plat historique : `dist\state\` et
    # `dist\data\` (le `data\` plat tenait les assets Flutter — purge
    # distinguee ci-dessous) suivent le bundle dans `OnionBit\`.
    New-Item -ItemType Directory -Force -Path $osDir | Out-Null
    if ((Test-Path (Join-Path $dist 'state')) -and
        -not (Test-Path (Join-Path $bundleRoot 'state'))) {
        Write-Host "   migration dist\state\ -> dist\OnionBit\state\" -ForegroundColor Yellow
        Move-Item (Join-Path $dist 'state') (Join-Path $bundleRoot 'state')
    }
    # `dist\data\` historique = assets Flutter du layout plat (le
    # `data\` ADR-0018 nait sous `OnionBit\`) — si rien n'y ressemble
    # a du contenu utilisateur (public/private absents), c'est le
    # payload Flutter : purgee avec les binaires.
    $flatData = Join-Path $dist 'data'
    if ((Test-Path $flatData) -and -not (Test-Path (Join-Path $flatData 'public'))) {
        Remove-Item $flatData -Recurse -Force -ErrorAction SilentlyContinue
    }

    # Marqueur portable : sa presence seule commande le layout
    # (`find_portable_root` remonte depuis l'exe jusqu'au marqueur).
    $marker = Join-Path $bundleRoot "OnionBit.portable"
    if (-not (Test-Path $marker)) {
        "Bundle portable OnionBit (ADR-0018) : state/ et data/ vivent a cote de ce marqueur.`n" +
        "Ne pas supprimer — deplacable tel quel sur cle USB ou disque externe." |
            Set-Content $marker -Encoding UTF8
    }

    foreach ($bin in @("onionbit-daemon.exe", "onionbit-cli.exe")) {
        Copy-Item (Join-Path $cargoOut $bin) -Destination $osDir -Force
    }
    # Payload Flutter : exe + DLLs + data\ (l'etat utilisateur n'y est pas).
    Copy-Item (Join-Path $flutterOut "*") -Destination $osDir -Recurse -Force

    # Interface web : windows\web\ est detecte automatiquement par le
    # daemon (`<exe>/web`) et servi sur http://127.0.0.1:<port>/.
    $webOut = Join-Path $app "build\web"
    if (-not (Test-Path "$webOut\index.html")) {
        throw "build web introuvable dans $webOut"
    }
    $webDist = Join-Path $osDir "web"
    if (Test-Path $webDist) { Remove-Item $webDist -Recurse -Force }
    Copy-Item $webOut -Destination $webDist -Recurse -Force

    # Lanceurs a la RACINE du bundle — trois copies renommees du meme
    # `onionbit-launcher.exe` (~30 Ko) : chacune choisit sa cible d'apres
    # son propre nom et la resout RELATIVEMENT a sa position
    # (`windows\<cible>`). Portable a 100 % — un `.lnk` figerait le
    # chemin absolu du build dans TargetPath et lancerait l'ancienne
    # copie tant que `dist\` existe encore (ou se casserait ailleurs).
    #   OnionBit.exe         → windows\OnionBit.exe (demarre le daemon
    #                          au besoin via daemon_launcher)
    #   OnionBit Daemon.exe  → windows\onionbit-daemon.exe (systray)
    #   OnionBit Web.exe     → windows\onionbit-daemon.exe --open-webui
    # Icone : `onionbit.ico` embarquee dans le lanceur (resources.rc).
    $launcherSrc = Join-Path $cargoOut "onionbit-launcher.exe"
    if (-not (Test-Path $launcherSrc)) {
        throw "onionbit-launcher.exe introuvable dans $cargoOut"
    }
    foreach ($name in @("OnionBit", "OnionBit Daemon", "OnionBit Web")) {
        Copy-Item $launcherSrc `
            -Destination (Join-Path $bundleRoot "$name.exe") -Force
    }
    # Restes de l'ere .lnk : chemins absolus perimes — supprimes plutot
    # que laisses trompeurs a cote des nouveaux lanceurs.
    foreach ($name in @("OnionBit", "OnionBit Daemon", "OnionBit Web")) {
        Remove-Item (Join-Path $bundleRoot "$name.lnk") -Force `
            -ErrorAction SilentlyContinue
    }

    # -- 4) Nettoyage des artefacts historiques -------------------------
    # demarrer/arreter n'ont plus lieu d'etre (lancement par l'UI, arret
    # via le systray ou PUT /api/shutdown) — retirer les restes des
    # builds precedents : binaires de l'ere tribler-*, l'ancien exe UI
    # `onionbit_ui` (renomme OnionBit), les lanceurs .cmd/.ps1 et l'ere
    # .lnk a chemins absolus (remplaces par les lanceurs .exe
    # `onionbit-launcher`). Idem pour le layout plat pre-ADR-0018 :
    # binaires/dlls/web/ a la racine de `dist\`. `onionbit.ico` lache a
    # cote des exe (ere du .lnk) : supprime — sans extension visible il
    # se confondait avec l'application. L'ancien « OnionBit Web.lnk »
    # vivait sous windows\ — les lanceurs .exe sont a la racine du bundle.
    foreach ($f in @("demarrer.cmd", "demarrer.ps1", "arreter.cmd", "arreter.ps1",
                     "tribler-daemon.exe", "tribler-cli.exe", "tribler_ui.exe",
                     "tribler_ui.pdb", "onionbit_ui.exe", "onionbit_ui.pdb",
                     "OnionBit Web.cmd", "web-launch.ps1", "onionbit.ico",
                     "OnionBit.lnk", "OnionBit Daemon.lnk", "OnionBit Web.lnk")) {
        Remove-Item (Join-Path $osDir $f) -Force -ErrorAction SilentlyContinue
        Remove-Item (Join-Path $dist $f) -Force -ErrorAction SilentlyContinue
    }
    # Layout plat obsolescent : tout binaire/dll/web encore a la racine
    # de `dist\` appartient a l'ancienne assemblee (le nouveau payload
    # vit sous `OnionBit\windows\`).
    foreach ($f in @("onionbit-daemon.exe", "onionbit-cli.exe", "OnionBit.exe",
                     "onionbit.ico", "OnionBit.lnk", "OnionBit Daemon.lnk",
                     "OnionBit Web.lnk")) {
        Remove-Item (Join-Path $dist $f) -Force -ErrorAction SilentlyContinue
    }
    Remove-Item (Join-Path $dist "*.dll") -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path $dist "web") -Recurse -Force -ErrorAction SilentlyContinue

    # -- 5) Manifest de build -------------------------------------------
    $manifest = @{
        profile   = $BuildProfile
        daemon    = (cargo pkgid -p onionbit-daemon).Split("#")[-1]
        ui        = "OnionBit.exe (Flutter windows $BuildProfile) + web/"
        layout    = "portable ADR-0018 (OnionBit.portable + <os>/)"
        api       = $listen
        commit    = (git rev-parse --short HEAD 2>$null)
        rustc     = (rustc -V)
        built_utc = (Get-Date).ToUniversalTime().ToString("o")
    }
    $manifest | ConvertTo-Json | Set-Content (Join-Path $bundleRoot "build-manifest.json")

    # LISEZMOI a la racine du bundle, dans tous les builds (pas
    # seulement -ZipRelease) : c'est lui qui indique les trois
    # lanceurs si l'utilisateur ouvre le dossier.
    $ver = (cargo pkgid -p onionbit-daemon).Split('#')[-1]
    (Get-Content (Join-Path $PSScriptRoot 'dist_lisezmoi.txt') -Raw -Encoding UTF8).
        Replace('{{VERSION}}', $ver) |
        Set-Content (Join-Path $bundleRoot 'LISEZMOI.txt') -Encoding UTF8

    # -- 6) Bundle + zip de release GitHub (optionnel) ---------------------
    if ($ZipRelease) {
        Write-Host "== bundle release OnionBit-<ver>-windows-$winArch ==" -ForegroundColor Cyan
        $bundle  = Join-Path $dist "OnionBit-$ver-windows-$winArch"
        $zipPath = "$bundle.zip"
        if (Test-Path $bundle)  { Remove-Item $bundle -Recurse -Force }
        if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
        New-Item -ItemType Directory -Force -Path $bundle | Out-Null

        # Le bundle zip EST la racine portable : marqueur + <os>/ +
        # state/ + data/ (vide) reproduisent le layout ADR-0018.
        Get-ChildItem $bundleRoot |
            Where-Object {
                $_.Name -ne 'state' -and
                $_.Name -ne 'data' -and
                $_.Name -ne 'build-manifest.json'
            } | Copy-Item -Destination $bundle -Recurse -Force
        New-Item -ItemType Directory -Force -Path (Join-Path $bundle 'state') | Out-Null
        New-Item -ItemType Directory -Force -Path (Join-Path $bundle 'data') | Out-Null

        Copy-Item (Join-Path $root 'LICENSE') -Destination $bundle
        Copy-Item (Join-Path $bundleRoot 'build-manifest.json') -Destination $bundle
        # LISEZMOI.txt deja ecrit a la racine du bundle — emporte par
        # le `Get-ChildItem $bundleRoot | Copy-Item` ci-dessus.

        Compress-Archive -Path $bundle -DestinationPath $zipPath -CompressionLevel Optimal
        Write-Host "  Zip release : $zipPath" -ForegroundColor Green
        Write-Host "  SHA-256     : $((Get-FileHash $zipPath -Algorithm SHA256).Hash)"
    }

    Write-Host ""
    Write-Host "Build OK -> dist\OnionBit\ (bundle portable ADR-0018)" -ForegroundColor Green
    Write-Host "  Lancement  : dist\OnionBit\OnionBit.exe (UI desktop, demarre"
    Write-Host "               le daemon au besoin) — aussi OnionBit Daemon.exe et"
    Write-Host "               OnionBit Web.exe a la racine du bundle"
    Write-Host "  UI web     : http://127.0.0.1:8085/ une fois le daemon lance"
    Write-Host "               (cle API injectee automatiquement)"
    Write-Host "  Arret      : systray « Quitter » ou PUT /api/shutdown"
    Write-Host "  Etat/datas : dist\OnionBit\{state,data}\ (conserves entre builds,"
    Write-Host "               transportables sur cle USB tels quels)"
    Get-ChildItem $osDir -File | Format-Table Name, Length
} finally {
    Pop-Location
}
