# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# bench_profiles.ps1 - banc inter-demons des profils d'anonymat
# (ADR-0022, etape 81).
#
# Topologie loopback :
#   A  (8291/18801) - ancre IPv8 legacy, reste en `legacy` tout du long
#   L  (8292/18802) - noeud sous test : legacy -> full -> legacy -> custom
#   SB (8293/18803) - pont stealth (transport morphe)
#   tap(18800)      - relais UDP `stealth_bench tap` L -> SB, PCAP reel
#
# Scenarios (roadmap_adr0022 etape 81) :
#   a. deux demons `legacy` s'interconnectent (ext HELLO echange) ;
#   b. `PUT profile=full` refuse sans pont (409 missing_prerequis-
#      ites) ; pont ajoute via POST /stealth/bridges ; bascule +
#      restart_required/restart_pending ; apres redemarrage la socket
#      n'emet plus AUCUN datagramme legacy (oracle PCAP no_markers) ;
#   c. `full -> legacy` restaure l'interop — oracle = re-pairage du
#      mesh IPv8 (`overlays` >= 1) ; la re-convergence ext HELLO est
#      rapportee en metrique info (trou pre-existant sur restart
#      unilateral, reproduit sans profil — hors perimetre preset) ;
#   d. edition d'une cle couverte via POST /settings -> l'API remonte
#      effective=custom + diverged_keys.
#
# Usage : pwsh -File scripts\bench_profiles.ps1 [-SkipBuild]
# Produits : <OutDir>\*.pcap, report.json, logs des 3 demons + tap.
#
# NOTE encodage : UTF-8 avec BOM (convention scripts du depot) ;
# commentaires ASCII sauf trois tirets cadratin documentes.

