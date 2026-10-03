# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# sec_leak_capture.ps1 - banc P0-17b : observation reseau au niveau OS.
#
#   Capture pktmon (paquets complets) pendant un telechargement
#   anonyme REEL a sauts libres sur le reseau Tribler public
#   (examples/interop_public_download, meme banc que
#   interop_public_dht.ps1), puis classification hors-ligne de chaque
#   endpoint distant par scripts/analyze_leak_capture.py :
#
#     OVERLAY  : endpoint vu dans le journal TAP du processus teste
#                (verite fil : le noeud ne parle qu'UDP IPv8/tunnel)
#     DNS      : port 53 -- qnames decodes et rapportes
#     INTERDIT : TCP vers WAN, UDP vers WAN hors overlay, routeurs DHT
#                mainline contactes en direct (dht.libtorrent.org etc.)
#
#   La capture inclut une fenetre post-arret (-PostExitSec) : tout
#   datagramme WAN emis apres la mort du processus = trafic fantome.
#
# PRECONDITIONS : droits administrateur (pktmon). Sans elevation, le
# script se relance via UAC sauf si -NoElevate.
#
# Usage (shell admin) :
#   powershell -NoProfile -ExecutionPolicy RemoteSigned `
#     -File scripts\sec_leak_capture.ps1 [-Hops 2] [-MinBytes 1048576]
#
# NOTE encodage : fichier volontairement ASCII.

