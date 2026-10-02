# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# fingerprint_mesh.ps1 - session de fingerprinting en maillage
# controle : Tribler.exe et un daemon OnionBit cohabitent dans un
# meme mesh IPv8 ferme (ancre + 2 relais OnionBit), echantillonnes
# simultanement par fingerprint_stats.ps1 pendant -DurationMin.
#
# Alignement (critere de comparaison) : meme duree, meme mesh,
# memes communautes actives (ipv8 + tunnel + dht + content
# discovery), role idle pour les deux. Les relais A2/A3 servent de
# pool de candidats pour les circuits min_circuits des deux sujets.
#
# Usage :
#   .\scripts\fingerprint_mesh.ps1 [-Daemon path\to\onionbit-daemon.exe]
#                                  [-DurationMin 15] [-WithAnonDownload]
# CSV produits : <OutDir>\fp_onionbit_mesh.csv, fp_tribler_mesh.csv
#
# -WithAnonDownload : ajoute un magnet en stall (infohash factice,
# aucun pair dans le mesh) en anonyme 1 saut sur D et T. Cree la
# lane anonyme OnionBit (watchdog de circuits) — sans elle D ne
# construit pas min_circuits au repos alors que Tribler le fait
# proactivement : comparaison de cadence de circuits a perimetre
# egal.
#
# NOTE encodage : fichier volontairement ASCII.
param(
    [string]$Daemon     = "",
    [int]   $DurationMin = 15,
    [int]   $IntervalSec = 5,
    [switch]$WithAnonDownload,
    [string]$OutDir     = ("target\fingerprint-mesh-" + (Get-Date -Format 'yyyyMMdd-HHmmss')),
    [string]$TriblerExe = $(if ($env:TRIBLER_EXE) { $env:TRIBLER_EXE } else { 'C:\Program Files (x86)\Tribler\Tribler.exe' })
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$out  = Join-Path $root $OutDir
if ($Daemon -eq "") { $Daemon = Join-Path $root 'target\debug\onionbit-daemon.exe' }
New-Item -ItemType Directory -Force -Path $out | Out-Null

# Ports fixes du mesh (loopback ferme) — meme convention que
# interop_hidden_tribler_download.ps1.
$D  = @{ Api = 8096; Ipv8 = 17786; Dir = (Join-Path $out 'd');  Name = 'D'  }
$A1 = @{ Api = 8097; Ipv8 = 17787; Dir = (Join-Path $out 'a1'); Name = 'A1' }
$A2 = @{ Api = 8098; Ipv8 = 17788; Dir = (Join-Path $out 'a2'); Name = 'A2' }
$A3 = @{ Api = 8099; Ipv8 = 17789; Dir = (Join-Path $out 'a3'); Name = 'A3' }
$TApi = 52198; $TKey = 'fingerprint'

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

$procs = @{}
try {
    # ---------- Config noeuds OnionBit (mesh ferme, cf. interop) ----
    # `ipv8.bootstrap.override` remplace les bootstrappeurs publics ;
    # `interfaces` cloue chaque noeud au loopback ; `estimated_wan`
    # forcee pour que `on_node_discovered` accepte les pairs LAN.
    # A1 : racine + seul exit (EXIT_BT) — sortie imposee des circuits.
    New-Item -ItemType Directory -Force -Path $A1.Dir | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $A1.Dir 'configuration.json'),
        (@{ tunnel_community = @{ enabled = $true; exitnode_enabled = $true };
            ipv8 = @{ bootstrap = @{ override = @() };
                      interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $A1.Ipv8 } );
                      estimated_wan = "127.0.0.1:$($A1.Ipv8)" } } |
            ConvertTo-Json -Compress -Depth 6))
    $procs['A1'] = Start-OnionBit $A1
    $kA1 = Wait-ApiKey $A1.Dir; Wait-ApiUp $A1.Api $kA1

    $boot = @("127.0.0.1:$($A1.Ipv8)")
    foreach ($p in @($A2, $A3, $D)) {
        New-Item -ItemType Directory -Force -Path $p.Dir | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $p.Dir 'configuration.json'),
            (@{ tunnel_community = @{ enabled = $true };
                ipv8 = @{ bootstrap = @{ override = $boot };
                          interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $p.Ipv8 } );
                          estimated_wan = "127.0.0.1:$($p.Ipv8)" } } |
                ConvertTo-Json -Compress -Depth 6))
        $procs[$p.Name] = Start-OnionBit $p
        $k = Wait-ApiKey $p.Dir; Wait-ApiUp $p.Api $k
        if ($p.Name -eq 'D') { $kD = $k }
    }

    # ---------- Config Tribler (meme mesh, communautes alignees) ---
    $tstate = Join-Path $out 'tribler-state'
    $confDir = Join-Path $tstate '8.0'
    New-Item -ItemType Directory -Force -Path $confDir | Out-Null
    $tIpv8 = 17792
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
                        @{ strategy = 'RandomChurn'; peers = -1; init = @{ sample_size = 8; ping_interval = 10.0; inactive_time = 27.5; drop_time = 57.5 } }
                        @{ strategy = 'PeriodicSimilarity'; peers = -1; init = @{} }
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
        tunnel_community = @{ enabled = $true; min_circuits = 3; max_circuits = 8 }
        dht_discovery = @{ enabled = $true }
        content_discovery = @{ enabled = $true }
        recommender = @{ enabled = $false }
        rendezvous = @{ enabled = $false }
        rss = @{ enabled = $false }
        torrent_checker = @{ enabled = $false }
        versioning = @{ enabled = $false }
        watch_folder = @{ enabled = $false }
        statistics = $false
    }
    # Sans BOM : json.load de Tribler rejette le BOM.
    [System.IO.File]::WriteAllText((Join-Path $confDir 'configuration.json'),
        ($conf | ConvertTo-Json -Depth 10), [System.Text.UTF8Encoding]::new($false))

    Log "== Tribler.exe -s (mesh, api :$TApi) =="
    $env:TSTATEDIR = $tstate
    $env:CORE_API_PORT = "$TApi"
    $env:CORE_API_KEY = $TKey
    $procs['T'] = Start-Process -FilePath $TriblerExe -PassThru -NoNewWindow `
        -ArgumentList '-s','--log-level','INFO' `
        -RedirectStandardOutput (Join-Path $out 'tribler_stdout.log') `
        -RedirectStandardError (Join-Path $out 'tribler_stderr.log')
    Wait-ApiUp $TApi $TKey 90

    # ---------- Lane anonyme (option -WithAnonDownload) ----------
    # Magnet factice (infohash bidon, aucun pair dans le mesh) :
    # force la creation de la lane anonyme et le maintien de
    # min_circuits, sans aucun flux de donnees utile.
    if ($WithAnonDownload) {
        $magnet = 'magnet:?xt=urn:btih:0000000000000000000000000000000000000001&dn=fp-probe'
        foreach ($t in @(
            @{ name = 'D'; api = $D.Api; key = $kD },
            @{ name = 'T'; api = $TApi;  key = $TKey }
        )) {
            try {
                Invoke-RestMethod -Method Put `
                    -Uri "http://127.0.0.1:$($t.api)/api/downloads" `
                    -Headers @{ 'X-Api-Key' = $t.key } `
                    -Body (@{ uri = $magnet; anon_hops = 1; safe_seeding = $true } |
                        ConvertTo-Json -Compress) `
                    -ContentType 'application/json' -TimeoutSec 15 | Out-Null
                Log "download anonyme ajoute sur $($t.name)"
            } catch {
                Log "WARN : add download anonyme $($t.name) : $($_.Exception.Message)"
            }
        }
    }

    # ---------- Manifeste ----------
    @{
        run_utc = [datetime]::UtcNow.ToString('o')
        script  = 'fingerprint_mesh.ps1'
        daemon  = $Daemon
        tribler = $TriblerExe
        duration_min = $DurationMin
        anon_download = [bool]$WithAnonDownload
        nodes = @(
            @{ name = 'D';  api = $D.Api;  ipv8 = $D.Ipv8;  role = 'onionbit echantillonne' },
            @{ name = 'A1'; api = $A1.Api; ipv8 = $A1.Ipv8; role = 'ancre + exit' },
            @{ name = 'A2'; api = $A2.Api; ipv8 = $A2.Ipv8; role = 'relais' },
            @{ name = 'A3'; api = $A3.Api; ipv8 = $A3.Ipv8; role = 'relais' },
            @{ name = 'T';  api = $TApi;   ipv8 = $tIpv8;   role = 'tribler echantillonne' }
        )
    } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'manifest.json') -Encoding UTF8

    # ---------- Echantillonnage simultane ----------
    Log "== echantillonnage ${DurationMin} min (intervalle ${IntervalSec}s) =="
    $fpScript = Join-Path $root 'scripts\fingerprint_stats.ps1'
    $dBase = "http://127.0.0.1:$($D.Api)"
    $tBase = "http://127.0.0.1:$TApi"
    $csvD = Join-Path $out 'fp_onionbit_mesh.csv'
    $csvT = Join-Path $out 'fp_tribler_mesh.csv'
    $jobD = Start-Job -ScriptBlock {
        & $using:fpScript -ApiBase $using:dBase `
            -ApiKey $using:kD -DurationMin $using:DurationMin `
            -IntervalSec $using:IntervalSec -OutCsv $using:csvD
    }
    $jobT = Start-Job -ScriptBlock {
        & $using:fpScript -ApiBase $using:tBase `
            -ApiKey $using:TKey -DurationMin $using:DurationMin `
            -IntervalSec $using:IntervalSec -OutCsv $using:csvT
    }
    $jobD, $jobT | Wait-Job | Out-Null
    $jobD, $jobT | Receive-Job
    Log "== termine : $(Join-Path $out 'fp_onionbit_mesh.csv'), $(Join-Path $out 'fp_tribler_mesh.csv') =="
}
finally {
    foreach ($kv in $procs.GetEnumerator()) {
        Stop-Process -Id $kv.Value.Id -Force -ErrorAction SilentlyContinue
    }
}
