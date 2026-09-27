# interop_tribler.ps1 - echange reproductible Rust <-> Tribler 8.4.3
# installe (`C:\Program Files (x86)\Tribler\Tribler.exe -s`).
#
# Jalon du roadmap : l'interop avec le vrai client Tribler. Tribler
# stocke `peer_flags = {RELAY, SPEED_TEST}` (son `exitnode_enabled`
# n'est pas exposable par configuration) : il RELAIE mais ne SORT
# pas. Le scenario valide donc :
#   1. `introduction-request` -> `introduction-response` sur le
#      prefixe `TriblerTunnelCommunity` (a3591a6b...d6bc) avec les
#      `ExtraIntroductionPayload.flags` de Tribler (suivi des flags).
#   2. `create` -> `created` Rust -> Tribler (DH + auth + cles de
#      session avec le vrai client).
#   3. `extend` via Tribler comme RELAI vers une sortie Rust, puis
#      datagramme "uTP" a travers le circuit 2 sauts et echo retour
#      (crypto par couches traversant le client reel).
#
# Tribler tourne avec un APPDATA isole (aucun trafic vers
# l'exterieur : bootstrappeurs vides, upnp/natpmp/lsd/dht off).
#
# Usage : powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\interop_tribler.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$triblerExe = if ($env:TRIBLER_EXE) { $env:TRIBLER_EXE } else { "C:\Program Files (x86)\Tribler\Tribler.exe" }
$py = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
$outDir = Join-Path $root "target\interop-tribler"
$stateDir = Join-Path $outDir "tribler-state"   # TSTATEDIR -> racine d'etat isolee
$confDir = Join-Path $stateDir "8.0"            # VERSION_SUBDIR
$confFile = Join-Path $confDir "configuration.json"
New-Item -ItemType Directory -Force -Path $confDir | Out-Null

$triblerPort = 22090   # interface UDPIPv4 de Tribler
$apiPort = 23100       # REST API (juste pour la disponibilite)
$apiKey = "interoptest"
$echoPort = 22091      # echo UDP (destination de sortie)
$rsLog = Join-Path $outDir "rust_tribler_packets.log"
$pem = Join-Path $stateDir "ec_multichain.pem"   # genere par Tribler au 1er demarrage

# Config minimale : les cles absentes retombent sur les defauts
# (DEFAULT_CONFIG est calcule avec NOTRE APPDATA, donc les chemins de
# cles pointent bien dans l'etat isole).
$conf = @{
    api = @{
        http_enabled = $true
        http_port = $apiPort
        http_host = "127.0.0.1"
        https_enabled = $false
        key = $apiKey
    }
    # ipv8_service.py accede directement a ["logger"], ["interfaces"],
    # ["overlays"] (pas de fusion partielle) : la section doit etre
    # complete pour les cles lues en direct.
    ipv8 = @{
        logger = @{ level = "INFO" }
        interfaces = @(
            @{ interface = "UDPIPv4"; ip = "0.0.0.0"; port = $triblerPort }
        )
        walker_interval = 5.0
        overlays = @(
            @{
                class = "DiscoveryCommunity"
                key = "anonymous id"
                walkers = @()
                bootstrappers = @(
                    @{ class = "DispersyBootstrapper"; init = @{ ip_addresses = @(); dns_addresses = @(); bootstrap_timeout = 1.0 } }
                )
                initialize = @{}
                on_start = @()
            }
        )
    }
    libtorrent = @{
        port = 0
        utp = $true
        dht = $false
        upnp = $false
        natpmp = $false
        lsd = $false
    }
    tunnel_community = @{ enabled = $true; min_circuits = 0; max_circuits = 8 }
    content_discovery = @{ enabled = $false }
    dht_discovery = @{ enabled = $false }
    recommender = @{ enabled = $false }
    rendezvous = @{ enabled = $false }
    rss = @{ enabled = $false }
    torrent_checker = @{ enabled = $false }
    versioning = @{ enabled = $false }
    watch_folder = @{ enabled = $false }
    statistics = $false
}
# IMPORTANT : ecrire SANS BOM — `json.load` de TriblerConfigManager
# echouerait sinon et repartirait sur DEFAULT_CONFIG (bootstrappeurs
# reels, api/http_port=0...).
$json = $conf | ConvertTo-Json -Depth 10
[System.IO.File]::WriteAllText($confFile, $json, [System.Text.UTF8Encoding]::new($false))

