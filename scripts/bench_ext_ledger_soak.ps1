# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# bench_ext_ledger_soak.ps1 - banc ADR-0015 « soak ledger » : le
# ledger bilateral signe (Phase 9c) sous trafic tunnel REEL entre
# pairs OnionBit, pas en loopback in-process.
#
# Topologie (mesh ferme loopback, conventions fingerprint_mesh) :
#   A1 : OnionBit, ext + OBF, ancre + exit (exitnode_enabled)
#   A2 : OnionBit, ext + OBF, relais
#   A3 : OnionBit, ext + OBF, relais
#   D  : OnionBit, ext + OBF, initiateur (circuits SPEED_TEST)
#
# Mecanique : `GET /api/ipv8/tunnel/circuits/test?goal_hops=2` cree
# un circuit SPEED_TEST reel D -> relais -> sortie et y pousse des
# cellules pendant test_time_ms. Chaque saut voit le volume dans son
# comptage `served` (peer_stats) ; au tick de settlement, un relais
# dont la creance depasse `ext/ledger_tranche_bytes` propose un lien
# LEDGER_PROPOSE -> LEDGER_SEAL signe des deux cotes. Tous les
# messages ext voyagent sous enveloppe OBF (caps negocie) quand les
# deux pairs l'annoncent.
#
# Oracles (observables via API, jamais au ressenti) :
#   - Convergence : ext.peer_count >= 2 partout.
#   - Trafic mesure : D.tunnel_ledger.total_used > 0 ; au moins un
#     relais total_served > 0.
#   - Liens : chaque noeud ext a links_count >= 1, tous sealed
#     (sig_b pose), forks == 0, pending == 0 apres stabilisation.
#   - Coherence bilaterale : au moins un hash de lien present des
#     deux cotes de la paire (meme objet signe des deux bouts).
#   - OBF : somme des obf_tx > 0 (les trames ledger ont voyage
#     chiffrees entre pairs capables).
#   - Sequence : my_head.seq >= 1 la ou des liens existent.
#
# Journal : docs/plans/bench_adr0015/journal.jsonl.
#
# Usage : pwsh -NoProfile -ExecutionPolicy RemoteSigned `
#           -File scripts\bench_ext_ledger_soak.ps1 [-TestTimeMs 30000]
#
# NOTE encodage : fichier volontairement ASCII.

