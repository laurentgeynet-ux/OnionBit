# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# mem_watchdog.ps1 -- garde-fou memoire du daemon OnionBit en charge.
#
# Contexte : docs/diagnostics/memoire_charge_reelle.md -- l'empreinte
# privee d'un daemon public n'est bornee que par des caps
# (max_joined_circuits, peer_limit par session, buffers uTP). Ce
# script echantillonne periodiquement la memoire du processus
# (PrivateMemorySize64, working set, handles) et les compteurs de
# charge de l'API (circuits par type/etat, relais joints, sorties,
# pairs BT, taille DB) dans un CSV, puis signale les derives au-dela
# des seuils (-Max*/-Warn*) : une memoire privee qui croit a compteurs
# stables est le signal d'une fuite ; parallele a la charge, c'est de
# la facture normale.
#
# Usage :
#   .\scripts\mem_watchdog.ps1 -StateDir C:\chemin\vers\state `
#       [-DurationMin 60] [-IntervalSec 30] [-OutCsv mem.csv]
#       [-MaxPrivateMB 400] [-WarnGrowthPct 25] [-WarnGrowthMB 64]
#       [-MaxCircuits 300] [-MaxRelays 120] [-MaxBtPeers 320]
#
# -StateDir sert a lire api.key + http_port_running dans
#   configuration.json et a retrouver le PID du daemon par sa ligne
#   de commande. -ApiBase/-ApiKey/-Pid restent forcables a la main
#   (daemon distant de la ligne de commande inconnue, etc.).
# -DurationMin 0 = surveillance illimitee (Ctrl+C pour arreter).
#
# NOTE encodage : fichier volontairement ASCII -- un caractere
# multi-octets lu en CP1252 par Windows PowerShell 5.1 peut casser le
# parsing (cf. fuzz_campaign.ps1).
param(
    [string]$StateDir = "",
    [string]$ApiBase = "",
    [string]$ApiKey = "",
    [int]$DaemonPid = 0,
    [int]$DurationMin = 60,
    [int]$IntervalSec = 30,
    [string]$OutCsv = "",
    [double]$MaxPrivateMB = 400,
    [double]$WarnGrowthPct = 25,
    [double]$WarnGrowthMB = 64,
    [int]$MaxCircuits = 300,
    [int]$MaxRelays = 120,
    [int]$MaxBtPeers = 320
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

function Log([string]$m) { Write-Host ('[{0:HH:mm:ss}] {1}' -f (Get-Date), $m) }

# ---------- Resolution API : state dir -> cle + port ----------
if ($StateDir -ne "" -and ($ApiKey -eq "" -or $ApiBase -eq "")) {
    $cfgPath = Join-Path $StateDir 'configuration.json'
    if (Test-Path $cfgPath) {
        $cfg = Get-Content $cfgPath -Raw | ConvertFrom-Json
        if ($ApiKey -eq "") { $ApiKey = $cfg.api.key }
        if ($ApiBase -eq "") {
            $port = if ($cfg.api.http_port_running) { [int]$cfg.api.http_port_running } else { [int]$cfg.api.http_port }
            $ApiBase = "http://127.0.0.1:$port"
        }
    }
}
if ($ApiBase -eq "") { $ApiBase = "http://127.0.0.1:8085" }
$headers = @{}
if ($ApiKey -ne "") { $headers['X-Api-Key'] = $ApiKey }

# ---------- Resolution PID : ligne de commande contenant StateDir --
if ($DaemonPid -eq 0) {
    $procs = @(Get-CimInstance Win32_Process -Filter "Name='onionbit-daemon.exe'")
    if ($StateDir -ne "") {
        $norm = $StateDir.TrimEnd('\', '/')
        $match = @($procs | Where-Object { $_.CommandLine -like "*$norm*" })
        if ($match.Count -ge 1) { $procs = $match }
    }
    if ($procs.Count -eq 0) {
        Log 'WARN : aucun processus onionbit-daemon.exe trouve -- compteurs API seuls (priv_mb=-1).'
    } else {
        $big = $procs | Sort-Object WorkingSetSize -Descending | Select-Object -First 1
        $DaemonPid = [int]$big.ProcessId
        if ($procs.Count -gt 1) {
            Log ("WARN : {0} daemons en cours, surveillance du plus gros (pid={1})" -f $procs.Count, $DaemonPid)
        }
    }
}
if ($DaemonPid -ne 0) { Log ("surveillance pid={0} api={1}" -f $DaemonPid, $ApiBase) }
else { Log ("surveillance api={0} (pas de processus local associe)" -f $ApiBase) }

# ---------- Sorties ----------
if ($OutCsv -eq "") { $OutCsv = "mem_watchdog_{0}.csv" -f (Get-Date -Format 'yyyyMMdd-HHmmss') }
$alertLog = [regex]::Replace($OutCsv, '\.csv$', '') + '.alerts.log'

function Alert([string]$m) {
    $line = "[{0:HH:mm:ss}] ALERTE {1}" -f (Get-Date), $m
    Write-Warning $m
    $line | Out-File $alertLog -Append -Encoding utf8
}

# Decimales CSV en culture invariante (virgule fr-FR sinon).
function Dbl([double]$v) {
    if ($v -lt 0) { return '-1' }
    return $v.ToString('0.0', [System.Globalization.CultureInfo]::InvariantCulture)
}

function Get-Json([string]$path) {
    try {
        return Invoke-RestMethod -Uri "$ApiBase/api/$path" -Headers $headers -TimeoutSec ([Math]::Max(2, $IntervalSec - 1))
    } catch { return $null }
}

"ts,pid,priv_mb,ws_mb,handles,circuits_total,circuits_ready,circuits_ip_seeder,circuits_data,relays,exits,tunnel_peers,ipv8_peers,bt_peers,downloads,num_torrents_db,db_mb,alerts" |
    Out-File $OutCsv -Encoding utf8

# ---------- Boucle d'echantillonnage ----------
$baselinePriv = $null
$deadline = if ($DurationMin -gt 0) { (Get-Date).AddMinutes($DurationMin) } else { [datetime]::MaxValue }
$samples = 0
while ((Get-Date) -lt $deadline) {
    $t0 = Get-Date
    $ts = $t0.ToString("yyyy-MM-ddTHH:mm:ss.fff")
    $flags = @()

    # -- Processus --
    $priv = -1.0; $ws = -1.0; $handles = -1
    if ($DaemonPid -ne 0) {
        $p = Get-Process -Id $DaemonPid -ErrorAction SilentlyContinue
        if ($null -eq $p) {
            $flags += 'PROCESSUS_MORT'
            Alert "pid=$DaemonPid termine -- arret de la surveillance."
        } else {
            $priv = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
            $ws = [math]::Round($p.WorkingSet64 / 1MB, 1)
            $handles = $p.HandleCount
        }
    }

    # -- API : compteurs de charge --
    $ct = 0; $cr = 0; $cip = 0; $cd = 0; $rl = 0; $ex = 0; $tp = 0
    $ip8 = 0; $bt = 0; $dl = 0; $nt = 0; $dbmb = 0.0

    $r = Get-Json 'ipv8/tunnel/circuits'
    if ($null -ne $r -and $r.circuits) {
        $items = @($r.circuits); $ct = $items.Count
        $cr = @($items | Where-Object { $_.state -eq 'READY' }).Count
        $cip = @($items | Where-Object { $_.type -eq 'IP_SEEDER' }).Count
        $cd = @($items | Where-Object { $_.type -eq 'DATA' }).Count
    }
    $r = Get-Json 'ipv8/tunnel/relays'; if ($null -ne $r -and $r.relays) { $rl = @($r.relays).Count }
    $r = Get-Json 'ipv8/tunnel/exits';  if ($null -ne $r -and $r.exits)  { $ex = @($r.exits).Count }
    $r = Get-Json 'ipv8/tunnel/peers';  if ($null -ne $r -and $r.peers)  { $tp = @($r.peers).Count }
    $r = Get-Json 'statistics/tribler'
    if ($null -ne $r -and $r.tribler_statistics) {
        $s = $r.tribler_statistics
        $ip8 = [int]$s.peers; $nt = [int]$s.num_torrents
        $dbmb = [math]::Round($s.db_size / 1MB, 1)
    }
    $r = Get-Json 'downloads'
    if ($null -ne $r -and $r.downloads) {
        $items = @($r.downloads); $dl = $items.Count
        foreach ($d in $items) {
            # Signal utile = connexions live : num_connected_peers
            # (num_peers/num_seeds incluent le scrape swarm du torrent
            # checker -- non borne par peer_limit). Champs absents sur
            # certains etats (magnet non resolu) : on somme ce qui est.
            $added = $false
            foreach ($f in @('num_connected_peers', 'num_peers')) {
                $prop = $d.PSObject.Properties[$f]
                if (-not $added -and $null -ne $prop -and $null -ne $prop.Value) {
                    $bt += [int]$prop.Value; $added = $true
                }
            }
        }
    }

    # -- Seuils --
    if ($priv -ge 0) {
        if ($null -eq $baselinePriv) { $baselinePriv = $priv }
        if ($priv -gt $MaxPrivateMB) {
            $flags += 'PRIV_DEPASSE'
            Alert ("priv_mb={0} > MaxPrivateMB={1} (relays={2} circuits={3} bt_peers={4})" -f $priv, $MaxPrivateMB, $rl, $ct, $bt)
        }
        $growth = $priv - $baselinePriv
        $growthPct = if ($baselinePriv -gt 0) { 100.0 * $growth / $baselinePriv } else { 0 }
        if ($growth -gt $WarnGrowthMB -and $growthPct -gt $WarnGrowthPct) {
            $flags += 'DERIVE_MEM'
            Alert ("priv_mb {0} -> {1} (+{2} Mo, +{3:N0}%% vs baseline) -- correler aux colonnes de charge" -f $baselinePriv, $priv, $growth, $growthPct)
        }
        if ($ct -gt $MaxCircuits) { $flags += 'CAP_CIRCUITS'; Alert "circuits_total=$ct > MaxCircuits=$MaxCircuits" }
        if ($rl -gt $MaxRelays)   { $flags += 'CAP_RELAYS';   Alert "relays=$rl > MaxRelays=$MaxRelays (cap max_joined_circuits viole ?)" }
        if ($bt -gt $MaxBtPeers)  { $flags += 'CAP_BT_PEERS'; Alert "bt_peers=$bt > MaxBtPeers=$MaxBtPeers (cap peer_limit x sessions viole ?)" }
    }

    '{0},{1},{2},{3},{4},{5},{6},{7},{8},{9},{10},{11},{12},{13},{14},{15},{16},{17}' -f `
        $ts, $DaemonPid, (Dbl $priv), (Dbl $ws), $handles, $ct, $cr, $cip, $cd, $rl, $ex, $tp, `
        $ip8, $bt, $dl, $nt, (Dbl $dbmb), ($flags -join '+') |
        Out-File $OutCsv -Append -Encoding utf8
    $samples++

    if ($flags -contains 'PROCESSUS_MORT') { break }

    $elapsed = ((Get-Date) - $t0).TotalSeconds
    $wait = $IntervalSec - $elapsed
    if ($wait -gt 0) { Start-Sleep -Seconds $wait }
}

Log ("mem_watchdog : {0} echantillons -> {1} (alertes : {2})" -f $samples, $OutCsv, $alertLog)
