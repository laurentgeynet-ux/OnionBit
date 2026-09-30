# interop_tribler_relay.ps1 - interop : relais = vrai Tribler.exe
# installe (TriblerTunnelCommunity, prefixe a3591a6b…), sortie =
# TunnelCommunity pyipv8 controle (EXIT_BT).
#
#   rqbit downloader -> Tribler.exe (relais) -> [relais Rust epingle] -> sortie pyipv8 -> seeder
#
# Le script :
#   1. lit la configuration Tribler installee (%APPDATA%\.Tribler\8.0)
#      pour recuperer le port IPv8 UDPv4 et la cle API REST ;
#   2. verifie l'identite de l'instance Tribler joignable via
#      /api/ipv8/overlays (cle publique de la tunnel community) — un
#      processus "Tribler" deja en cours n'est utilise que si c'est bien
#      celui de cette configuration ;
#   3. demarre Tribler.exe si absent (GUI — pas de mode headless) ;
#   4. demarre la sortie pyipv8 controlee avec le prefixe de community
#      de TriblerTunnelCommunity ;
#   5. lance le telechargement Rust avec --relay + --tribler-id ;
#      les relais Rust intermediaires sont epingles (saut impose, sans
#      repli sur les candidats publics annonces par Tribler) ;
#   6. echantillonne /api/ipv8/tunnel/relays pendant le transfert pour
#      rapprocher circuit_from/circuit_to et compteurs d'octets ;
#   7. exige : code 0 + ligne de succes + verifie= == payload demande.
#
# -Dht : la decouverte DHT est celle du banc local controle (noeud
# bootstrap local), PAS la DHT publique.
#
# Usage : powershell -NoProfile -ExecutionPolicy RemoteSigned `
#   -File scripts\interop_tribler_relay.ps1 [-Hops 2|3] [-Dht] [-Payload N]