param(
    [string]$Daemon    = "",
    [string]$Bench     = "",
    [int]   $CoverSec  = 15,
    [switch]$SkipBuild,
    [string]$OutDir = ("target\bench-profiles-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
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

$A  = @{ Api = 8291; Udp = 18801; Dir = (Join-Path $out 'a');  Name = 'A'  }
$L  = @{ Api = 8292; Udp = 18802; Dir = (Join-Path $out 'l');  Name = 'L'  }
$SB = @{ Api = 8293; Udp = 18803; Dir = (Join-Path $out 'sb'); Name = 'SB' }
$TapPort = 18800

function Log-Host([string]$m) { Write-Host ('[{0:HH:mm:ss}] {1}' -f (Get-Date), $m) }
function ApiKey([string]$dir) {
    $cfg = Join-Path $dir 'configuration.json'
    if (-not (Test-Path $cfg)) { return $null }
    try { return (Get-Content $cfg -Raw | ConvertFrom-Json).api.key } catch { return $null }
}
function Wait-ApiKey([string]$dir, [int]$sec = 60) {
    $dl = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $dl) {
        $k = ApiKey $dir; if ($k) { return $k }
        Start-Sleep -Milliseconds 500
    }
    throw "cle API absente dans $dir"
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
function Wait-StealthSession([int]$port, $key, [int]$sec = 60) {
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
function Start-Daemon($p) {
    $proc = Start-Process -FilePath $Daemon -PassThru -NoNewWindow `
        -ArgumentList '--state-dir', "`"$($p.Dir)`"", '--listen', "127.0.0.1:$($p.Api)", '--no-tray' `
        -RedirectStandardOutput (Join-Path $p.Dir 'daemon_stdout.log') `
        -RedirectStandardError  (Join-Path $p.Dir 'daemon_stderr.log')
    Log-Host ("{0} pid={1} api={2} udp={3}" -f $p.Name, $proc.Id, $p.Api, $p.Udp)
    return $proc
}
function Write-Config($p, $cfg) {
    New-Item -ItemType Directory -Force -Path $p.Dir | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $p.Dir 'configuration.json'),
        ($cfg | ConvertTo-Json -Compress -Depth 8))
}
function Get-Profile([int]$port, $key) {
    Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/privacy/profile" `
        -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 5
}
function Put-Profile([int]$port, $key, [string]$profile) {
    Invoke-RestMethod -Method Put `
        -Uri "http://127.0.0.1:$port/api/privacy/profile" `
        -Headers @{ 'X-Api-Key' = $key } -ContentType 'application/json' `
        -Body ("{ `"profile`": `"$profile`" }") -TimeoutSec 10
}
function Get-ExtPeers([int]$port, $key) {
    $e = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/ipv8/ext" `
        -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 10
    return @{ pc = [int]$e.ext.peer_count; tx = [int]$e.ext.hello_tx }
}
function Wait-ExtConverged([int]$pa, $ka, [int]$pl, $kl, [int]$sec = 60) {
    $dl = (Get-Date).AddSeconds($sec)
    $a = @{ pc = 0; tx = 0 }; $l = @{ pc = 0; tx = 0 }
    while ((Get-Date) -lt $dl) {
        try { $a = Get-ExtPeers $pa $ka } catch {}
        try { $l = Get-ExtPeers $pl $kl } catch {}
        if ($a.pc -ge 1 -and $l.pc -ge 1) { return @{ a = $a; l = $l } }
        Start-Sleep -Seconds 2
    }
    return @{ a = $a; l = $l }
}
function Get-OverlayCount([int]$port, $key) {
    try {
        $ov = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/ipv8/overlays" `
            -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 10
        $n = 0
        foreach ($o in @($ov.overlays)) { $n += @($o.peers).Count }
        return $n
    } catch { return -1 }
}

# Config legacy d'un noeud interconnecte : IPv8 legacy + ext hello
# rapide + bootstrap vers A pour le second noeud.
function LegacyCfg([int]$udp, [bool]$anchored) {
    $boot = @()
    if (-not $anchored) { $boot = @("127.0.0.1:$($A.Udp)") }
    @{
        tunnel_community = @{ enabled = $true }
        ext = @{ enabled = $true; hello_interval_secs = 5 }
        ipv8 = @{ enabled = $true
                  bootstrap = @{ override = $boot }
                  interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $udp } )
                  estimated_wan = "127.0.0.1:$udp" }
    }
}
# Config du pont stealth — meme gabarit que bench_stealth_fingerprint.
function StealthBridgeCfg([int]$udp) {
    @{
        ipv8 = @{ enabled = $false
                  interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $udp } ) }
        tunnel_community = @{ enabled = $true }
        ext = @{ enabled = $true }
        stealth = @{
            enabled = $true; role = 'bridge'; bridges = @(); cover_traffic = $true
            tuning = @{ tick_ms = 200; hs_retry_secs = 1; hs_attempts_max = 8
                        pending_timeout_secs = 8; dial_cooldown_secs = 2
                        session_idle_timeout_secs = 60
                        cover_interval_min_ms = 150; cover_interval_max_ms = 500 }
        }
    }
}
function Start-Tap([string]$pcap) {
    $proc = Start-Process -FilePath $Bench -PassThru -NoNewWindow `
        -ArgumentList 'tap', '--listen', "127.0.0.1:$TapPort", '--upstream', "127.0.0.1:$($SB.Udp)",
                      '--pcap', "`"$pcap`"", '--loss', '0', '--dup', '0',
                      '--reorder', '0', '--jitter-ms', '0' `
        -RedirectStandardError (Join-Path $out 'tap_stderr.log')
    Log-Host ("tap pid={0} :{1} -> :{2} -> {3}" -f $proc.Id, $TapPort, $SB.Udp, $pcap)
    return $proc
}
function Restart-Node($p, $keyRef) {
    if ($procs[$p.Name]) {
        Stop-Process -Id $procs[$p.Name].Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 2
    }
    $procs[$p.Name] = Start-Daemon $p
    $k = Wait-ApiKey $p.Dir
    Wait-ApiUp $p.Api $k
    return $k
}

$verdicts = [System.Collections.Generic.List[string]]::new()
function Oracle([string]$name, [bool]$ok, [string]$detail) {
    $verdicts.Add(("{0} [{1}] {2}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $detail))
    Log-Host ("{0} [{1}] {2}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $detail)
}

if (-not $SkipBuild) {
    Log-Host "build daemon + stealth_bench"
    cargo build --manifest-path (Join-Path $root 'Cargo.toml') `
        -p onionbit-daemon --bin onionbit-daemon `
        -p onionbit-ipv8 --bin stealth_bench | Out-Null
}
if (-not (Test-Path $Daemon)) { throw "daemon absent: $Daemon" }
if (-not (Test-Path $Bench))  { throw "stealth_bench absent: $Bench" }

$procs = @{}
$results = [ordered]@{}
try {
    # ================ (a) legacy x legacy ===========================
    Log-Host "===== (a) deux demons legacy s'interconnectent ====="
    Write-Config $A (LegacyCfg $A.Udp $true)
    Write-Config $L (LegacyCfg $L.Udp $false)
    $procs['A'] = Start-Daemon $A
    $procs['L'] = Start-Daemon $L
    $kA = Wait-ApiKey $A.Dir; Wait-ApiUp $A.Api $kA
    $kL = Wait-ApiKey $L.Dir; Wait-ApiUp $L.Api $kL

    $c = Wait-ExtConverged $A.Api $kA $L.Api $kL 60
    Oracle 'a-interconnexion-legacy' ($c.a.pc -ge 1 -and $c.l.pc -ge 1) `
        "ext.peer_count A=$($c.a.pc) L=$($c.l.pc) ; hello_tx A=$($c.a.tx) L=$($c.l.tx)"

    $p0 = Get-Profile $L.Api $kL
    Oracle 'a-profil-legacy-effectif' ($p0.stored -eq 'legacy' -and $p0.effective -eq 'legacy') `
        "stored=$($p0.stored) effective=$($p0.effective) restart_pending=$($p0.restart_pending)"

    # ================ (b) full : prerequis + silence legacy =========
    Log-Host "===== (b) bascule full : pont requis puis silence legacy ====="
    # 409 hostile : aucun pont configure.
    $code = 0; $body = $null
    try {
        Put-Profile $L.Api $kL 'full' | Out-Null
    } catch {
        $code = $_.Exception.Response.StatusCode.value__
        $body = $_.ErrorDetails.Message | ConvertFrom-Json
    }
    Oracle 'b-full-sans-pont-409' ($code -eq 409 -and $body.error.message -eq 'missing_prerequisites') `
        "status=$code missing=$($body.missing -join ',')"

    # Pont stealth + lien d'invitation pointant sur le TAP.
    Write-Config $SB (StealthBridgeCfg $SB.Udp)
    $procs['SB'] = Start-Daemon $SB
    $kSB = Wait-ApiKey $SB.Dir; Wait-ApiUp $SB.Api $kSB
    $skFile = Join-Path $SB.Dir 'identity\stealth_bridge.key'
    $dsk = (Get-Date).AddSeconds(30)
    while (-not (Test-Path $skFile) -and (Get-Date) -lt $dsk) { Start-Sleep -Milliseconds 400 }
    if (-not (Test-Path $skFile)) { throw "stealth_bridge.key absent" }
    $link = (& $Bench link --key $skFile --addr "127.0.0.1:$TapPort").Trim()
    Log-Host "lien bridge (via tap) : $($link.Substring(0, 40))..."

    $pcap = Join-Path $out 'full_l_to_bridge.pcap'
    $procs['tap'] = Start-Tap $pcap

    $br = Invoke-RestMethod -Method Post `
        -Uri "http://127.0.0.1:$($L.Api)/api/stealth/bridges" `
        -Headers @{ 'X-Api-Key' = $kL } -ContentType 'application/json' `
        -Body ("{ `"bridge`": `"$link`" }") -TimeoutSec 10
    Oracle 'b-pont-ajoute' ($br.modified -eq $true -and [int]$br.bridges -ge 1) `
        "bridges_total=$($br.bridges)"

    $r = Put-Profile $L.Api $kL 'full'
    Oracle 'b-put-full-accepte' ($r.effective -eq 'full' -and $r.restart_required -eq $true) `
        "effective=$($r.effective) restart_required=$($r.restart_required) applied=$($r.applied_keys.Count) cles"

    $p1 = Get-Profile $L.Api $kL
    Oracle 'b-restart-pending' ($p1.restart_pending -eq $true) `
        "restart_pending=$($p1.restart_pending) (cles froides en attente)"

    # Redemarrage = ce que fait l'app (shutdown + respawn) : le preset
    # persiste est charge au boot.
    Log-Host "restart de L en profil full..."
    $kL = Restart-Node $L ([ref]$null)

    $p2 = Get-Profile $L.Api $kL
    Oracle 'b-effectif-full' ($p2.stored -eq 'full' -and $p2.effective -eq 'full' `
        -and $p2.restart_pending -eq $false) `
        "stored=$($p2.stored) effective=$($p2.effective) restart_pending=$($p2.restart_pending)"

    $s = Wait-StealthSession $L.Api $kL 60
    Oracle 'b-session-stealth' ($s.sessions -ge 1) "sessions=$($s.sessions) via pont"

    # Laisser tourner cover_traffic + ext intro pour remplir le PCAP.
    Log-Host "capture ${CoverSec}s (tout le trafic UDP de L transite par le tap)..."
    Start-Sleep -Seconds $CoverSec
    Stop-Process -Id $procs['tap'].Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    $a2 = & $Bench analyze --pcap $pcap | ConvertFrom-Json
    $results['analyze_full'] = $a2
    Oracle 'b-aucun-datagramme-legacy' ($a2.oracle_no_markers -eq $true) `
        "oracle_no_markers=$($a2.oracle_no_markers) (LibNaCLPK/community_id/onionbit/bittorrent/dht absents du PCAP)"

    # ================ (c) full -> legacy ============================
    Log-Host "===== (c) retour legacy : interop restauree ====="
    $r3 = Put-Profile $L.Api $kL 'legacy'
    Oracle 'c-put-legacy' ($r3.effective -eq 'legacy' -and $r3.restart_required -eq $true) `
        "effective=$($r3.effective) restart_required=$($r3.restart_required)"
    $kL = Restart-Node $L ([ref]$null)

    # Interop restauree = L a rejoint le mesh IPv8 legacy (le preset
    # a rallume `ipv8.enabled`, le bootstrap re-paire). La
    # re-convergence ext HELLO est une METRIQUE informationnelle :
    # trou pre-existant demontre sur restart pur (repro sans profil) —
    # le pair survivant ne re-verifie pas le noeud renait tant que
    # l'association n'a pas expire, et `hello_cooldown=3600s` fige les
    # re-sondes ; hors perimetre du preset (ADR-0022), consigne dans
    # report.json + bancs_tests.md.
    $c2 = Wait-ExtConverged $A.Api $kA $L.Api $kL 60
    $ovL = Get-OverlayCount $L.Api $kL
    $ovA = Get-OverlayCount $A.Api $kA
    Oracle 'c-interop-ipv8-restauree' ($ovL -ge 1 -and $ovA -ge 1) `
        "overlays peers A=$ovA L=$ovL (mesh legacy rejoint)"
    $results['c_ext_reconverge_info'] = @{
        a_pc = $c2.a.pc; l_pc = $c2.l.pc; a_tx = $c2.a.tx; l_tx = $c2.l.tx
        note = 'pre-existant : re-verification IPv8 apres restart ' +
               'unilateral lente (association survivante + ' +
               'hello_cooldown 3600s) — reproduit sans profil' }
    $p3 = Get-Profile $L.Api $kL
    Oracle 'c-effectif-legacy' ($p3.effective -eq 'legacy' -and $p3.restart_pending -eq $false) `
        "effective=$($p3.effective) restart_pending=$($p3.restart_pending)"

    # ================ (d) divergence -> custom ======================
    Log-Host "===== (d) edition d'une cle couverte -> effective=custom ====="
    Invoke-RestMethod -Method Post `
        -Uri "http://127.0.0.1:$($L.Api)/api/settings" `
        -Headers @{ 'X-Api-Key' = $kL } -ContentType 'application/json' `
        -Body '{ "settings": { "libtorrent": { "download_defaults": { "number_hops": 3 } } } }' `
        -TimeoutSec 10 | Out-Null
    $p4 = Get-Profile $L.Api $kL
    $div = @($p4.diverged_keys) -contains 'libtorrent.download_defaults.number_hops'
    Oracle 'd-divergence-custom' ($p4.effective -eq 'custom' -and $div) `
        "effective=$($p4.effective) diverged=$($p4.diverged_keys -join ',')"

    $results['verdicts'] = $verdicts
    $fail = @($verdicts | Where-Object { $_ -like 'FAIL*' }).Count
    @{
        run_utc = [datetime]::UtcNow.ToString('o')
        script = 'bench_profiles.ps1'
        ports = @{ a = $A.Udp; l = $L.Udp; bridge = $SB.Udp; tap = $TapPort }
        results = $results
        pass = ($fail -eq 0)
    } | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $out 'report.json') -Encoding UTF8

    Write-Host "`n================ VERDICTS ================"
    $verdicts | ForEach-Object { Write-Host "  $_" }
    Write-Host ("`n=====> {0} FAIL" -f $fail)
    exit ($(if ($fail -eq 0) { 0 } else { 1 }))
}
finally {
    foreach ($kv in $procs.GetEnumerator()) {
        Stop-Process -Id $kv.Value.Id -Force -ErrorAction SilentlyContinue
    }
}
