# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# bench_stealth_fingerprint.ps1 - banc de fingerprinting stealth
# (ADR-0017, etape 55).
#
# Topologie loopback : un daemon `bridge` (B) et un daemon `client`
# (C) en mode stealth, relies a travers un relais UDP `stealth_bench
# tap` qui consigne chaque datagramme en PCAP reel (preuve niveau
# socket — jamais un compteur interne) et peut injecter
# perte/duplication/reordonnancement.
#
# Familles de tests (revue externe 1) :
#   1. probing actif     -> zero reponse (silence uniforme)
#   2. amplification     -> bytes_recv / bytes_sent <= 1
#   3. fuite legacy      -> zero marqueur (LibNaCLPK:, community_id,
#                          onionbit, bittorrent, dht) dans le PCAP
#   4. resilience        -> perte/dup/reordonnancement, restart pont,
#                          nouveau port client (NAT rebinding),
#                          expiration de session, saturation — jamais
#                          de repli clair
#   5. classifieur       -> corpus {dns, quic, wg, noise, ipv8,
#                          stealth} : mesure honnete de separation
#
# Oracles stricts (critere de sortie Phase 10) :
#   oracle_silence, oracle_amplification_le_1, oracle_no_markers,
#   oracle_no_constants, oracle_entropy — consignes dans report.json.
#
# Usage :
#   .\scripts\bench_stealth_fingerprint.ps1 [-DurationSec 60]
# Produits : <OutDir>\*.pcap, report.json, manifest.json
#
# NOTE encodage : fichier volontairement ASCII.
param(
    [string]$Daemon     = "",
    [string]$Bench      = "",
    [int]   $DurationSec = 45,
    [switch]$SkipBuild,
    [string]$OutDir = ("target\bench-stealth-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
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


$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$out  = Join-Path $root $OutDir
if ($Daemon -eq "") { $Daemon = Join-Path $root 'target\debug\onionbit-daemon.exe' }
if ($Bench  -eq "") { $Bench  = Join-Path $root 'target\debug\stealth_bench.exe' }
New-Item -ItemType Directory -Force -Path $out | Out-Null

# Ports fixes du banc (loopback ferme).
$B = @{ Api = 8191; Udp = 18791; Dir = (Join-Path $out 'b'); Name = 'B' }
$C = @{ Api = 8192; Udp = 18792; Dir = (Join-Path $out 'c'); Name = 'C' }
$TapPort = 18790

function Log-Host([string]$m) { Write-Host ('[{0:HH:mm:ss}] {1}' -f (Get-Date), $m) }
function ApiKey([string]$dir) {
    $cfg = Join-Path $dir 'configuration.json'
    if (-not (Test-Path $cfg)) { return $null }
    try { return (Get-Content $cfg -Raw | ConvertFrom-Json).api.key } catch { return $null }
}
function Wait-ApiUp([int]$port, $key, [int]$sec = 90) {
    $dl = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $dl) {
        try {
            Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/statistics/tribler" `
                -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 5 | Out-Null
            return
        } catch {}
        Start-Sleep -Milliseconds 700
    }
    throw "API 127.0.0.1:$port injoignable"
}
function Wait-StealthSession([int]$port, $key, [int]$sec = 45) {
    $dl = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $dl) {
        try {
            $s = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/stealth" `
                -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 5
            if ($s.sessions -ge 1) { return $s }
        } catch {}
        Start-Sleep -Milliseconds 800
    }
    throw "aucune session stealth sur :$port apres ${sec}s"
}
function Start-Daemon($p, $cfg) {
    New-Item -ItemType Directory -Force -Path $p.Dir | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $p.Dir 'configuration.json'),
        ($cfg | ConvertTo-Json -Compress -Depth 8))
    $proc = Start-Process -FilePath $Daemon -PassThru -NoNewWindow `
        -ArgumentList '--state-dir', "`"$($p.Dir)`"", '--listen', "127.0.0.1:$($p.Api)", '--no-tray' `
        -RedirectStandardOutput (Join-Path $p.Dir 'daemon_stdout.log') `
        -RedirectStandardError  (Join-Path $p.Dir 'daemon_stderr.log')
    Log-Host ("{0} pid={1} api={2} udp={3}" -f $p.Name, $proc.Id, $p.Api, $p.Udp)
    return $proc
}
function Start-Tap([string]$pcap, [double]$loss, [double]$dup, [double]$reorder) {
    $proc = Start-Process -FilePath $Bench -PassThru -NoNewWindow `
        -ArgumentList 'tap', '--listen', "127.0.0.1:$TapPort", '--upstream', "127.0.0.1:$($B.Udp)",
                      '--pcap', "`"$pcap`"", '--loss', "$loss", '--dup', "$dup",
                      '--reorder', "$reorder", '--jitter-ms', '80' `
        -RedirectStandardError (Join-Path $out 'tap_stderr.log')
    Log-Host ("tap pid={0} :{1} -> :{2} -> {3}" -f $proc.Id, $TapPort, $B.Udp, $pcap)
    return $proc
}
function Get-Stealth([int]$port, $key) {
    Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/stealth" `
        -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 5
}

