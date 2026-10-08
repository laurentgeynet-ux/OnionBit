# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# interop_ipv8.ps1 - echange reproductible Rust <-> pyipv8 (loopback).
#
# Jalon du roadmap : prouve que nos paquets signes sont decodes et
# verifies par le vrai pyipv8 (et reciproquement), avec les paquets
# enregistres dans target/interop/ puis rejoues par le test de replay.
#
# Usage : pwsh -NoProfile -ExecutionPolicy RemoteSigned -File scripts\interop_ipv8.ps1
# Prerequis : venv interop (scripts/interop/setup_venv.ps1 fait l'install).


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
# Chemins reseables via variables d'environnement (defauts : sources
# locales du projet).
$pyipv8 = if ($env:TRIBLER_PYIPV8) { $env:TRIBLER_PYIPV8 } else { "D:\Projet\Tribler_sources\tribler\pyipv8" }
$venvPy = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
$outDir = Join-Path $root "target\interop"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$pyPort = 11090
$rsPort = 11091
$pyLog = Join-Path $outDir "py_packets.log"
$rsLog = Join-Path $outDir "rust_packets.log"

Write-Host "== build interop_node (rust) =="
cargo build -p onionbit-ipv8 --example interop_node
if ($LASTEXITCODE -ne 0) { throw "build echoue" }

$env:PYTHONPATH = $pyipv8
Write-Host "== noeud Python sur 127.0.0.1:$pyPort, cible Rust $rsPort =="
$pyProc = Start-Process -FilePath $venvPy -PassThru -NoNewWindow `
    -ArgumentList "`"$PSScriptRoot\interop\py_node.py`" --port $pyPort --target 127.0.0.1:$rsPort --duration 9 --log `"$pyLog`"" `
    -RedirectStandardError (Join-Path $outDir "py_stderr.log")

Write-Host "== noeud Rust sur 127.0.0.1:$rsPort, cible Python $pyPort =="
$rsProc = Start-Process -FilePath ".\target\debug\examples\interop_node.exe" -PassThru -NoNewWindow `
    -ArgumentList "--port $rsPort --target 127.0.0.1:$pyPort --duration 9 --log `"$rsLog`"" `
    -RedirectStandardError (Join-Path $outDir "rust_stderr.log")

$rsProc.WaitForExit()
$pyProc.WaitForExit()

# Whitelist : ce banc n'echange que des messages DiscoveryCommunity —
# 1/2 similarity-request/response, 3/4 ping/pong, 246/245
# introduction-request/response ancien format, 250/249
# puncture-request/response. Tout autre msg_id est un echec.
$allow = "1,2,3,4,246,245,250,249"

Write-Host "== verification des paquets Rust par pyipv8 =="
& $venvPy "$PSScriptRoot\interop\verify_packets.py" $rsLog --allow-msg-id $allow
$rustOk = $LASTEXITCODE -eq 0

Write-Host "== verification des paquets Python par pyipv8 (sanity) =="
& $venvPy "$PSScriptRoot\interop\verify_packets.py" $pyLog --allow-msg-id $allow
$pyOk = $LASTEXITCODE -eq 0

# Echange reussi si chaque cote a vu l'autre (peers verifies dans les
# journaux stderr) et que toutes les signatures verifient.
$pyPeers = Select-String -Path (Join-Path $outDir "py_stderr.log") -Pattern "peers verifies : (\d+)" | ForEach-Object { $_.Matches.Groups[1].Value }
$rsPeers = Select-String -Path (Join-Path $outDir "rust_stderr.log") -Pattern "peers verifies : (\d+)" | ForEach-Object { $_.Matches.Groups[1].Value }
Write-Host "peers vus : py=$pyPeers rust=$rsPeers"

if ($rustOk -and $pyOk -and [int]$pyPeers -ge 1 -and [int]$rsPeers -ge 1) {
    Write-Host "INTEROP OK - paquets enregistres dans $outDir"
    exit 0
}
Write-Error "INTEROP ECHOUEE (rustOk=$rustOk pyOk=$pyOk pyPeers=$pyPeers rsPeers=$rsPeers)"
exit 1
