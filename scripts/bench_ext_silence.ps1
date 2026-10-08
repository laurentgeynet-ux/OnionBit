# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# bench_ext_silence.ps1 - banc T1 ADR-0015 < legacy silence > :
# l'extension OnionBit-only (ADR-0015, Phase 9b/9d) cohabite avec un
# vrai Tribler.exe dans un mesh IPv8 ferme en loopback, sans que le
# trafic ext ne soit jamais destine a Tribler.
#
# Topologie (conventions fingerprint_mesh.ps1) :
#   A1 : OnionBit, ext enabled, ancre + exit
#   D  : OnionBit, ext enabled, curators = [cle ext de A1]
#   T  : Tribler.exe officiel, DiscoveryCommunity + TriblerTunnel
#
# Oracles (tous observables via les APIs, jamais < au ressenti >) :
#   - A1.ext.peer_count == 1 : seul D est marque ext - T, qui ne
#     repond jamais au `hello` ext, n'est JAMAIS promu pair ext et ne
#     recoit donc aucun ATTEST (le gossip ne cible que `ext_peers`).
#   - A1.ext.hello_probed / hello_tx bornes : le trafic ext vers T se
#     reduit a au plus un `hello` par `hello_cooldown` (comportement
#     opportuniste documente ADR-0015 2) - compteurs produits dans
#     le journal.
#   - D.ext.attest_stored >= 1 et score trust == 1 : l'attestation de
#     A1 a voyage entre pairs OnionBit.
#   - T : overlays statistics actives -> seules les communautes
#     legacy (Discovery, TriblerTunnel, DHT, content-discovery) -
#     aucun overlay ext ; son journal sans erreur de paquet.
#   - Legacy nominal : A1 voit >= 2 pairs discovery (D + T), T voit
#     des pairs sur Discovery + Tunnel.
#
# Journal : une ligne JSONL ajoutee a
# docs/plans/bench_adr0015/journal.jsonl (gitignore - prive).
#
# Usage : pwsh -NoProfile -ExecutionPolicy RemoteSigned `
#           -File scripts\bench_ext_silence.ps1 [-DurationSec 90]
#
# NOTE encodage : fichier volontairement ASCII.
param(
    [string]$Daemon      = "",
    [int]   $DurationSec = 90,
    [string]$OutDir      = ("target\bench-ext-silence-" + (Get-Date -Format 'yyyyMMdd-HHmmss')),
    [string]$TriblerExe  = $(if ($env:TRIBLER_EXE) { $env:TRIBLER_EXE } else { 'C:\Program Files (x86)\Tribler\Tribler.exe' })
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
$journal = Join-Path $root 'docs\plans\bench_adr0015\journal.jsonl'
New-Item -ItemType Directory -Force -Path $out | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $journal) | Out-Null

# Ports fixes du mesh (loopback ferme) - convention fingerprint_mesh.
$A1 = @{ Api = 8231; Ipv8 = 18731; Dir = (Join-Path $out 'a1'); Name = 'A1' }
$D  = @{ Api = 8232; Ipv8 = 18732; Dir = (Join-Path $out 'd');  Name = 'D'  }
$TApi = 52311; $TKey = 'extsilence'
$tIpv8 = 18733

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
function Git-Commit {
    try { return (git -C $root rev-parse --short HEAD 2>$null).Trim() } catch { return 'inconnu' }
}

