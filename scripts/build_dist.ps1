# build_dist.ps1 - Assemble `dist\` : daemon + CLI + UI Windows.
#
# Usage :
#   powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\build_dist.ps1
#   powershell ... -SkipCheck                        (sans le cargo check prealable)
#   powershell ... -Profile debug                    (binaires debug)
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
    [string]$Profile = "release"
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
    # builds precedents.
    foreach ($f in @("demarrer.cmd", "demarrer.ps1", "arreter.cmd", "arreter.ps1")) {
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

    Write-Host ""
    Write-Host "Build OK -> dist\" -ForegroundColor Green
    Write-Host "  Lancement  : dist\onionbit_ui.exe (demarre le daemon au besoin)"
    Write-Host "  Arret      : systray « Quitter » ou PUT /api/shutdown"
    Write-Host "  Etat/datas : dist\state\ (conserve entre builds)"
    Get-ChildItem $dist -File | Format-Table Name, Length
} finally {
    Pop-Location
}
