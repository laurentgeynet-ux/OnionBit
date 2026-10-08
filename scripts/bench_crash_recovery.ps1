# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# bench_crash_recovery.ps1 — CH-6 : crash-recovery en boucle.
#
# Cycle : demarrer le daemon (--offline, pas de reseau), attendre
# l'API, ajouter un magnet (ecriture SQLite), puis taskkill -F
# (simulation crash) a un instant pseudo-aleatoire — parfois en plein
# milieu de la rafale d'ecritures. Redemarrage sur le MEME state-dir,
# verification que l'API remonte et que les downloads persistes sont
# tous restitues. A la fin : `PRAGMA quick_check` sur onionbit.db.
#
# Usage :
#   pwsh -NoProfile -ExecutionPolicy RemoteSigned -File scripts\bench_crash_recovery.ps1
#   ... -Cycles 8 -StateDir D:\tmp\crash-state

param(
    [int]$Cycles = 5,
    [string]$StateDir = "",
    [string]$DaemonExe = "",
    [int]$ApiWaitSec = 60,
    # .torrent distincts alternes entre cycles (le magnet n'est
    # persiste qu'apres resolution des metadonnees — jamais en
    # --offline : le chemin teste est `torrent_data` persiste).
    [string[]]$TorrentFiles = @()
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


$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if ($DaemonExe -eq "") { $DaemonExe = Join-Path $root "target\debug\onionbit-daemon.exe" }
if ($StateDir -eq "") {
    $StateDir = Join-Path $root ("target\crash-recovery-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
}
New-Item -ItemType Directory -Force -Path $StateDir | Out-Null
$logFile = Join-Path $StateDir 'crash_recovery.log'

function Log([string]$m) {
    $line = '[{0:HH:mm:ss}] {1}' -f (Get-Date), $m
    Write-Host $line
    Add-Content -Path $logFile -Value $line
}
$script:fails = 0
function Verdict([bool]$ok, [string]$name, [string]$detail = "") {
    if ($ok) { Log ("OK   {0} {1}" -f $name, $detail) }
    else     { Log ("FAIL {0} {1}" -f $name, $detail); $script:fails++ }
}

$confFile = Join-Path $StateDir 'configuration.json'
$dbFile = Join-Path $StateDir 'onionbit.db'

function Get-Api {
    # Relit la config (port reel publie dans http_port_running).
    $conf = Get-Content $confFile -Raw | ConvertFrom-Json
    $port = if ($conf.api.http_port_running -gt 0) { $conf.api.http_port_running } else { $conf.api.http_port }
    @{ port = [int]$port; key = $conf.api.key }
}

function Wait-Daemon([int]$pidToWait) {
    $deadline = (Get-Date).AddSeconds($ApiWaitSec)
    while ((Get-Date) -lt $deadline) {
        if (-not (Get-Process -Id $pidToWait -ErrorAction SilentlyContinue)) { return $null }
        if (Test-Path $confFile) {
            try {
                $api = Get-Api
                if ($api.port -gt 0) {
                    $r = Invoke-RestMethod -Uri "http://127.0.0.1:$($api.port)/api/downloads" `
                        -Headers @{ "X-Api-Key" = $api.key } -TimeoutSec 3
                    if ($null -ne $r) { return $api }
                }
            } catch { Start-Sleep -Milliseconds 500 }
        }
        Start-Sleep -Milliseconds 500
    }
    return $null
}

# .torrent de banc : resolus instantanement en --offline et persistes
# immediatement (`persist_torrent`) — c'est la ligne `downloads` que
# le crash doit conserver.
if ($TorrentFiles.Count -eq 0) {
    $TorrentFiles = @(
        (Join-Path $root 'vendor\librqbit\resources\ubuntu-21.04-desktop-amd64.iso.torrent'),
        (Join-Path $root 'vendor\librqbit\resources\ubuntu-21.04-live-server-amd64.iso.torrent')
    )
}
foreach ($f in $TorrentFiles) {
    if (-not (Test-Path $f)) { throw "torrent de banc introuvable : $f" }
}

Log "=== CH-6 crash-recovery : $Cycles cycle(s), state=$StateDir ==="
$expectedHashes = [System.Collections.Generic.HashSet[string]]::new()
$rng = [System.Random]::new()

for ($i = 1; $i -le $Cycles; $i++) {
    $errLog = Join-Path $StateDir "daemon-c$i-err.log"
    $outLog = Join-Path $StateDir "daemon-c$i-out.log"
    $proc = Start-Process -FilePath $DaemonExe -PassThru -NoNewWindow `
        -ArgumentList '--state-dir', "`"$StateDir`"", '--offline', '--no-tray' `
        -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    $api = Wait-Daemon $proc.Id
    if (-not $api) {
        Verdict $false "cycle $i : API montee" "pid=$($proc.Id)"
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        continue
    }
    Log "cycle $i : API montee sur :$($api.port) (pid $($proc.Id))"

    # Ecriture : ajoute un .torrent puis tue le daemon a un instant
    # pseudo-aleatoire (0-1200 ms) — au milieu des commits SQLite.
    $tf = $TorrentFiles[($i - 1) % $TorrentFiles.Count]
    try {
        $bytes = [System.IO.File]::ReadAllBytes($tf)
        $r = Invoke-RestMethod -Method Put -Uri "http://127.0.0.1:$($api.port)/api/downloads" `
            -Headers @{ "X-Api-Key" = $api.key } -ContentType 'application/octet-stream' `
            -Body $bytes -TimeoutSec 15
        if ($r.infohash) { [void]$expectedHashes.Add($r.infohash) }
    } catch {
        Log "cycle $i : ajout download a echoue ($_)"
    }
    Start-Sleep -Milliseconds ($rng.Next(0, 1200))
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    $proc.WaitForExit()
    Log "cycle $i : daemon tue (taskkill -F) apres ajout de $(Split-Path $tf -Leaf)"

    # Redemarrage sur le meme state-dir : la base doit rouvrir saine et
    # restituer TOUS les downloads persistes avant le crash. Attente de
    # `checkpoints.all_loaded` — la restauration est asynchrone.
    $proc2 = Start-Process -FilePath $DaemonExe -PassThru -NoNewWindow `
        -ArgumentList '--state-dir', "`"$StateDir`"", '--offline', '--no-tray' `
        -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    $api2 = Wait-Daemon $proc2.Id
    Verdict ($null -ne $api2) "cycle $i : API remontee apres crash"
    if ($api2) {
        try {
            $dls = $null
            $deadline2 = (Get-Date).AddSeconds($ApiWaitSec)
            while ((Get-Date) -lt $deadline2) {
                $dls = Invoke-RestMethod -Uri "http://127.0.0.1:$($api2.port)/api/downloads" `
                    -Headers @{ "X-Api-Key" = $api2.key } -TimeoutSec 10
                # `all_loaded` ou au moins le compte attendu deja
                # restitue (le drapeau peut rester bas en --offline).
                if ($dls.checkpoints.all_loaded -or
                        @($dls.downloads).Count -ge $expectedHashes.Count) { break }
                Start-Sleep -Milliseconds 500
            }
            $restored = @($dls.downloads | ForEach-Object { $_.infohash })
            $missing = @($expectedHashes | Where-Object { $_ -notin $restored })
            Verdict ($missing.Count -eq 0 -and $restored.Count -ge $expectedHashes.Count) `
                "cycle $i : downloads restitues" "$($restored.Count) listes, $($missing.Count) manquants"
        } catch {
            Verdict $false "cycle $i : lectures post-crash" "$_"
        }
    }
    Stop-Process -Id $proc2.Id -Force -ErrorAction SilentlyContinue
    $proc2.WaitForExit()
}

# ---------- Integrite SQLite ----------
if (Test-Path $dbFile) {
    $py = 'python'
    if ($env:TRIBLER_INTEROP_PY -and (Test-Path $env:TRIBLER_INTEROP_PY)) { $py = $env:TRIBLER_INTEROP_PY }
    $chk = & $py -c "import sqlite3;print(sqlite3.connect(r'$dbFile').execute('PRAGMA quick_check').fetchone()[0])" 2>&1
    Verdict ($chk -match 'ok') 'PRAGMA quick_check' "$chk"
} else {
    Verdict $false 'onionbit.db present'
}

Log ""
if ($script:fails -eq 0) { Log "CRASH RECOVERY OK"; exit 0 }
Log "CRASH RECOVERY ECHEC ($($script:fails) verdict(s))"
exit 1
