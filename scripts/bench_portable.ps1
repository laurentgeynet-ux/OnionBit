# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# bench_portable.ps1 - ADR-0018 etape 63 : portabilite du bundle
# `state/` + `data/`.
#
# Scenarios (--offline, aucune fuite reseau) :
#   A. Demarrage sur <work>\A, ajout d'un .torrent public + prive
#      (area=private), arret propre.
#   B. Copie du bundle complet vers <work>\B (autre chemin = lettre de
#      lecteur/point de montage different en usage reel), redemarrage :
#      API identique, downloads restitues (public ET prive via
#      manifest.obm), oracle "zero chemin absolu de A ni lettre de
#      lecteur dans les artefacts persistes" (configuration.json,
#      session.json rqbit, onionbit.db) - sauf chemins explicitement
#      externes choisis par l'utilisateur (aucun ici).
#   C. Identite etrangere : bundle <work>\C avec son propre `state/`
#      (nouvelle identite) mais le `data/private` de A recopie :
#      `GET /api/private` doit lister zero download (manifeste OBM
#      indechiffrable, groupes .obd inertes/orphelins).
#
# Usage :
#   pwsh -NoProfile -ExecutionPolicy RemoteSigned -File scripts\bench_portable.ps1
#   ... -DaemonExe target\release\onionbit-daemon.exe -WorkDir D:\tmp\portable