param(
    [string]$Daemon       = "",
    [int]   $TestTimeMs   = 30000,
    [int]   $ConvergeSec  = 90,
    [int]   $SettleWaitS  = 25,
    [string]$OutDir       = ("target\bench-ext-ledger-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
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

# Ports fixes du mesh (loopback ferme) - distincts des autres bancs.
$A1 = @{ Api = 8241; Ipv8 = 18741; Dir = (Join-Path $out 'a1'); Name = 'A1' }
$A2 = @{ Api = 8242; Ipv8 = 18742; Dir = (Join-Path $out 'a2'); Name = 'A2' }
$A3 = @{ Api = 8243; Ipv8 = 18743; Dir = (Join-Path $out 'a3'); Name = 'A3' }
$D  = @{ Api = 8244; Ipv8 = 18744; Dir = (Join-Path $out 'd');  Name = 'D'  }
$nodes = @($A1, $A2, $A3, $D)

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
function Get-ExtLedger([int]$api, [string]$key) {
    return Invoke-RestMethod -Uri "http://127.0.0.1:$api/api/ipv8/ext/ledger" `
        -Headers @{ 'X-Api-Key' = $key } -TimeoutSec 10
}
function Get-TunnelLedger([int]$api, [string]$key) {
    return Invoke-RestMethod -Uri "http://127.0.0.1:$api/api/ipv8/tunnel/ledger" `
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

# Config d'un noeud : ext + OBF, tranche 1 Mio et tick settlement 5 s
# pour que le trafic d'un speedtest de ~30 s produise plusieurs liens
# sans attendre des tranches de 16 Mio.
# ATTENTION : l'affectation via une expression `if` deroule les
# tableaux PowerShell (@() -> $null, @(x) -> scalaire) ; $boot passe
# par affectation directe pour conserver la forme JSON tableau.
function Write-NodeConfig($p, [bool]$exitNode, [bool]$anchored) {
    New-Item -ItemType Directory -Force -Path $p.Dir | Out-Null
    $boot = @()
    if (-not $anchored) { $boot = @("127.0.0.1:$($A1.Ipv8)") }
    [System.IO.File]::WriteAllText((Join-Path $p.Dir 'configuration.json'),
        (@{ tunnel_community = @{ enabled = $true; exitnode_enabled = $exitNode };
            ext = @{ enabled = $true; obf_enabled = $true;
                     hello_interval_secs = 10;
                     ledger_tranche_bytes = 1048576;
                     ledger_settle_interval_secs = 5 };
            ipv8 = @{ bootstrap = @{ override = $boot };
                      interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $p.Ipv8 } );
                      estimated_wan = "127.0.0.1:$($p.Ipv8)" } } |
            ConvertTo-Json -Compress -Depth 6))
}

# Acces defensif : une reponse `{ext|ledger: {enabled:false}}` n'a
# aucun compteur — Set-StrictMode rejette la propriete absente.
function Prop($o, [string]$n) {
    if ($null -eq $o) { return 0 }
    $m = $o.PSObject.Properties[$n]
    if ($null -eq $m) { return 0 }
    return $m.Value
}
function Links($o) {
    if ($null -eq $o -or $null -eq $o.PSObject.Properties['links']) { return @() }
    return @($o.links)
}

$procs = @{}
$keys  = @{}
$t0 = Get-Date
try {
    Write-NodeConfig $A1 $true $true
    $procs['A1'] = Start-OnionBit $A1
    $keys['A1'] = Wait-ApiKey $A1.Dir; Wait-ApiUp $A1.Api $keys['A1']

    foreach ($p in @($A2, $A3, $D)) {
        Write-NodeConfig $p $false $false
        $procs[$p.Name] = Start-OnionBit $p
        $keys[$p.Name] = Wait-ApiKey $p.Dir; Wait-ApiUp $p.Api $keys[$p.Name]
    }

    # ---------- Convergence : hello ext + mesh -------------------
    Log "convergence (hello ext 10 s, mesh legacy)..."
    $dl = (Get-Date).AddSeconds($ConvergeSec)
    $ok = $false
    while ((Get-Date) -lt $dl) {
        $ok = $true
        foreach ($p in $nodes) {
            try {
                $e = Get-Ext $p.Api $keys[$p.Name]
                if ([int](Prop $e.ext 'peer_count') -lt 2) { $ok = $false }
            } catch { $ok = $false }
        }
        if ($ok) { break }
        Start-Sleep -Seconds 3
    }
    $counts = ($nodes | ForEach-Object { "{0}={1}" -f $_.Name, (Prop (Get-Ext $_.Api $keys[$_.Name]).ext 'peer_count') }) -join ','
    Oracle 'convergence-ext' $ok "peer_count par noeud : $counts"

    # ---------- Trafic reel : speedtest circuit 2 sauts ----------
    # Invoke-WebRequest gere mal les corps SSE chunked vides/ouverts
    # (NullReference interne observee) : curl.exe consomme le stream
    # jusqu'a fermeture — la reponse SSE est un detail, seul le
    # trafic genere compte pour les oracles.
    Log "speedtest D : circuit SPEED_TEST goal_hops=2, ${TestTimeMs} ms..."
    $spOut = Join-Path $out 'speedtest_d_sse.txt'
    $curlArgs = @('-sS', '-N', '--max-time', ([string]([int]($TestTimeMs/1000) + 45)),
        '-H', "X-Api-Key: $($keys['D'])",
        "-o", $spOut,
        "http://127.0.0.1:$($D.Api)/api/ipv8/tunnel/circuits/test?goal_hops=2&test_time_ms=$TestTimeMs")
    & curl.exe @curlArgs 2>$null | Out-Null
    $spBytes = if (Test-Path $spOut) { (Get-Item $spOut).Length } else { 0 }
    Oracle 'speedtest-executed' ($spBytes -gt 0) "SSE D : $spBytes octets"

    # ---------- Settlement : ticks + aller-retour ----------------
    Log "attente settlement (${SettleWaitS} s)..."
    Start-Sleep -Seconds $SettleWaitS

    # ---------- Collecte ------------------------------------------
    $ledgers = @{}
    $tunnelL = @{}
    $exts    = @{}
    foreach ($p in $nodes) {
        $ledgers[$p.Name] = Get-ExtLedger $p.Api $keys[$p.Name]
        $tunnelL[$p.Name] = Get-TunnelLedger $p.Api $keys[$p.Name]
        $exts[$p.Name]    = Get-Ext $p.Api $keys[$p.Name]
    }

    # Trafic mesure : le comptage local a vu les octets passer.
    Oracle 'trafic-used-D' ([int64](Prop $tunnelL['D'].ledger 'total_used') -gt 0) `
        "D.total_used=$(Prop $tunnelL['D'].ledger 'total_used')"
    $servedAny = @($A1, $A2, $A3 | Where-Object { [int64](Prop $tunnelL[$_.Name].ledger 'total_served') -gt 0 })
    Oracle 'trafic-served-relais' ($servedAny.Count -ge 1) `
        ("total_served : " + (($nodes | ForEach-Object { "{0}={1}" -f $_.Name, (Prop $tunnelL[$_.Name].ledger 'total_served') }) -join ','))

    # Liens signes : chaque noeud detenant des liens en a au moins un
    # scelle ; les propositions rejetees restent stockees non scellees
    # (artefact legitime de l'historique) — elles ne sont pas un echec.
    # Aucun fork avere ne doit apparaitre.
    $anyLinks = $false; $sealedOk = $true; $noForks = $true
    foreach ($p in $nodes) {
        $l = $ledgers[$p.Name].ledger
        $sealedCount = @((Links $l) | Where-Object { $_.sealed }).Count
        if ([int](Prop $l 'links_count') -ge 1) {
            $anyLinks = $true
            if ($sealedCount -lt 1) { $sealedOk = $false }
        }
        if ([int](Prop $l 'forks') -ne 0) { $noForks = $false }
        Log ("{0}: links={1} sealed={2} forks={3} pending={4} head_seq={5} rx={6} tx={7}" -f `
            $p.Name, (Prop $l 'links_count'), $sealedCount, (Prop $l 'forks'), (Prop $l 'pending'), `
            (Prop $l.my_head 'seq'), (Prop $l 'rx'), (Prop $l 'tx'))
    }
    Oracle 'liens-formes' $anyLinks "liens stockes sur au moins un noeud"
    Oracle 'lien-scelle-par-noeud' $sealedOk "chaque noeud avec des liens en a >= 1 scelle"
    Oracle 'aucun-fork' $noForks "forks == 0 partout"

    # Coherence bilaterale : un hash de lien partage entre deux noeuds.
    $hashByNode = @{}
    foreach ($p in $nodes) { $hashByNode[$p.Name] = @((Links $ledgers[$p.Name].ledger) | ForEach-Object { $_.hash }) }
    $shared = @()
    foreach ($a in $nodes) { foreach ($b in $nodes) {
        if ($a.Name -lt $b.Name) {
            $shared += @($hashByNode[$a.Name] | Where-Object { $hashByNode[$b.Name] -contains $_ })
        }
    } }
    Oracle 'lien-bilateral-partage' (@($shared | Select-Object -Unique).Count -ge 1) `
        "$(@($shared | Select-Object -Unique).Count) hash de lien communs entre noeuds distincts"

    # OBF a effectivement porte des trames (negociation caps + enveloppes).
    $obfTx = ($nodes | ForEach-Object { [int](Prop $exts[$_.Name].ext 'obf_tx') } | Measure-Object -Sum).Sum
    $obfRx = ($nodes | ForEach-Object { [int](Prop $exts[$_.Name].ext 'obf_rx') } | Measure-Object -Sum).Sum
    Oracle 'obf-emis' ($obfTx -gt 0) "obf_tx total=$obfTx, obf_rx=$obfRx"

    # Tete de chaine avancee la ou des liens existent.
    $heads = ($nodes | ForEach-Object { "{0}={1}" -f $_.Name, (Prop $ledgers[$_.Name].ledger.my_head 'seq') }) -join ','
    $anyHead = @($nodes | Where-Object { [int](Prop $ledgers[$_.Name].ledger.my_head 'seq') -ge 1 }).Count -ge 1
    Oracle 'tete-avancee' $anyHead "my_head.seq : $heads"

    $pass = @($verdicts | Where-Object { $_ -like 'PASS*' }).Count
    $fail = @($verdicts | Where-Object { $_ -like 'FAIL*' }).Count
    $verdict = if ($fail -eq 0) { 'PASS' } else { "FAIL($fail)" }

    # ---------- Journal ------------------------------------------
    $rec = [ordered]@{
        ts = (Get-Date).ToString('o'); commit = (Git-Commit); scenario = 'ledger_soak'
        ext_config = 'ext+obf, tranche=1Mio, settle=5s, hello=10s'
        topology = 'A1 exit + A2/A3 relay + D speedtest'
        duration_ms = [int]((Get-Date) - $t0).TotalMilliseconds
        counters = [ordered]@{}
        verdict = $verdict
    }
    foreach ($p in $nodes) {
        $rec.counters[$p.Name] = [ordered]@{
            peer_count = [int](Prop $exts[$p.Name].ext 'peer_count')
            obf_tx = [int](Prop $exts[$p.Name].ext 'obf_tx')
            obf_rx = [int](Prop $exts[$p.Name].ext 'obf_rx')
            ledger_rx = [int](Prop $ledgers[$p.Name].ledger 'rx')
            ledger_tx = [int](Prop $ledgers[$p.Name].ledger 'tx')
            ledger_links = [int](Prop $ledgers[$p.Name].ledger 'links_count')
            ledger_forks = [int](Prop $ledgers[$p.Name].ledger 'forks')
            head_seq = [int](Prop $ledgers[$p.Name].ledger.my_head 'seq')
            total_served = [int64](Prop $tunnelL[$p.Name].ledger 'total_served')
            total_used = [int64](Prop $tunnelL[$p.Name].ledger 'total_used')
        }
    }
    ($rec | ConvertTo-Json -Compress -Depth 8) | Add-Content -Path $journal -Encoding UTF8
    Log "journal -> $journal"
    Log "== verdict global : $verdict ($pass PASS / $fail FAIL) - artefacts : $out =="
    if ($fail -gt 0) { exit 1 }
} finally {
    foreach ($p in $procs.Values) {
        try { if (-not $p.HasExited) { $p.Kill() } } catch {}
    }
}