param(
    [string]$Magnet = 'magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10',
    [int]$Hops = 2,
    [long]$MinBytes = 1048576,
    [int]$WalkSeconds = 30,
    [int]$DownloadTimeoutSec = 240,
    [int]$MaxCircuits = 4,
    [int]$PostExitSec = 30,
    [int]$TriblerWaitSec = 180,
    [string]$OutDir = "",
    [switch]$NoElevate,
    [switch]$Inner
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if ($OutDir -eq "") {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutDir = Join-Path $root "target\leak-capture-$stamp"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logFile = Join-Path $OutDir 'sec_leak_capture.log'

function Log([string]$m) {
    $line = '[{0:HH:mm:ss}] {1}' -f (Get-Date), $m
    Write-Host $line
    Add-Content -Path $logFile -Value $line
}

# ---------- Elevation ----------
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    if ($NoElevate) {
        throw "droits administrateur requis pour pktmon (relancer en shell eleve ou sans -NoElevate)"
    }
    Log "re-lancement en admin via UAC -> $logFile"
    $psArgs = @(
        '-NoProfile', '-ExecutionPolicy', 'RemoteSigned',
        '-File', "`"$PSCommandPath`"",
        '-Inner',
        '-Magnet', "`"$Magnet`"",
        '-Hops', "$Hops", '-MinBytes', "$MinBytes",
        '-WalkSeconds', "$WalkSeconds",
        '-DownloadTimeoutSec', "$DownloadTimeoutSec",
        '-MaxCircuits', "$MaxCircuits",
        '-PostExitSec', "$PostExitSec",
        '-TriblerWaitSec', "$TriblerWaitSec",
        '-OutDir', "`"$OutDir`""
    )
    $p = Start-Process -FilePath 'pwsh' -Verb RunAs -Wait -PassThru `
        -ArgumentList $psArgs -WindowStyle Normal
    Write-Host "code de sortie du processus eleve : $($p.ExitCode)"
    exit $p.ExitCode
}

# ---------- Phase elevee ----------
$script:fails = 0
function Verdict([bool]$ok, [string]$name, [string]$detail = "") {
    if ($ok) { Log ("OK   {0} {1}" -f $name, $detail) }
    else     { Log ("FAIL {0} {1}" -f $name, $detail); $script:fails++ }
}

$triblerExe = "C:\Program Files (x86)\Tribler\Tribler.exe"
$stateDir = Join-Path $env:APPDATA ".Tribler"
$confFile = Join-Path $stateDir "8.0\configuration.json"
$triblerProc = $null; $startedTribler = $false; $rsProc = $null
$captureStarted = $false
$t0 = Get-Date

try {
    if (-not (Test-Path $confFile)) { throw "configuration.json Tribler introuvable : $confFile" }
    $conf = Get-Content $confFile -Raw | ConvertFrom-Json
    $ipv4 = $conf.ipv8.interfaces | Where-Object { $_.interface -eq "UDPIPv4" } | Select-Object -First 1
    $triblerPort = [int] $ipv4.port
    $apiKey = $conf.api.key
    $apiPort = $conf.api.http_port_running

    # ---------- Tribler.exe : point d'entree de l'overlay ----------
    $triblerProc = Get-Process -Name "Tribler" -ErrorAction SilentlyContinue
    if (-not $triblerProc) {
        Log "lancement Tribler.exe (GUI, bootstrap reseau reel)"
        $triblerProc = Start-Process -FilePath $triblerExe -PassThru `
            -RedirectStandardOutput (Join-Path $OutDir "tribler_stdout.log") `
            -RedirectStandardError (Join-Path $OutDir "tribler_stderr.log")
        $startedTribler = $true
    } else {
        Log "Tribler.exe deja en cours (pid $($triblerProc.Id))"
    }
    $deadline = (Get-Date).AddSeconds($TriblerWaitSec)
    $peersOk = $false
    while ((Get-Date) -lt $deadline -and -not $peersOk) {
        try {
            $conf2 = Get-Content $confFile -Raw | ConvertFrom-Json
            if ($conf2.api.http_port_running -gt 0) { $apiPort = $conf2.api.http_port_running }
            if ($conf2.api.key) { $apiKey = $conf2.api.key }
            $net = Invoke-RestMethod -Uri "http://127.0.0.1:$apiPort/api/ipv8/network" `
                -Headers @{ "X-Api-Key" = $apiKey } -TimeoutSec 3
            $n = @($net.network.peers).Count
            if ($n -gt 0) { $peersOk = $true; Log "overlay Tribler : $n pair(s) connu(s)" }
        } catch { Start-Sleep -Seconds 2 }
        if (-not $peersOk) { Start-Sleep -Seconds 3 }
    }
    if (-not $peersOk) { throw "Tribler.exe n'a decouvert aucun pair en ${TriblerWaitSec}s" }

    # ---------- Build du banc ----------
    Log "build interop_public_download"
    cargo build -p onionbit-bittorrent --example interop_public_download 2>&1 |
        Out-File (Join-Path $OutDir 'build.log')
    if ($LASTEXITCODE -ne 0) { throw "build echoue (voir build.log)" }
    $exe = Join-Path $root "target\debug\examples\interop_public_download.exe"

    # ---------- Ensembles de classification ----------
    # Routeurs DHT mainline : interdits EN DIRECT (ils doivent passer
    # par le tunnel, donc n'apparaitre localement que vers le 1er saut).
    $routerHosts = @('dht.libtorrent.org', 'dht.transmissionbt.com',
                     'router.bittorrent.com', 'router.utorrent.com', 'dht.aelitis.com')
    $dhtForbidden = @()
    foreach ($h in $routerHosts) {
        try {
            foreach ($a in [System.Net.Dns]::GetHostAddresses($h)) {
                if ($a.AddressFamily -eq 'InterNetwork') { $dhtForbidden += "$($a.IPAddressToString)" }
            }
        } catch {}
    }
    Log ("routeurs DHT resolus : {0}" -f ($dhtForbidden -join ', '))
    $resolvers = @(Get-DnsClientServerAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        ForEach-Object { $_.ServerAddresses } | Where-Object { $_ -and $_ -ne '0.0.0.0' })
    Log ("resolveurs DNS : {0}" -f ($resolvers -join ', '))

    # ---------- Demarrage capture pktmon ----------
    $etl = Join-Path $OutDir 'capture.etl'
    $pcap = Join-Path $OutDir 'capture.pcapng'
    & pktmon filter remove 2>&1 | Out-Null
    & pktmon start --capture --pkt-size 0 --file-name $etl --file-size 512 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "pktmon start a echoue" }
    $captureStarted = $true
    $capStart = Get-Date
    Log "capture pktmon demarree -> $etl"

    # ---------- Banc : download anonyme reel ----------
    $rsLog = Join-Path $OutDir 'public_download.log'
    $rsErr = Join-Path $OutDir 'public_download_err.log'
    $rsArgs = @(
        '--bootstrap', "127.0.0.1:$triblerPort",
        '--hops', "$Hops", '--walk-seconds', "$WalkSeconds",
        '--download-timeout', "$DownloadTimeoutSec",
        '--min-bytes', "$MinBytes", '--max-circuits', "$MaxCircuits",
        '--magnet', $Magnet, '--tap'
    )
    Log "download anonyme $Hops saut(s) (min $MinBytes octets verifies)"
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $exe
    $psi.Arguments = ($rsArgs | ForEach-Object {
        if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
    }) -join ' '
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $rsProc = [System.Diagnostics.Process]::Start($psi)
    $pidBench = $rsProc.Id
    $stdoutTask = $rsProc.StandardOutput.ReadToEndAsync()
    $stderrTask = $rsProc.StandardError.ReadToEndAsync()
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $totalSec = ($WalkSeconds + $MaxCircuits * ($Hops + 1) * 30 + $DownloadTimeoutSec)
    $timedOut = $false
    while (-not $rsProc.HasExited) {
        if ($sw.Elapsed.TotalSeconds -gt $totalSec) {
            $timedOut = $true
            Stop-Process -Id $rsProc.Id -Force -ErrorAction SilentlyContinue
            break
        }
        Start-Sleep -Seconds 5
    }
    $rsProc.WaitForExit()
    [System.IO.File]::WriteAllText($rsLog, $stdoutTask.Result)
    [System.IO.File]::WriteAllText($rsErr, $stderrTask.Result)
    Log "processus de banc termine (code $($rsProc.ExitCode), timeout=$timedOut)"

    # ---------- Fenetre post-arret : trafic fantome ----------
    if ($PostExitSec -gt 0) {
        Log "fenetre post-arret ${PostExitSec}s (tout paquet WAN = suspect)"
        Start-Sleep -Seconds $PostExitSec
    }
    & pktmon stop 2>&1 | Out-Null
    $captureStarted = $false
    $capEnd = Get-Date
    Log "capture arretee"

    & pktmon etl2pcap $etl -o $pcap 2>&1 | Out-Null
    if (-not (Test-Path $pcap)) { throw "etl2pcap a echoue" }
    Log "pcapng : $pcap"

    # ---------- Verite fil : endpoints autorises depuis le TAP ----------
    $stderr = Get-Content $rsErr -Raw -ErrorAction SilentlyContinue
    $allowedFile = Join-Path $OutDir 'allowed_endpoints.txt'
    [regex]::Matches($stderr, 'TAP\[dl\]\s+\w+\s+(\d+\.\d+\.\d+\.\d+:\d+)') |
        ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique |
        Set-Content $allowedFile -Encoding ascii
    Add-Content $allowedFile "127.0.0.1:$triblerPort"
    $nAllowed = @(Get-Content $allowedFile).Count
    Log "endpoints overlay autorises : $nAllowed (TAP + bootstrap)"

    # Ports locaux du processus de banc : attribution de chaque paquet
    # capture au processus teste (pktmon ne porte pas de PID). Un
    # endpoint UDP par socket : noeud IPv8/tunnel + ecoute uTP rqbit.
    $benchPortsFile = Join-Path $OutDir 'bench_ports.txt'
    @(
        [regex]::Matches($stderr, 'noeud downloader : \S+:(\d+)') |
            ForEach-Object { $_.Groups[1].Value }
        [regex]::Matches($stderr, 'Listening on UDP \S+:(\d+)') |
            ForEach-Object { $_.Groups[1].Value }
    ) | Sort-Object -Unique | Set-Content $benchPortsFile -Encoding ascii
    Log "ports locaux du banc : $((Get-Content $benchPortsFile) -join ', ')"

    # ---------- Verdicts ----------
    $verified = [regex]::Match($stderr, 'octets_verifies=(\d+)')
    $okBytes = $verified.Success -and [int64]$verified.Groups[1].Value -ge $MinBytes
    Verdict (-not $timedOut -and $rsProc.ExitCode -eq 0) 'download anonyme termine' "exit=$($rsProc.ExitCode)"
    $vBytes = if ($verified.Success) { $verified.Groups[1].Value } else { 'absent' }
    Verdict $okBytes 'octets verifies >= MinBytes' $vBytes
    ($stderr -split "`n" | Select-String 'route observee' | Select-Object -Last 1) |
        ForEach-Object { Log $_.Line }

    $py = 'python'
    if ($env:TRIBLER_INTEROP_PY -and (Test-Path $env:TRIBLER_INTEROP_PY)) { $py = $env:TRIBLER_INTEROP_PY }
    $analyzer = Join-Path $root 'scripts\analyze_leak_capture.py'
    $report = Join-Path $OutDir 'leak_report.json'
    $anArgs = @($analyzer, $pcap, '--allowed', $allowedFile, '--bench-ports', $benchPortsFile, '--report', $report)
    if ($resolvers.Count) { $anArgs += @('--dns-resolvers', ($resolvers -join ',')) }
    if ($dhtForbidden.Count) { $anArgs += @('--dht-routers', ($dhtForbidden -join ',')) }
    $anOut = & $py @anArgs 2>&1
    $anOut | Out-File (Join-Path $OutDir 'leak_analysis.txt')
    $anOut | Select-Object -Last 30 | ForEach-Object { Log $_ }
    Verdict ($LASTEXITCODE -eq 0) 'analyse de fuite (0 paquet interdit)'

    # ---------- Manifeste ----------
    @{
        run_utc    = $t0.ToUniversalTime().ToString('o')
        script     = 'sec_leak_capture.ps1'
        commit     = (git -C $root rev-parse --short HEAD)
        hops       = $Hops; min_bytes = $MinBytes; magnet = $Magnet
        pid_bench  = $pidBench
        tribler    = @{ exe = $triblerExe; port_ipv8 = $triblerPort; started_by_bench = $startedTribler }
        capture    = @{ etl = $etl; pcapng = $pcap; start = $capStart.ToString('o'); end = $capEnd.ToString('o'); post_exit_sec = $PostExitSec }
        resolvers  = $resolvers
        dht_routers_forbidden = $dhtForbidden
        artifacts  = @{ rust_stderr = $rsErr; analysis = 'leak_analysis.txt'; report = 'leak_report.json' }
    } | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OutDir 'manifest.json') -Encoding UTF8

    Log ""
    if ($script:fails -eq 0) { Log "SEC LEAK CAPTURE OK"; exit 0 }
    Log "SEC LEAK CAPTURE ECHEC ($($script:fails) verdict(s))"
    exit 1
}
finally {
    if ($captureStarted) { & pktmon stop 2>&1 | Out-Null }
    if ($rsProc -and -not $rsProc.HasExited) { Stop-Process -Id $rsProc.Id -Force -ErrorAction SilentlyContinue }
    if ($startedTribler -and $triblerProc) { Stop-Process -Id $triblerProc.Id -Force -ErrorAction SilentlyContinue }
    Get-Process -Name 'interop_public_download' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}