if (-not $SkipBuild) {
    Log-Host "build daemon + stealth_bench"
    cargo build --manifest-path (Join-Path $root 'Cargo.toml') `
        -p onionbit-daemon --bin onionbit-daemon `
        -p onionbit-ipv8 --bin stealth_bench | Out-Null
}
if (-not (Test-Path $Daemon)) { throw "daemon absent: $Daemon" }
if (-not (Test-Path $Bench))  { throw "stealth_bench absent: $Bench" }

# Config commune stealth — tuning court pour les phases restart/
# expiration, cover traffic pour donner du volume a la capture.
function StealthCfg([string]$role, [int]$udp, [string[]]$bridges) {
    @{
        ipv8 = @{ enabled = $false
                  interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $udp } ) }
        tunnel_community = @{ enabled = $true }
        ext = @{ enabled = $true }
        stealth = @{
            enabled = $true
            role = $role
            bridges = $bridges
            cover_traffic = $true
            tuning = @{
                tick_ms = 200
                hs_retry_secs = 1
                hs_attempts_max = 8
                pending_timeout_secs = 8
                dial_cooldown_secs = 2
                session_idle_timeout_secs = 25
                cover_interval_min_ms = 150
                cover_interval_max_ms = 500
            }
        }
    }
}

$procs = @{}
$results = [ordered]@{}
try {
    # ---------- Phase 0 : pont puis lien, tap, client ---------------
    $procs['B'] = Start-Daemon $B (StealthCfg 'bridge' $B.Udp @())
    $kB = ApiKey $B.Dir
    $d0 = (Get-Date).AddSeconds(60)
    while (-not $kB -and (Get-Date) -lt $d0) { Start-Sleep -Milliseconds 500; $kB = ApiKey $B.Dir }
    if (-not $kB) { throw "cle API pont absente" }
    Wait-ApiUp $B.Api $kB

    # Lien d'invitation pointant sur le TAP (tout le trafic client
    # transite par la capture socket).
    $skFile = Join-Path $B.Dir 'stealth_bridge.key'
    $d1 = (Get-Date).AddSeconds(30)
    while (-not (Test-Path $skFile) -and (Get-Date) -lt $d1) { Start-Sleep -Milliseconds 400 }
    if (-not (Test-Path $skFile)) { throw "stealth_bridge.key absent" }
    $link = (& $Bench link --key $skFile --addr "127.0.0.1:$TapPort").Trim()
    Log-Host "lien bridge (via tap) : $($link.Substring(0, 40))..."

    $pcap1 = Join-Path $out 'stealth_main.pcap'
    $procs['tap'] = Start-Tap $pcap1 0 0 0

    $procs['C'] = Start-Daemon $C (StealthCfg 'client' $C.Udp @($link))
    $d2 = (Get-Date).AddSeconds(60)
    $kC = $null
    while (-not $kC -and (Get-Date) -lt $d2) { Start-Sleep -Milliseconds 500; $kC = ApiKey $C.Dir }
    if (-not $kC) { throw "cle API client absente" }
    Wait-ApiUp $C.Api $kC

    # ---------- Phase 1 : session + capture de fond -----------------
    $s0 = Wait-StealthSession $C.Api $kC 60
    Log-Host ("session stealth etablie (client sessions={0}, bytes_up={1})" -f `
        $s0.sessions, $s0.bytes_up)
    $results['phase1_session'] = $true

    Log-Host "== capture de fond ${DurationSec}s (ext hello/intro + cover) =="
    $rssB0 = (Get-Process -Id $procs['B'].Id).WorkingSet64
    $rssC0 = (Get-Process -Id $procs['C'].Id).WorkingSet64
    Start-Sleep -Seconds $DurationSec
    $sC = Get-Stealth $C.Api $kC
    $results['phase1_bytes_up'] = $sC.bytes_up
    $results['phase1_sessions'] = $sC.sessions

    # ---------- Phase 2 : probing actif + amplification -------------
    Log-Host "== probing actif (600 sondes + rejeu de la capture) =="
    foreach ($t in @(@{n='B'; port=$B.Udp}, @{n='C'; port=$C.Udp})) {
        $j = & $Bench probe --target "127.0.0.1:$($t.port)" --count 600 `
            --pace-ms 2 --window-ms 1500 --replay $pcap1 | ConvertFrom-Json
        $results["probe_$($t.n)"] = $j
        Log-Host ("  probe {0} : reponses={1} amp={2:N4}" -f $t.n, $j.responses, $j.amplification)
    }
    $rssB1 = (Get-Process -Id $procs['B'].Id).WorkingSet64
    $rssC1 = (Get-Process -Id $procs['C'].Id).WorkingSet64
    $results['rss_delta_probing'] = @{
        B = $rssB1 - $rssB0; C = $rssC1 - $rssC0
    }

    # Saturation : rafale soutenue — la session legtime doit rester
    # debout (le plafond global protege le CPU, le rate-limit par IP
    # peut dropper la sonde mais pas la session etablie).
    Log-Host "== saturation (rafale 4000 sondes) =="
    $j = & $Bench probe --target "127.0.0.1:$($B.Udp)" --count 4000 `
        --pace-ms 0 --window-ms 1000 | ConvertFrom-Json
    $results['probe_flood_B'] = $j
    $sF = Get-Stealth $C.Api $kC
    $results['session_survit_flood'] = ($sF.sessions -ge 1)

    # ---------- Phase 3 : resilience --------------------------------
    # (a) perte/dup/reordonnancement injectes par le tap.
    Log-Host "== resilience : tap avec impairments (5% loss/dup/reorder) =="
    Stop-Process -Id $procs['tap'].Id -Force
    $pcap2 = Join-Path $out 'stealth_impaired.pcap'
    $procs['tap'] = Start-Tap $pcap2 0.05 0.05 0.05
    Start-Sleep -Seconds 20
    $sI = Get-Stealth $C.Api $kC
    $results['session_sous_impairments'] = ($sI.sessions -ge 1)

    # (b) restart du pont — meme state dir => meme cle => meme lien ;
    # le client re-diale apres dial_cooldown.
    Log-Host "== resilience : restart du pont =="
    $tapBefore = $procs['tap'].Id
    Stop-Process -Id $procs['B'].Id -Force
    Start-Sleep -Seconds 3
    $procs['B'] = Start-Daemon $B (StealthCfg 'bridge' $B.Udp @())
    $kB2 = $null
    $d3 = (Get-Date).AddSeconds(60)
    while (-not $kB2 -and (Get-Date) -lt $d3) { Start-Sleep -Milliseconds 500; $kB2 = ApiKey $B.Dir }
    Wait-ApiUp $B.Api $kB2 60
    $sR = Wait-StealthSession $C.Api $kC 60
    $results['session_apres_restart_pont'] = ($sR.sessions -ge 1)
    Log-Host "  session re-etablie apres restart"

    # (c) "NAT rebinding" : nouveau port source du client.
    Log-Host "== resilience : nouveau port client (rebinding) =="
    Stop-Process -Id $procs['C'].Id -Force
    $C.Udp = 18793
    $procs['C'] = Start-Daemon $C (StealthCfg 'client' $C.Udp @($link))
    $kC = $null
    $d4 = (Get-Date).AddSeconds(60)
    while (-not $kC -and (Get-Date) -lt $d4) { Start-Sleep -Milliseconds 500; $kC = ApiKey $C.Dir }
    Wait-ApiUp $C.Api $kC 60
    $sN = Wait-StealthSession $C.Api $kC 60
    $results['session_apres_rebinding'] = ($sN.sessions -ge 1)

    # ---------- Phase 4 : corpus + classifieur ----------------------
    Log-Host "== corpus de reference + analyse =="
    $kinds = @('dns','quic','wg','noise','ipv8')
    $capArgs = @()
    foreach ($k in $kinds) {
        $f = Join-Path $out "corpus_$k.pcap"
        & $Bench synth --kind $k --pcap $f --count 2000 | Out-Null
        $capArgs += "--cap=$k=$f"
    }
    $capArgs += "--cap=stealth=$pcap1"
    # `--up` omis : le premier emetteur consigne est le client (le
    # dial part toujours de lui) — l'auto-detection suffit.
    $report = @{
        analyze_stealth = (& $Bench analyze --pcap $pcap1 | ConvertFrom-Json)
        analyze_impaired = (& $Bench analyze --pcap $pcap2 | ConvertFrom-Json)
        classify = (& $Bench classify @capArgs --window-ms 2000 | ConvertFrom-Json)
    }
    $results['analysis'] = $report

    # ---------- Verdict ----------------------------------------------
    $a = $report.analyze_stealth
    $verdict = [ordered]@{
        oracle_silence            = ($results.probe_B.oracle_silence -and $results.probe_C.oracle_silence)
        oracle_amplification_le_1 = ($results.probe_B.oracle_amplification_le_1 -and `
                                     $results.probe_C.oracle_amplification_le_1 -and `
                                     $results.probe_flood_B.oracle_amplification_le_1)
        oracle_no_markers         = ($a.oracle_no_markers -and $report.analyze_impaired.oracle_no_markers)
        oracle_no_constants       = $a.oracle_no_constants
        oracle_entropy            = $a.oracle_entropy
        session_under_impairment  = $results.session_sous_impairments
        session_after_restart     = $results.session_apres_restart_pont
        session_after_rebinding   = $results.session_apres_rebinding
        session_survives_flood    = $results.session_survit_flood
    }
    $results['verdict'] = $verdict
    $results['pass'] = -not ($verdict.Values | Where-Object { -not $_ })

    @{
        run_utc = [datetime]::UtcNow.ToString('o')
        script = 'bench_stealth_fingerprint.ps1'
        duration_sec = $DurationSec
        ports = @{ tap = $TapPort; bridge = $B.Udp; client = $C.Udp }
        results = $results
    } | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $out 'report.json') -Encoding UTF8

    Log-Host ("== verdict global : " + $(if ($results['pass']) { 'PASS' } else { 'FAIL' }) + " ==")
    foreach ($k in $verdict.Keys) {
        Log-Host ("   {0,-28} {1}" -f $k, $(if ($verdict[$k]) { 'ok' } else { 'FAIL' }))
    }
    Log-Host "pcap : $pcap1, $pcap2 ; rapport : $(Join-Path $out 'report.json')"
}
finally {
    foreach ($kv in $procs.GetEnumerator()) {
        Stop-Process -Id $kv.Value.Id -Force -ErrorAction SilentlyContinue
    }
}
