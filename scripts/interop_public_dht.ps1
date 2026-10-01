# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# interop_public_dht.ps1 - banc DHT publique / reseau Tribler reel.
#
#   downloader Rust -> circuits a sauts LIBRES sur le reseau Tribler
#   (selection standard, route OBSERVEE rapportee) -> sortie = noeud
#   Tribler reel flagge exit -> swarm BitTorrent public.
#
# NON-DETERMINISTE par nature : depend des pairs/relays/exits reels
# disponibles. Le verdict exige : code 0 + "INTEROP PUBLIC DOWNLOAD
# OK" + octets_verifies >= -MinBytes (ou completion si -MinBytes 0).
# La route imprimee dans les logs est toujours celle observee.
#
# La decouverte demarre par un walk vers Tribler.exe local
# (configuration.json -> port IPv8), dont l'overlay connait le reseau
# reel ; les introductions enchainent vers les pairs publics.
#
# Usage : powershell -NoProfile -ExecutionPolicy RemoteSigned `
#   -File scripts\interop_public_dht.ps1 -Magnet "magnet:?xt=..." `
#   [-Hops 2] [-MinBytes 1048576] [-WalkSeconds 30]

param(
    # Magnet URI ou chemin d'un .torrent reel (exactement un requis).
    [string] $Magnet,
    [string] $TorrentFile,
    [int] $Hops = 2,
    # Octets verifies exiges (0 = completion du torrent).
    [long] $MinBytes = 1048576,
    [int] $WalkSeconds = 30,
    [int] $DownloadTimeoutSec = 300,
    [int] $MaxCircuits = 4,
    [int] $TriblerWaitSec = 180
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$outDir = Join-Path $root "target\interop-public-dht"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$triblerExe = "C:\Program Files (x86)\Tribler\Tribler.exe"
$stateDir = Join-Path $env:APPDATA ".Tribler"
$confFile = Join-Path $stateDir "8.0\configuration.json"

if (-not $Magnet -and -not $TorrentFile) { throw "fournir -Magnet ou -TorrentFile" }
if ($Magnet -and $TorrentFile) { throw "-Magnet et -TorrentFile sont exclusifs" }
if ($Hops -lt 1 -or $Hops -gt 5) { throw "-Hops dans 1..5" }
if (-not (Test-Path $confFile)) { throw "configuration.json Tribler introuvable : $confFile" }

$conf = Get-Content $confFile -Raw | ConvertFrom-Json
$ipv4 = $conf.ipv8.interfaces | Where-Object { $_.interface -eq "UDPIPv4" } | Select-Object -First 1
$triblerPort = [int] $ipv4.port
$apiKey = $conf.api.key
$apiPort = $conf.api.http_port_running

$triblerProc = Get-Process -Name "Tribler" -ErrorAction SilentlyContinue
$startedTribler = $false
$rsProc = $null
try {
    if (-not $triblerProc) {
        Write-Host "== lancement Tribler.exe (GUI, bootstrap dans le reseau reel) =="
        $triblerProc = Start-Process -FilePath $triblerExe -PassThru `
            -RedirectStandardOutput (Join-Path $outDir "tribler_stdout.log") `
            -RedirectStandardError (Join-Path $outDir "tribler_stderr.log")
        $startedTribler = $true
    } else {
        Write-Host "== Tribler.exe deja en cours (pid $($triblerProc.Id)) =="
    }

    # Attente de l'API : la tunnel community de l'instance doit etre
    # vivante ET l'overlay doit connaitre des pairs externes (sinon la
    # marche ne peut pas sortir du reseau local).
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
            if ($n -gt 0) { $peersOk = $true; Write-Host "== overlay Tribler : $n pair(s) connu(s) ==" }
        } catch { Start-Sleep -Seconds 2 }
        if (-not $peersOk) { Start-Sleep -Seconds 3 }
    }
    if (-not $peersOk) { throw "Tribler.exe n'a decouvert aucun pair en ${TriblerWaitSec}s" }

    Write-Host "== build interop_public_download (rust) =="
    cargo build -p onionbit-bittorrent --example interop_public_download
    if ($LASTEXITCODE -ne 0) { throw "build echoue" }

    $rsLog = Join-Path $outDir "public_download.log"
    $rsErr = Join-Path $outDir "public_download_err.log"
    Remove-Item -Force -ErrorAction SilentlyContinue $rsLog, $rsErr

    $rsArgs = @(
        "--bootstrap", "127.0.0.1:$triblerPort",
        "--hops", "$Hops",
        "--walk-seconds", "$WalkSeconds",
        "--download-timeout", "$DownloadTimeoutSec",
        "--min-bytes", "$MinBytes",
        "--max-circuits", "$MaxCircuits",
        "--tap"
    )
    if ($Magnet) { $rsArgs += @("--magnet", $Magnet) }
    else { $rsArgs += @("--torrent", $TorrentFile) }

    Write-Host "== telechargement via reseau Tribler reel ($Hops saut(s), min $MinBytes octets verifies) =="
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = Join-Path $root "target\debug\examples\interop_public_download.exe"
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
    # Toujours vider les flux — meme (surtout) en timeout, sinon les
    # logs de diagnostic sont perdus.
    [System.IO.File]::WriteAllText($rsLog, $stdoutTask.Result)
    [System.IO.File]::WriteAllText($rsErr, $stderrTask.Result)

    $stderr = Get-Content $rsErr -Raw -ErrorAction SilentlyContinue
    Write-Host "---- rust stderr (extrait) ----"
    ($stderr -split "`n" | Select-String "decouverte|pool|essai|route|progression|INTEROP|ECHEC" | Select-Object -Last 30) | ForEach-Object { $_.Line }
    if ($timedOut) {
        Write-Host "INTEROP PUBLIC DHT ECHEC - timeout ${totalSec}s (processus tue, logs conserves)"
        exit 1
    }

    $verified = [regex]::Match($stderr, "octets_verifies=(\d+)")
    if ($rsProc.ExitCode -ne 0) {
        Write-Host "INTEROP PUBLIC DHT ECHEC - code de sortie $($rsProc.ExitCode), voir target\interop-public-dht\*.log"
        exit 1
    }
    if ($stderr -notmatch "INTEROP PUBLIC DOWNLOAD OK") {
        Write-Host "INTEROP PUBLIC DHT ECHEC - pas de ligne de succes"
        exit 1
    }
    if (-not $verified.Success -or [int64]$verified.Groups[1].Value -lt $MinBytes) {
        Write-Host "INTEROP PUBLIC DHT ECHEC - octets verifies insuffisants"
        exit 1
    }
    Write-Host "INTEROP PUBLIC DHT OK"
    exit 0
}
finally {
    if ($rsProc -and -not $rsProc.HasExited) { Stop-Process -Id $rsProc.Id -Force -ErrorAction SilentlyContinue }
    if ($startedTribler -and $triblerProc) { Stop-Process -Id $triblerProc.Id -Force -ErrorAction SilentlyContinue }
    Get-Process -Name "interop_public_download" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}