Write-Host "== build tribler_relay_interop (rust) =="
cargo build -p tribler-tunnel --example tribler_relay_interop
if ($LASTEXITCODE -ne 0) { throw "build echoue" }

# Nettoyage d'un eventuel Tribler de test restant + echo.
Get-Process -Name "Tribler" -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $triblerExe } | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

Write-Host "== Tribler.exe -s (etat isole : $stateDir, ipv8 :$triblerPort, api :$apiPort) =="
# Hooks officiels de run_tribler.py : TSTATEDIR = racine d'etat,
# CORE_API_PORT/CORE_API_KEY = API REST.
$env:TSTATEDIR = $stateDir
$env:CORE_API_PORT = "$apiPort"
$env:CORE_API_KEY = $apiKey
$triblerProc = Start-Process -FilePath $triblerExe -PassThru -NoNewWindow `
    -ArgumentList "-s","--log-level","INFO" `
    -RedirectStandardOutput (Join-Path $outDir "tribler_stdout.log") `
    -RedirectStandardError (Join-Path $outDir "tribler_stderr.log")

$echoProc = Start-Process -FilePath $py -PassThru -NoNewWindow `
    -ArgumentList "`"$PSScriptRoot\interop\udp_echo.py`" --port $echoPort" `
    -RedirectStandardOutput (Join-Path $outDir "echo_stdout.log") `
    -RedirectStandardError (Join-Path $outDir "echo_stderr.log")

try {
    # Attendre que l'API REST soit en ligne (max ~90s : premier
    # demarrage = generation des cles + DB).
    $deadline = (Get-Date).AddSeconds(90)
    $apiUp = $false
    while ((Get-Date) -lt $deadline -and -not $apiUp) {
        try {
            $r = Invoke-RestMethod -Uri "http://127.0.0.1:$apiPort/api/ipv8/overlays" `
                -Headers @{ "X-Api-Key" = $apiKey } -TimeoutSec 3
            $apiUp = $true
        } catch {
            Start-Sleep -Seconds 1
        }
    }
    if (-not $apiUp) { throw "API Tribler jamais en ligne - voir target\interop-tribler\tribler_stderr.log" }

    # Attendre la cle (generee au premier demarrage).
    $deadline = (Get-Date).AddSeconds(15)
    while (-not (Test-Path $pem) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
    if (-not (Test-Path $pem)) { throw "ec_multichain.pem absent" }

    Write-Host "== noeud Rust : intro + circuit 2 sauts via Tribler =="
    $rsErr = Join-Path $outDir "rust_tribler_stderr.log"
    cmd /c "`".\target\debug\examples\tribler_relay_interop.exe`" --tribler-pem `"$pem`" --tribler-port $triblerPort --echo 127.0.0.1:$echoPort --log `"$rsLog`" 2> `"$rsErr`""
    $rsOk = $LASTEXITCODE -eq 0
} finally {
    Stop-Process -Id $triblerProc.Id -Force -ErrorAction SilentlyContinue
    Stop-Process -Id $echoProc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item Env:TSTATEDIR, Env:CORE_API_PORT, Env:CORE_API_KEY -ErrorAction SilentlyContinue
}

$rsStderr = Get-Content $rsErr -Raw -ErrorAction SilentlyContinue
Write-Host "---- rust stderr (extrait) ----"
($rsStderr -split "`n" | Select-String "READY|flags|echo|INTEROP|ECHEC" | Select-Object -Last 12) | ForEach-Object { $_.Line }

if ($rsOk) {
    Write-Host "INTEROP TRIBLER OK"
    exit 0
} else {
    Write-Host "INTEROP TRIBLER ECHEC - voir target\interop-tribler\*.log"
    exit 1
}