$verdicts = [System.Collections.Generic.List[string]]::new()
function Oracle([string]$name, [bool]$ok, [string]$detail) {
    $verdicts.Add(("{0} [{1}] {2}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $detail))
    Log ("{0} [{1}] {2}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $detail)
}

$procs = @{}
$t0 = Get-Date
try {
    # ---------- A1 : ancre + exit + ext ---------------------------
    New-Item -ItemType Directory -Force -Path $A1.Dir | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $A1.Dir 'configuration.json'),
        (@{ tunnel_community = @{ enabled = $true; exitnode_enabled = $true };
            ext = @{ enabled = $true };
            ipv8 = @{ bootstrap = @{ override = @() };
                      interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $A1.Ipv8 } );
                      estimated_wan = "127.0.0.1:$($A1.Ipv8)" } } |
            ConvertTo-Json -Compress -Depth 6))
    $procs['A1'] = Start-OnionBit $A1
    $kA1 = Wait-ApiKey $A1.Dir; Wait-ApiUp $A1.Api $kA1

    # Cle publique ext de A1 -> `ext.curators` de D.
    $ovA1 = Invoke-RestMethod -Uri "http://127.0.0.1:$($A1.Api)/api/ipv8/overlays" `
        -Headers @{ 'X-Api-Key' = $kA1 } -TimeoutSec 10
    $a1pk = (@($ovA1.overlays) | Where-Object { $_.overlay_name -eq 'OnionbitExtCommunity' }).my_peer
    if (-not $a1pk) { throw "overlay OnionbitExtCommunity absent sur A1 (ext non active ?)" }
    Log "cle ext de A1 : $($a1pk.Substring(0, 16))..."

    # ---------- D : ext + suit A1 --------------------------------
    New-Item -ItemType Directory -Force -Path $D.Dir | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $D.Dir 'configuration.json'),
        (@{ tunnel_community = @{ enabled = $true };
            ext = @{ enabled = $true; curators = @($a1pk) };
            ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($A1.Ipv8)") };
                      interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $D.Ipv8 } );
                      estimated_wan = "127.0.0.1:$($D.Ipv8)" } } |
            ConvertTo-Json -Compress -Depth 6))
    $procs['D'] = Start-OnionBit $D
    $kD = Wait-ApiKey $D.Dir; Wait-ApiUp $D.Api $kD

    # ---------- T : Tribler officiel ------------------------------
    $tstate = Join-Path $out 'tribler-state'
    $confDir = Join-Path $tstate '8.0'
    New-Item -ItemType Directory -Force -Path $confDir | Out-Null
    $conf = @{
        api = @{ http_enabled = $true; http_port = $TApi; http_host = '127.0.0.1';
                 https_enabled = $false; key = $TKey }
        ipv8 = @{
            logger = @{ level = 'INFO' }
            interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $tIpv8 } )
            walker_interval = 5.0
            overlays = @(
                @{
                    class = 'DiscoveryCommunity'
                    key = 'anonymous id'
                    walkers = @(
                        @{ strategy = 'RandomWalk'; peers = 20; init = @{ timeout = 3.0 } }
                    )
                    bootstrappers = @(
                        @{ class = 'DispersyBootstrapper'; init = @{
                            ip_addresses = ,@('127.0.0.1', $A1.Ipv8)
                            dns_addresses = @()
                            bootstrap_timeout = 30.0
                        } }
                    )
                    initialize = @{}
                    on_start = @()
                }
            )
        }
        libtorrent = @{ port = 0; utp = $true; dht = $false; upnp = $false;
                        natpmp = $false; lsd = $false }
        tunnel_community = @{ enabled = $true; min_circuits = 0; max_circuits = 8 }
        dht_discovery = @{ enabled = $false }
        content_discovery = @{ enabled = $false }
        recommender = @{ enabled = $false }
        rendezvous = @{ enabled = $false }
        rss = @{ enabled = $false }
        torrent_checker = @{ enabled = $false }
        versioning = @{ enabled = $false }
        watch_folder = @{ enabled = $false }
        statistics = $true
    }
    [System.IO.File]::WriteAllText((Join-Path $confDir 'configuration.json'),
        ($conf | ConvertTo-Json -Depth 10), [System.Text.UTF8Encoding]::new($false))

    Log "== Tribler.exe -s (api :$TApi, ipv8 :$tIpv8) =="
    $env:TSTATEDIR = $tstate
    $env:CORE_API_PORT = "$TApi"
    $env:CORE_API_KEY = $TKey
    $procs['T'] = Start-Process -FilePath $TriblerExe -PassThru -NoNewWindow `
        -ArgumentList '-s','--log-level','INFO' `
        -RedirectStandardOutput (Join-Path $out 'tribler_stdout.log') `
        -RedirectStandardError (Join-Path $out 'tribler_stderr.log')
    Wait-ApiUp $TApi $TKey 120

    # ---------- Convergence : hello ext + discovery legacy --------
    Log "convergence ${DurationSec}s (hello ext + mesh legacy)..."
    Start-Sleep -Seconds $DurationSec

    # ---------- A1 publie une attestation ------------------------
    $subject = -join ((1..20) | ForEach-Object { 'ab' })
    try {
        Invoke-RestMethod -Method Post `
            -Uri "http://127.0.0.1:$($A1.Api)/api/ipv8/ext/attest" `
            -Headers @{ 'X-Api-Key' = $kA1 } `
            -Body (@{ kind = 'infohash'; subject = $subject; verdict = 'endorse' } |
                ConvertTo-Json -Compress) `
            -ContentType 'application/json' -TimeoutSec 10 | Out-Null
        Log "attestation publiee sur A1"
    } catch {
        throw "publication attestation A1 : $($_.Exception.Message)"
    }
    Start-Sleep -Seconds 15   # laisser le gossip se propager et s'eteindre

    # ---------- Collecte ------------------------------------------
    $extA1 = Get-Ext $A1.Api $kA1
    $extD  = Get-Ext $D.Api  $kD
    $ovA1  = Invoke-RestMethod -Uri "http://127.0.0.1:$($A1.Api)/api/ipv8/overlays" `
        -Headers @{ 'X-Api-Key' = $kA1 } -TimeoutSec 10
    $trust = Invoke-RestMethod `
        -Uri "http://127.0.0.1:$($D.Api)/api/ipv8/ext/trust/infohash/$subject" `
        -Headers @{ 'X-Api-Key' = $kD } -TimeoutSec 10
    # Statistiques d'overlays de T (Tribler REST : enable puis GET).
    $tOverlays = @()
    try {
        Invoke-RestMethod -Method Post `
            -Uri "http://127.0.0.1:$TApi/api/ipv8/overlays/statistics" `
            -Headers @{ 'X-Api-Key' = $TKey } `
            -Body '{"enable": true}' -ContentType 'application/json' `
            -TimeoutSec 10 | Out-Null
        Start-Sleep -Seconds 3
        $tStats = Invoke-RestMethod -Uri "http://127.0.0.1:$TApi/api/ipv8/overlays/statistics" `
            -Headers @{ 'X-Api-Key' = $TKey } -TimeoutSec 10
        $tOverlays = @($tStats.statistics | ForEach-Object { $_.PSObject.Properties.Name })
        if ($tOverlays.Count -eq 0 -and $tStats.PSObject.Properties.Name) {
            $tOverlays = @($tStats.PSObject.Properties.Name)
        }
    } catch {
        Log "WARN : statistics de T indisponibles : $($_.Exception.Message)"
    }
    $ovT = Invoke-RestMethod -Uri "http://127.0.0.1:$TApi/api/ipv8/overlays" `
        -Headers @{ 'X-Api-Key' = $TKey } -TimeoutSec 10

    # ---------- Oracles -------------------------------------------
    Oracle 'T-n-est-pas-pair-ext' ($extA1.ext.peer_count -eq 1) `
        "A1.ext.peer_count=$($extA1.ext.peer_count) (attendu 1 = D seul ; T jamais marque)"
    Oracle 'hello-opportuniste-borne' ($extA1.ext.hello_probed -le 10) `
        "A1.hello_probed=$($extA1.ext.hello_probed), hello_tx=$($extA1.ext.hello_tx)"
    Oracle 'attest-gossip-onionbit-seul' ($extD.ext.attest_stored -ge 1 -and $extA1.ext.attest_stored -ge 1) `
        "A1.stored=$($extA1.ext.attest_stored) tx=$($extA1.ext.attest_tx) ; D.stored=$($extD.ext.attest_stored) rx=$($extD.ext.attest_rx)"
    Oracle 'trust-score-suiveur' ($trust.trust.score -eq 1) `
        "D.trust.score=$($trust.trust.score) (attendu 1)"
    $legacyA1 = (@($ovA1.overlays) | Where-Object { $_.overlay_name -eq 'DiscoveryCommunity' })
    Oracle 'legacy-discovery-vu-A1' ($legacyA1 -and @($legacyA1.peers).Count -ge 2) `
        "A1 discovery peers=$(@($legacyA1.peers).Count) (attendu >= 2 : D + T)"
    $ovTnames = @($ovT.overlays | ForEach-Object { $_.overlay_name })
    Oracle 'T-overlays-legacy-seuls' (@($ovTnames) -notcontains 'OnionbitExtCommunity') `
        "T.overlays = [$($ovTnames -join ', ')]"
    Oracle 'T-legacy-peers' (@($ovT.overlays | Where-Object { @($_.peers).Count -ge 1 }).Count -ge 1) `
        "T voit des pairs sur >= 1 overlay legacy"
    if ($tOverlays.Count -gt 0) {
        Oracle 'T-stats-sans-ext' (@($tOverlays) -notcontains 'OnionbitExtCommunity') `
            "T.statistics overlays = [$($tOverlays -join ', ')]"
    }
    $terr = ''
    try {
        # Bruit banc exclu : « Need a DHT provider » — Tribler tente un
        # connect DHT pour un hop de circuit legacy alors que le mesh
        # ferme n'a aucune DHT (dht_discovery=false). Interne a
        # TriblerTunnelCommunity : nos trames ext sont droppees au
        # prefixe de communaute et n'atteignent jamais ce code.
        $terr = (Select-String -Path (Join-Path $out 'tribler_stdout.log') `
            -Pattern 'error|exception|traceback' -AllMatches |
            Where-Object { $_.Line -notmatch 'Need a DHT provider' } |
            Select-Object -First 3 | ForEach-Object { $_.Line }) -join ' | '
    } catch {}
    Oracle 'T-log-sans-erreur' ([string]::IsNullOrWhiteSpace($terr)) `
        $(if ($terr) { "T log : $terr" } else { 'aucun paquet ext signale en erreur cote T' })

    $pass = @($verdicts | Where-Object { $_ -like 'PASS*' }).Count
    $fail = @($verdicts | Where-Object { $_ -like 'FAIL*' }).Count
    $verdict = if ($fail -eq 0) { 'PASS' } else { "FAIL($fail)" }

    # ---------- Journal -------------------------------------------
    $line = (@{
        ts = [int][double]::Parse((Get-Date -UFormat %s))
        commit = Git-Commit
        scenario = 'T1-legacy-silence'
        ext_config = 'A1 ext.enabled ; D ext.enabled + curators=[A1pk] ; T=Tribler.exe officiel'
        topology = "mesh ferme loopback : A1(exit,ext) <- D(ext) + T(legacy)"
        duration_ms = [int]((Get-Date) - $t0).TotalMilliseconds
        counters = @{
            A1 = @{ peer_count = $extA1.ext.peer_count; attest_rx = $extA1.ext.attest_rx;
                    attest_dropped = $extA1.ext.attest_dropped; attest_stored = $extA1.ext.attest_stored;
                    attest_tx = $extA1.ext.attest_tx; hello_tx = $extA1.ext.hello_tx;
                    hello_probed = $extA1.ext.hello_probed }
            D  = @{ peer_count = $extD.ext.peer_count; attest_rx = $extD.ext.attest_rx;
                    attest_dropped = $extD.ext.attest_dropped; attest_stored = $extD.ext.attest_stored;
                    attest_tx = $extD.ext.attest_tx }
            T_overlays = $ovTnames
        }
        db_before_after = "D.attestations=$($extD.ext.attest_stored) ; score=$($trust.trust.score)"
        endpoint_volume = "voir target/*.log + $out\tribler_stdout.log"
        cpu_rss = $null
        legacy_verdict = $verdict
        oracles = $verdicts
        artifacts = $out
    } | ConvertTo-Json -Compress -Depth 6)
    Add-Content -Path $journal -Value $line -Encoding utf8
    Log "journal -> $journal"
    Log "== verdict global : $verdict ($pass PASS / $fail FAIL) - artefacts : $out =="
    if ($fail -gt 0) { exit 1 }
}
finally {
    foreach ($kv in $procs.GetEnumerator()) {
        Stop-Process -Id $kv.Value.Id -Force -ErrorAction SilentlyContinue
    }
}