param(
    [string]$DaemonExe = "",
    [string]$WorkDir = "",
    [int]$ApiWaitSec = 60,
    # Le torrent PRIVE doit etre distinct du public : l'infohash est la
    # cle moteur - un meme .torrent ne peut vivre dans les deux zones.
    [string]$TorrentFile = "",
    [string]$TorrentFilePrivate = "",
    [switch]$Keep
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
if ($TorrentFile -eq "") {
    $TorrentFile = Join-Path $root 'vendor\librqbit\resources\ubuntu-21.04-desktop-amd64.iso.torrent'
}
if ($TorrentFilePrivate -eq "") {
    $TorrentFilePrivate = Join-Path $root 'vendor\librqbit\resources\ubuntu-21.04-live-server-amd64.iso.torrent'
}
if ($WorkDir -eq "") {
    $WorkDir = Join-Path $root ("target\bench-portable-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
}
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
$logFile = Join-Path $WorkDir 'bench_portable.log'

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

function Start-Daemon([string]$stateDir, [string]$tag) {
    $outLog = Join-Path $WorkDir "daemon-$tag-out.log"
    $errLog = Join-Path $WorkDir "daemon-$tag-err.log"
    return Start-Process -FilePath $DaemonExe -PassThru -NoNewWindow `
        -ArgumentList '--state-dir', "`"$stateDir`"", '--offline', '--no-tray' `
        -RedirectStandardOutput $outLog -RedirectStandardError $errLog
}

function Get-Api([string]$stateDir) {
    $confFile = Join-Path $stateDir 'configuration.json'
    if (-not (Test-Path $confFile)) { return $null }
    $conf = Get-Content $confFile -Raw | ConvertFrom-Json
    $port = if ($conf.api.http_port_running -gt 0) { $conf.api.http_port_running } else { $conf.api.http_port }
    @{ port = [int]$port; key = $conf.api.key }
}

function Wait-Daemon([int]$pidToWait, [string]$stateDir) {
    $deadline = (Get-Date).AddSeconds($ApiWaitSec)
    while ((Get-Date) -lt $deadline) {
        if (-not (Get-Process -Id $pidToWait -ErrorAction SilentlyContinue)) { return $null }
        $api = Get-Api $stateDir
        if ($api -and $api.port -gt 0) {
            try {
                $r = Invoke-RestMethod -Uri "http://127.0.0.1:$($api.port)/api/downloads" `
                    -Headers @{ "X-Api-Key" = $api.key } -TimeoutSec 3
                if ($null -ne $r) { return $api }
            } catch { Start-Sleep -Milliseconds 400 }
        }
        Start-Sleep -Milliseconds 400
    }
    return $null
}

function Stop-Daemon($proc, $api) {
    try {
        Invoke-RestMethod -Method Put -Uri "http://127.0.0.1:$($api.port)/api/shutdown" `
            -Headers @{ "X-Api-Key" = $api.key } -TimeoutSec 5 | Out-Null
    } catch { }
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline -and -not $proc.HasExited) { Start-Sleep -Milliseconds 300 }
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    $proc.WaitForExit()
}

# Oracle "zero chemin absolu" : les fichiers texte persistes ne
# doivent contenir ni lettre de lecteur ni UNC (les specs
# @root/@public/@private sont attendues) ; les fichiers binaires
# (onionbit.db) sont verifies par recherche exacte du chemin source —
# un regex de chemin sur des octets binaires produirait du bruit.
function Scan-AbsPaths([string]$dir, [string]$oldRoot) {
    $hits = @()
    $rx = [regex]'(?<![A-Za-z0-9])[A-Za-z]:[\\/][A-Za-z0-9_.\\/-]+|\\\\[^\\/\\\\]+[\\/][^\\/\\\\]+'
    $textTargets = Get-ChildItem -Path $dir -Recurse -File -Include 'session.json','configuration.json' -ErrorAction SilentlyContinue |
        ForEach-Object { $_.FullName }
    $binTargets = Get-ChildItem -Path $dir -Recurse -File -Include '*.db' -ErrorAction SilentlyContinue |
        ForEach-Object { $_.FullName }
    foreach ($f in $textTargets) {
        $raw = Get-Content $f -Raw
        foreach ($m in $rx.Matches($raw)) { $hits += "$(Split-Path $f -Leaf): $($m.Value)" }
    }
    foreach ($f in $binTargets) {
        $raw = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($f))
        if ($raw.Contains($oldRoot)) { $hits += "$(Split-Path $f -Leaf): chemin source persiste" }
    }
    return $hits
}

if (-not (Test-Path $TorrentFile)) { throw "torrent de banc introuvable : $TorrentFile" }
if (-not (Test-Path $TorrentFilePrivate)) { throw "torrent prive de banc introuvable : $TorrentFilePrivate" }
Log "=== Banc portable ADR-0018 : work=$WorkDir ==="

# ---------- A : bundle source ----------
$stateA = Join-Path $WorkDir 'A\state'
$proc = Start-Daemon $stateA 'A1'
$api = Wait-Daemon $proc.Id $stateA
Verdict ($null -ne $api) "A : API montee"
$ihPub = ""; $ihPriv = ""
if ($api) {
    # Ajout public (chemin du .torrent - le daemon resout le metainfo).
    $r = Invoke-RestMethod -Method Put -Uri "http://127.0.0.1:$($api.port)/api/downloads" `
        -Headers @{ "X-Api-Key" = $api.key } -ContentType 'application/json' `
        -Body (@{ torrent = $TorrentFile } | ConvertTo-Json) -TimeoutSec 15
    $ihPub = $r.infohash
    Verdict ($ihPub -ne "") "A : ajout public" $ihPub
    # Ajout prive - `area=private` : ligne opaque + manifeste OBM
    # (torrent distinct : l'infohash est la cle moteur).
    $r = Invoke-RestMethod -Method Put -Uri "http://127.0.0.1:$($api.port)/api/downloads" `
        -Headers @{ "X-Api-Key" = $api.key } -ContentType 'application/json' `
        -Body (@{ torrent = $TorrentFilePrivate; area = 'private' } | ConvertTo-Json) -TimeoutSec 15
    $ihPriv = $r.infohash
    Verdict ($ihPriv -ne "") "A : ajout prive" $ihPriv
    $priv = Invoke-RestMethod -Uri "http://127.0.0.1:$($api.port)/api/private" `
        -Headers @{ "X-Api-Key" = $api.key } -TimeoutSec 10
    Verdict ($priv.state -eq 'mounted' -and @($priv.downloads).Count -ge 1) `
        "A : /api/private monte" "state=$($priv.state) entries=$(@($priv.downloads).Count)"
    # Le groupe physique prive porte un nom opaque 32-hex, jamais
    # l'infohash en clair.
    $grpDir = Get-ChildItem -Path (Join-Path $WorkDir 'A\data\private\temp') -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^[0-9a-f]{32}$' }
    Verdict ($null -ne $grpDir) "A : groupe prive opaque" "$($grpDir.Name)"
    $leak = Get-ChildItem -Path (Join-Path $WorkDir 'A\data\private') -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like "*$ihPriv*" }
    Verdict ($null -eq $leak -or $leak.Count -eq 0) "A : aucun nom prive en clair"
    Stop-Daemon $proc $api
}

# ---------- B : copie du bundle -> autre chemin ----------
$srcBundle = Join-Path $WorkDir 'A'
$dstBundle = Join-Path $WorkDir 'B'
Copy-Item -Path $srcBundle -Destination $dstBundle -Recurse -Force

$hits = Scan-AbsPaths $dstBundle (Join-Path $WorkDir 'A')
Verdict ($hits.Count -eq 0) "B : zero chemin absolu persiste" ($(if ($hits) { ($hits | Select-Object -First 3) -join ' | ' } else { 'clean' }))

$stateB = Join-Path $WorkDir 'B\state'
$proc = Start-Daemon $stateB 'B1'
$api = Wait-Daemon $proc.Id $stateB
Verdict ($null -ne $api) "B : API montee sur la copie"
if ($api) {
    $dls = $null
    $deadline = (Get-Date).AddSeconds($ApiWaitSec)
    while ((Get-Date) -lt $deadline) {
        $dls = Invoke-RestMethod -Uri "http://127.0.0.1:$($api.port)/api/downloads" `
            -Headers @{ "X-Api-Key" = $api.key } -TimeoutSec 10
        if ($dls.checkpoints.all_loaded -or @($dls.downloads).Count -ge 2) { break }
        Start-Sleep -Milliseconds 400
    }
    $pub = @($dls.downloads | Where-Object { $_.infohash -eq $ihPub })
    Verdict ($pub.Count -eq 1 -and $pub[0].storage_area -eq 'public') "B : download public restitue"
    $pv = @($dls.downloads | Where-Object { $_.infohash -eq $ihPriv })
    Verdict ($pv.Count -eq 1 -and $pv[0].storage_area -eq 'private') `
        "B : download prive restitue" "$(if ($pv) { $pv[0].destination } else { 'absent' })"
    $priv = Invoke-RestMethod -Uri "http://127.0.0.1:$($api.port)/api/private" `
        -Headers @{ "X-Api-Key" = $api.key } -TimeoutSec 10
    Verdict ($priv.state -eq 'mounted' -and @($priv.downloads.infohash) -contains $ihPriv) `
        "B : manifeste prive relu apres copie" "state=$($priv.state)"
    Stop-Daemon $proc $api
}

# ---------- C : identite etrangere + data/private recopie ----------
$stateC = Join-Path $WorkDir 'C\state'
$proc = Start-Daemon $stateC 'C1'
$api = Wait-Daemon $proc.Id $stateC
Verdict ($null -ne $api) "C : API montee (identite neuve)"
if ($api) { Stop-Daemon $proc $api }
# Remplace la zone privee de C par celle de A - le manifeste OBM est
# indechiffrable sans la cle d'identite d'origine.
$privDst = Join-Path $WorkDir 'C\data\private'
if (Test-Path $privDst) { Remove-Item $privDst -Recurse -Force }
Copy-Item -Path (Join-Path $WorkDir 'A\data\private') -Destination $privDst -Recurse -Force

$proc = Start-Daemon $stateC 'C2'
$api = Wait-Daemon $proc.Id $stateC
Verdict ($null -ne $api) "C : API remontee avec data/private etranger"
if ($api) {
    $priv = Invoke-RestMethod -Uri "http://127.0.0.1:$($api.port)/api/private" `
        -Headers @{ "X-Api-Key" = $api.key } -TimeoutSec 10
    Verdict (@($priv.downloads).Count -eq 0) `
        "C : zone privee etrangere -> listing vide" "state=$($priv.state) entries=$(@($priv.downloads).Count)"
    Stop-Daemon $proc $api
}

Log ""
if ($script:fails -eq 0) {
    Log "BANC PORTABLE OK"
    if (-not $Keep) { Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue }
    exit 0
}
Log "BANC PORTABLE ECHEC ($($script:fails) verdict(s)) - artefacts conserves dans $WorkDir"
exit 1
