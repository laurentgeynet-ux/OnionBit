# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# bench_ext_interconnect.ps1 - banc ADR-0015 « ext off vs on » :
# deux instances OnionBit sur le meme mesh loopback, une fois avec
# ext.enabled=false explicite, une fois avec ext.enabled=true (le
# defaut produit).
#
# Demonstration oracles :
#   Phase A : les deux daemons se decouvrent en IPv8 legacy (overlay
#             DiscoveryCommunity peuple) MAIS ext desactive -> les
#             nœuds ne se reconnaissent JAMAIS comme OnionBit.
#   Phase B : memes daemons relances avec ext.enabled=true -> HELLO
#             signe echange, ext.peer_count >= 1 des deux cotes en
#             quelques secondes.
#
# Journal : docs/plans/bench_adr0015/journal.jsonl.
#
# Usage : powershell -NoProfile -ExecutionPolicy RemoteSigned `
#           -File scripts\bench_ext_interconnect.ps1
#
# NOTE encodage : fichier volontairement ASCII.

param(
    [string]$Daemon    = "",
    [int]   $PhaseSec  = 40,
    [string]$OutDir    = ("target\bench-ext-interconnect-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$out  = Join-Path $root $OutDir
if ($Daemon -eq "") { $Daemon = Join-Path $root 'target\debug\onionbit-daemon.exe' }
$journal = Join-Path $root 'docs\plans\bench_adr0015\journal.jsonl'
New-Item -ItemType Directory -Force -Path $out | Out-Null

# Ports dedies (plage 826x/1876x — sans collision avec les autres bancs).
$A = @{ Api = 8261; Ipv8 = 18761; Dir = (Join-Path $out 'a'); Name = 'A' }
$B = @{ Api = 8262; Ipv8 = 18762; Dir = (Join-Path $out 'b'); Name = 'B' }
$nodes = @($A, $B)

function Log([string]$m) { Write-Host ('[{0:HH:mm:ss}] {1}' -f (Get-Date), $m) }
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
function Wait-ApiUp([int]$port, $key, [int]$sec = 60) {
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
function Start-OnionBit($p) {
    $argList = @('--state-dir', ('"{0}"' -f $p.Dir), '--listen', "127.0.0.1:$($p.Api)", '--no-tray')
    $psi = New-Object System.Diagnostics.ProcessStartInfo($Daemon)
    $psi.Arguments = $argList -join ' '
    $psi.UseShellExecute = $false
    $proc = [System.Diagnostics.Process]::Start($psi)
    Log ("{0} demarre pid={1} api={2} ipv8={3}" -f $p.Name, $proc.Id, $p.Api, $p.Ipv8)
    return $proc
}
function Get-Ext([int]$api, [string]$key) {
    return Invoke-RestMethod -Uri "http://127.0.0.1:$api/api/ipv8/ext" `
        -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 10
}
function Get-OverlayPeers([int]$api, [string]$key) {
    # Nombre de pairs connus dans les overlays (preuve d'interconnexion
    # IPv8 legacy independante de l'ext).
    $ov = Invoke-RestMethod -Uri "http://127.0.0.1:$api/api/ipv8/overlays" `
        -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 10
    $n = 0
    foreach ($o in @($ov.overlays)) { $n += @($o.peers).Count }
    return $n
}

$verdicts = [System.Collections.Generic.List[string]]::new()
function Oracle([string]$name, [bool]$ok, [string]$detail) {
    $verdicts.Add(("{0} [{1}] {2}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $detail))
    Log ("{0} [{1}] {2}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $detail)
}

# Config d'un noeud. $ExtOn=$false ecrit ext.enabled=false
# explicitement (le defaut produit est `true` depuis le switch
# ADR-0015 — l'absence de cle activerait l'ext). B boote sur A.
function Write-NodeConfig($p, [bool]$extOn, [bool]$anchored) {
    New-Item -ItemType Directory -Force -Path $p.Dir | Out-Null
    $boot = @()
    if (-not $anchored) { $boot = @("127.0.0.1:$($A.Ipv8)") }
    $cfg = @{ tunnel_community = @{ enabled = $true; exitnode_enabled = $false };
              ext = @{ enabled = $extOn; hello_interval_secs = 5 };
              ipv8 = @{ bootstrap = @{ override = $boot };
                        interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $p.Ipv8 } );
                        estimated_wan = "127.0.0.1:$($p.Ipv8)" } }
    [System.IO.File]::WriteAllText((Join-Path $p.Dir 'configuration.json'),
        ($cfg | ConvertTo-Json -Compress -Depth 6))
}

$procs = @{}
$keys  = @{}
$t0 = Get-Date
try {
    # ================= Phase A : ext desactive =================
    Log "===== PHASE A : ext.enabled=false ====="
    Write-NodeConfig $A $false $true
    Write-NodeConfig $B $false $false
    $procs['A'] = Start-OnionBit $A
    $procs['B'] = Start-OnionBit $B
    $keys['A'] = Wait-ApiKey $A.Dir; Wait-ApiUp $A.Api $keys['A']
    $keys['B'] = Wait-ApiKey $B.Dir; Wait-ApiUp $B.Api $keys['B']

    # Laisser le walk IPv8 interconnecter les deux noeuds.
    Log "walk IPv8 en cours (${PhaseSec}s)..."
    Start-Sleep -Seconds $PhaseSec

    $ovA = Get-OverlayPeers $A.Api $keys['A']
    $ovB = Get-OverlayPeers $B.Api $keys['B']
    Oracle 'A-interconnexion-legacy' ($ovA -ge 1 -and $ovB -ge 1) `
        "overlays peers A=$ovA B=$ovB (les deux daemons se voient en IPv8)"

    foreach ($p in $nodes) {
        $e = Get-Ext $p.Api $keys[$p.Name]
        $en = $false; $pc = 0
        if ($null -ne $e -and $null -ne $e.ext) {
            if ($e.ext.PSObject.Properties['enabled']) { $en = [bool]$e.ext.enabled }
            if ($e.ext.PSObject.Properties['peer_count']) { $pc = [int]$e.ext.peer_count }
        }
        Oracle "A-ext-off-$($p.Name)" ((-not $en) -and $pc -eq 0) `
            "ext.enabled=$en peer_count=$pc (aucune reconnaissance ext)"
    }

    foreach ($p in $nodes) { if ($procs[$p.Name]) { Stop-Process -Id $procs[$p.Name].Id -Force -ErrorAction SilentlyContinue } }
    $procs.Clear()
    Start-Sleep -Seconds 3

    # ================= Phase B : ext.enabled=true (defaut produit) =================
    Log "===== PHASE B : ext.enabled=true ====="
    Write-NodeConfig $A $true $true
    Write-NodeConfig $B $true $false
    $procs['A'] = Start-OnionBit $A
    $procs['B'] = Start-OnionBit $B
    $keys['A'] = Wait-ApiKey $A.Dir; Wait-ApiUp $A.Api $keys['A']
    $keys['B'] = Wait-ApiKey $B.Dir; Wait-ApiUp $B.Api $keys['B']

    # Convergence : hello ext toutes les 5 s.
    Log "convergence ext (hello 5 s, fenetre ${PhaseSec}s)..."
    $dl = (Get-Date).AddSeconds($PhaseSec)
    $okA = $false; $okB = $false; $pcA = 0; $pcB = 0
    while ((Get-Date) -lt $dl -and -not ($okA -and $okB)) {
        Start-Sleep -Seconds 2
        try { $pcA = [int](Get-Ext $A.Api $keys['A']).ext.peer_count } catch { $pcA = 0 }
        try { $pcB = [int](Get-Ext $B.Api $keys['B']).ext.peer_count } catch { $pcB = 0 }
        $okA = ($pcA -ge 1); $okB = ($pcB -ge 1)
    }
    Oracle 'B-interconnexion-ext' ($okA -and $okB) `
        "ext.peer_count A=$pcA B=$pcB (HELLO echange, pair reconnu des deux cotes)"

    # Les trames HELLO ont bien ete emises (pas un peer_count importe).
    $eA = Get-Ext $A.Api $keys['A']; $eB = Get-Ext $B.Api $keys['B']
    $hA = 0; $hB = 0
    if ($eA.ext.PSObject.Properties['hello_tx']) { $hA = [int]$eA.ext.hello_tx }
    if ($eB.ext.PSObject.Properties['hello_tx']) { $hB = [int]$eB.ext.hello_tx }
    Oracle 'B-hello-emis' ($hA -ge 1 -and $hB -ge 1) "hello_tx A=$hA B=$hB"

    # CAP_MSG_V1 (bit 1 de hello.caps, ADR-0011/0015) : les deux
    # noeuds ont tunnel_community.messaging_enabled=true par defaut
    # -> chacun annonce "msg_v1" localement et l'observe sur le pair.
    $capLoc = ($eA.ext.caps_names -contains 'msg_v1') -and ($eB.ext.caps_names -contains 'msg_v1')
    $capPA = @($eA.ext.peers | Where-Object { $_.caps_names -contains 'msg_v1' }).Count
    $capPB = @($eB.ext.peers | Where-Object { $_.caps_names -contains 'msg_v1' }).Count
    Oracle 'B-cap-msg-v1' ($capLoc -and $capPA -ge 1 -and $capPB -ge 1) `
        "msg_v1 local=$capLoc pairs_observees A=$capPA B=$capPB (decouverte messagerie)"

    # ---------- Journal ----------
    $commit = 'inconnu'
    try { $commit = (git -C $root rev-parse --short HEAD 2>$null).Trim() } catch {}
    $rec = @{
        ts = [int][double]::Parse((Get-Date -UFormat %s))
        commit = $commit
        scenario = 'T8-defaut-vs-ext'
        ext_config = 'phase A: ext.enabled=false explicite ; phase B: ext.enabled=true (defaut produit) + hello 5s'
        topology = 'loopback 2 noeuds A(ancre)-B ; meme mesh relance deux fois'
        duration_ms = [int]((Get-Date) - $t0).TotalMilliseconds
        counters = @{ A = @{ overlay_peers = $ovA; ext_peer_count = $pcA; hello_tx = $hA };
                      B = @{ overlay_peers = $ovB; ext_peer_count = $pcB; hello_tx = $hB } }
        endpoint_volume = @{ note = 'datagrammes loopback ; pas de trafic tunnel (demo interconnexion)' }
        cpu_rss = $null
        legacy_verdict = 'n/a — loopback OnionBit-only (silence prouve par T1)'
        cases = ($verdicts -join ' ; ')
    } | ConvertTo-Json -Compress -Depth 6
    [System.IO.File]::AppendAllText($journal, $rec + "`n")

    Write-Host "`n================ VERDICTS ================"
    $verdicts | ForEach-Object { Write-Host "  $_" }
    $fail = @($verdicts | Where-Object { $_ -like 'FAIL*' }).Count
    Write-Host ("`n=====> {0} FAIL" -f $fail)
    exit ($(if ($fail -eq 0) { 0 } else { 1 }))
}
finally {
    foreach ($p in $nodes) {
        if ($procs[$p.Name] -and -not $procs[$p.Name].HasExited) {
            Stop-Process -Id $procs[$p.Name].Id -Force -ErrorAction SilentlyContinue
        }
    }
}