param(
    # Nombre de sauts total (2 = Tribler relais + sortie pyipv8 ; 3 =
    # Tribler + relais Rust + sortie pyipv8).
    [int] $Hops = 2,
    [switch] $Dht,
    [int] $Payload = 200000,
    # Delai max d'attente du demarrage de Tribler.exe (GUI lourde).
    [int] $TriblerWaitSec = 180,
    # Timeout de telechargement (s) — borne `WaitForExit` du processus
    # Rust (transfert tunnel peut etre lent ; dimensionne sur le defaut
    # qui couvre 32 Mio dans le banc controle).
    [int] $DownloadTimeoutSec = 240
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$pyipv8 = if ($env:TRIBLER_PYIPV8) { $env:TRIBLER_PYIPV8 } else { "D:\Projet\Tribler_sources\tribler\pyipv8" }
$venvPy = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
$outDir = Join-Path $root "target\interop-tribler-relay"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$triblerExe = "C:\Program Files (x86)\Tribler\Tribler.exe"
$stateDir = Join-Path $env:APPDATA ".Tribler"
$confFile = Join-Path $stateDir "8.0\configuration.json"
# Prefixe fil de TriblerTunnelCommunity (distinct du TunnelCommunity
# pyipv8 generique 81ded073…).
$triblerCid = "a3591a6bd89bbaca0974062a1287afcfbc6fd6bc"

if ($Hops -lt 2 -or $Hops -gt 3) { throw "-Hops doit etre 2 ou 3 (Tribler n'est pas la sortie)" }
if (-not (Test-Path $triblerExe)) { throw "Tribler.exe introuvable : $triblerExe" }
if (-not (Test-Path $confFile)) { throw "configuration.json Tribler introuvable : $confFile (lancer Tribler une fois)" }

# 1. Config Tribler : port IPv4 IPv8 + API REST.
$conf = Get-Content $confFile -Raw | ConvertFrom-Json
$ipv4 = $conf.ipv8.interfaces | Where-Object { $_.interface -eq "UDPIPv4" } | Select-Object -First 1
$triblerPort = [int] $ipv4.port
$pemPath = Join-Path $stateDir "ec_multichain.pem"
if (-not (Test-Path $pemPath)) { throw "ec_multichain.pem introuvable : $pemPath" }

# 2. Cle publique attendue depuis le PEM (reference pour verifier que
#    l'instance jointe est bien celle de cette configuration).
$env:PYTHONPATH = $pyipv8
$triblerPubHex = (& $venvPy "$PSScriptRoot\interop\tribler_pubkey.py" $pemPath).Trim()
if ($LASTEXITCODE -ne 0 -or -not $triblerPubHex) { throw "extraction pubkey Tribler echouee" }

# 3. Tribler.exe : demarrer si absent, attendre l'API REST.
$triblerProc = Get-Process -Name "Tribler" -ErrorAction SilentlyContinue
$startedTribler = $false
$pyProc = $null
$rsProc = $null
try {
    if (-not $triblerProc) {
        Write-Host "== lancement Tribler.exe (GUI) =="
        $triblerOut = Join-Path $outDir "tribler_stdout.log"
        $triblerErr = Join-Path $outDir "tribler_stderr.log"
        # RUST_LOG : l'endpoint `ipv8_rust_tunnels` (crypto endpoint)
        # logue via env_logger — visibilite sur send_cell/relais.
        $env:RUST_LOG = "debug"
        $triblerProc = Start-Process -FilePath $triblerExe -PassThru `
            -RedirectStandardOutput $triblerOut -RedirectStandardError $triblerErr
        $startedTribler = $true
    } else {
        Write-Host "== Tribler.exe deja en cours (pid $($triblerProc.Id)), verification d'identite =="
    }

    # Le port API reel est `http_port_running` (peut etre reecrit au
    # demarrage si http_port=0).
    $apiKey = $conf.api.key
    $apiPort = $conf.api.http_port_running
    if (-not $apiPort -or $apiPort -eq 0) { $apiPort = $conf.api.http_port }
    $deadline = (Get-Date).AddSeconds($TriblerWaitSec)
    $apiUp = $false
    while ((Get-Date) -lt $deadline -and -not $apiUp) {
        try {
            $conf2 = Get-Content $confFile -Raw | ConvertFrom-Json
            if ($conf2.api.http_port_running -gt 0) { $apiPort = $conf2.api.http_port_running }
            if ($conf2.api.key) { $apiKey = $conf2.api.key }
            $r = Invoke-WebRequest -Uri "http://127.0.0.1:$apiPort/api/settings" `
                -Headers @{ "X-Api-Key" = $apiKey } -TimeoutSec 3 -UseBasicParsing
            if ($r.StatusCode -eq 200) { $apiUp = $true }
        } catch {
            Start-Sleep -Seconds 2
        }
    }
    if (-not $apiUp) { throw "API REST Tribler injoignable apres ${TriblerWaitSec}s" }
    Write-Host "== API REST Tribler OK sur 127.0.0.1:$apiPort =="
    # Marge pour que la TunnelCommunity soit chargee apres l'API.
    Start-Sleep -Seconds 8

    # 4. Identite de l'instance JOINTE : la tunnel community doit
    #    exister et sa cle doit correspondre au PEM de la config —
    #    sinon le processus reutilise n'est pas le bon relais.
    $overlays = Invoke-RestMethod -Uri "http://127.0.0.1:$apiPort/api/ipv8/overlays" `
        -Headers @{ "X-Api-Key" = $apiKey } -TimeoutSec 10
    $tunnelOverlay = $overlays.overlays | Where-Object { $_.id -eq $triblerCid } | Select-Object -First 1
    if (-not $tunnelOverlay) { throw "pas d'overlay tunnel $triblerCid — TunnelCommunity inactive sur cette instance" }
    $livePubHex = $tunnelOverlay.my_peer
    if ($livePubHex -ne $triblerPubHex) {
        throw "l'instance Tribler jointe a une cle differente du PEM ($($livePubHex.Substring(0,24))… != $($triblerPubHex.Substring(0,24))…) — autre processus"
    }
    Write-Host "== instance Tribler verifiee : port $triblerPort, mid $($triblerPubHex.Substring(0,16))… =="
    $triblerKeyFile = Join-Path $outDir "tribler_key.txt"
    Set-Content -Path $triblerKeyFile -Value "$triblerPubHex $triblerPort"

    # 5. Sortie pyipv8 controlee (meme prefixe de community que Tribler).
    $pyPort = 12100
    $pyLog = Join-Path $outDir "py_exit_packets.log"
    $pyKeyFile = Join-Path $outDir "py_key.txt"
    Remove-Item -Force -ErrorAction SilentlyContinue $pyKeyFile

    Write-Host "== build exit_download_interop (rust) =="
    cargo build -p tribler-bittorrent --example exit_download_interop
    if ($LASTEXITCODE -ne 0) { throw "build echoue" }

    Write-Host "== sortie pyipv8 (EXIT_BT, cid=$triblerCid) sur 127.0.0.1:$pyPort =="
    $pyProc = Start-Process -FilePath $venvPy -PassThru -NoNewWindow `
        -ArgumentList "`"$PSScriptRoot\interop\py_tunnel_node.py`" --port $pyPort --keyfile `"$pyKeyFile`" --log `"$pyLog`" --duration 300 --community-id $triblerCid" `
        -RedirectStandardError (Join-Path $outDir "py_exit_stderr.log")

    $deadline = (Get-Date).AddSeconds(10)
    while (-not (Test-Path $pyKeyFile) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 100
    }
    if (-not (Test-Path $pyKeyFile)) { throw "keyfile pyipv8 jamais ecrit" }

    # 6. Telechargement : Rust -> Tribler (relais) -> [relais epingles]
    #    -> sortie pyipv8. Lancement direct de l'exemple (pas de cmd
    #    /c : redirections et ExitCode fiables).
    $dhtFlag = if ($Dht) { "--dht" } else { "" }
    $dhtLabel = if ($Dht) { ", DHT locale controlee" } else { "" }
    Write-Host "== telechargement rqbit via Tribler.exe ($Hops saut(s), sortie pyipv8$dhtLabel, payload demande = $Payload octets) =="
    $rsLog = Join-Path $outDir "rust_exit_download.log"
    $rsErr = Join-Path $outDir "rust_exit_stderr.log"
    Remove-Item -Force -ErrorAction SilentlyContinue $rsLog, $rsErr
    # Hop timeout elargi : Tribler peut faire un `dht_peer_lookup`
    # avant de forwarder un `extend` vers un pair inconnu.
    $rsArgs = @(
        "--keyfile", $pyKeyFile,
        "--relay", $triblerKeyFile,
        "--tribler-id",
        "--hop-timeout-ms", "45000",
        "--hops", "$Hops",
        "--payload", "$Payload",
        "--tap"
    )
    if ($Dht) { $rsArgs += "--dht" }
    # ProcessStartInfo direct : `Start-Process -PassThru` avec
    # redirections ne renseigne jamais ExitCode sous PowerShell 5.1.
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = Join-Path $root "target\debug\examples\exit_download_interop.exe"
    $psi.Arguments = ($rsArgs | ForEach-Object {
        if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
    }) -join ' '
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $rsProc = [System.Diagnostics.Process]::Start($psi)
    $stdoutTask = $rsProc.StandardOutput.ReadToEndAsync()
    $stderrTask = $rsProc.StandardError.ReadToEndAsync()

    # Polling des relais Tribler pendant le transfert (rapprochement
    # circuit_from/circuit_to + compteurs d'octets avec le circuit
    # annonce cote Rust). Un echantillon toutes les ~2 s, borne par la
    # fin du processus Rust.
    $relaysLog = Join-Path $outDir "tribler_relays_samples.ndjson"
    Remove-Item -Force -ErrorAction SilentlyContinue $relaysLog
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while (-not $rsProc.HasExited) {
        if ($sw.Elapsed.TotalSeconds -gt $DownloadTimeoutSec) {
            Stop-Process -Id $rsProc.Id -Force -ErrorAction SilentlyContinue
            throw "exit_download_interop en timeout apres ${DownloadTimeoutSec}s (processus tue)"
        }
        try {
            $s = Invoke-RestMethod -Uri "http://127.0.0.1:$apiPort/api/ipv8/tunnel/relays" `
                -Headers @{ "X-Api-Key" = $apiKey } -TimeoutSec 3
            $entry = [ordered]@{ t = [int]$sw.Elapsed.TotalSeconds; relays = $s.relays }
            ($entry | ConvertTo-Json -Depth 5 -Compress) | Out-File -Append -Encoding utf8 $relaysLog
        } catch {}
        Start-Sleep -Seconds 2
    }
    $rsProc.WaitForExit()
    [System.IO.File]::WriteAllText($rsLog, $stdoutTask.Result)
    [System.IO.File]::WriteAllText($rsErr, $stderrTask.Result)
    $rsOk = $rsProc.ExitCode -eq 0

    # Buffer de logs Tribler via l'endpoint /api/logging.
    try {
        Invoke-WebRequest -Uri "http://127.0.0.1:$apiPort/api/logging" `
            -Headers @{ "X-Api-Key" = $apiKey } -TimeoutSec 10 -UseBasicParsing `
            -OutFile (Join-Path $outDir "tribler_api_logging.log") | Out-Null
    } catch {}

    $rsStderr = Get-Content $rsErr -Raw -ErrorAction SilentlyContinue
    Write-Host "---- rust stderr (extrait) ----"
    ($rsStderr -split "`n" | Select-String "READY|route|attendu|INTEROP|ECHEC|timeout|extend" | Select-Object -Last 12) | ForEach-Object { $_.Line }

    # Preuve positive exigee : code 0 ET ligne de succes ET verifie ==
    # payload demande (un succes sans la ligne verifie= n'est pas un
    # verdict).
    $verified = [regex]::Match($rsStderr, "verifie=(\d+) octets")
    $successLine = $rsStderr -match "INTEROP EXIT DOWNLOAD OK"
    if (-not $rsOk) {
        Write-Host "INTEROP TRIBLER RELAY ECHEC - code de sortie $($rsProc.ExitCode), voir target\interop-tribler-relay\*.log"
        exit 1
    }
    if (-not $successLine) {
        Write-Host "INTEROP TRIBLER RELAY ECHEC - pas de ligne INTEROP EXIT DOWNLOAD OK"
        exit 1
    }
    if (-not $verified.Success) {
        Write-Host "INTEROP TRIBLER RELAY ECHEC - succes annonce sans taille verifiee"
        exit 1
    }
    if ([int64]$verified.Groups[1].Value -ne [int64]$Payload) {
        Write-Host "INTEROP TRIBLER RELAY ECHEC - taille verifiee $($verified.Groups[1].Value) <> demandee $Payload"
        exit 1
    }
    Write-Host "INTEROP TRIBLER RELAY OK"
    exit 0
}
finally {
    # Nettoyage borne aux processus lances par le banc : une instance
    # Tribler preexistante n'est jamais arretee.
    if ($rsProc -and -not $rsProc.HasExited) { Stop-Process -Id $rsProc.Id -Force -ErrorAction SilentlyContinue }
    if ($pyProc) { Stop-Process -Id $pyProc.Id -Force -ErrorAction SilentlyContinue }
    if ($startedTribler -and $triblerProc) { Stop-Process -Id $triblerProc.Id -Force -ErrorAction SilentlyContinue }
    Get-Process -Name "exit_download_interop" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}
