# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# interop_discovery.ps1 - echange reproductible Rust <-> DiscoveryCommunity pyipv8.
#
# Jalon de l'etape 11 : introductions new-style (234 -> 233) et
# punctures (250/232 -> 249/231) dans les DEUX sens contre le vrai
# pyipv8, avec decode + verification de signature des deux cotes
# (tout handler pyipv8 passe par lazy_wrapper : decode + signature).
#
# Preuves attendues (marqueurs) :
#   Rust   : RUST_SEES_PY (new_style=true), RUST_NEW_INTRO_OK,
#            RUST_NEW_PUNCTURE_OK, RUST_OLD_PUNCTURE_OK
#   Python : PY_NEW_INTRO_RESP_OK (234->233 decode+signe),
#            PY_NEW_PUNCTURE_OK (232->231), PY_OLD_PUNCTURE_OK (250->249),
#            PY_RECV_NEW_INTRO_REQ, PY_RECV_NEW_PUNCTURE_REQ,
#            PY_RECV_OLD_PUNCTURE_REQ (requetes Rust decodees+verifiees)
#
# Usage : pwsh -NoProfile -ExecutionPolicy RemoteSigned -File scripts\interop_discovery.ps1
# Prerequis : venv interop (voir scripts/interop_ipv8.ps1).


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
$pyipv8 = if ($env:TRIBLER_PYIPV8) { $env:TRIBLER_PYIPV8 } else { "D:\Projet\Tribler_sources\tribler\pyipv8" }
$venvPy = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
$outDir = Join-Path $root "target\interop-discovery"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$pyPort = 12110      # endpoint discovery pyipv8
$rsPort = 12111      # endpoint discovery Rust
$pyLog = Join-Path $outDir "py_packets.log"
$pyErr = Join-Path $outDir "py_stderr.log"
$rsLog = Join-Path $outDir "rust_stderr.log"

Remove-Item -Force -ErrorAction SilentlyContinue $pyLog, $pyErr, $rsLog

Write-Host "== build discovery_interop_node (rust) =="
cargo build -p onionbit-ipv8 --example discovery_interop_node
if ($LASTEXITCODE -ne 0) { throw "build echoue" }

$env:PYTHONPATH = $pyipv8
Write-Host "== noeud Python discovery sur 127.0.0.1:$pyPort, cible Rust $rsPort =="
$pyProc = Start-Process -FilePath $venvPy -PassThru -NoNewWindow `
    -ArgumentList "`"$PSScriptRoot\interop\py_discovery_node.py`" --port $pyPort --target 127.0.0.1:$rsPort --duration 14 --log `"$pyLog`"" `
    -RedirectStandardError $pyErr

Write-Host "== noeud Rust discovery sur 127.0.0.1:$rsPort, cible Python $pyPort =="
$rsProc = Start-Process -FilePath ".\target\debug\examples\discovery_interop_node.exe" -PassThru -NoNewWindow `
    -ArgumentList "--port $rsPort --py-addr 127.0.0.1:$pyPort --duration 14" `
    -RedirectStandardError $rsLog

$rsProc.WaitForExit()
$pyProc.WaitForExit()

$py = Get-Content $pyErr -Raw -ErrorAction SilentlyContinue
$rs = Get-Content $rsLog -Raw -ErrorAction SilentlyContinue

$expected = @{
    "rust:RUST_SEES_PY"            = $rs -match "RUST_SEES_PY\|new_style=true"
    "rust:RUST_NEW_INTRO_OK"       = $rs -match "RUST_NEW_INTRO_OK"
    "rust:RUST_NEW_PUNCTURE_OK"    = $rs -match "RUST_NEW_PUNCTURE_OK"
    "rust:RUST_OLD_PUNCTURE_OK"    = $rs -match "RUST_OLD_PUNCTURE_OK"
    "py:PY_NEW_INTRO_RESP_OK"      = $py -match "PY_NEW_INTRO_RESP_OK : OK"
    "py:PY_NEW_PUNCTURE_OK"        = $py -match "PY_NEW_PUNCTURE_OK : OK"
    "py:PY_OLD_PUNCTURE_OK"        = $py -match "PY_OLD_PUNCTURE_OK : OK"
    "py:PY_RECV_NEW_INTRO_REQ"     = $py -match "PY_RECV_NEW_INTRO_REQ : OK"
    "py:PY_RECV_NEW_PUNCTURE_REQ"  = $py -match "PY_RECV_NEW_PUNCTURE_REQ : OK"
    "py:PY_RECV_OLD_PUNCTURE_REQ"  = $py -match "PY_RECV_OLD_PUNCTURE_REQ : OK"
}
$allOk = $true
foreach ($k in $expected.Keys | Sort-Object) {
    $ok = $expected[$k]
    if (-not $ok) { $allOk = $false }
    Write-Host ("{0} : {1}" -f $k, $(if ($ok) { "OK" } else { "ABSENT" }))
}

if ($allOk) {
    Write-Host "INTEROP DISCOVERY OK"
    exit 0
} else {
    Write-Host "INTEROP DISCOVERY ECHEC - voir target\interop-discovery\*.log"
    exit 1
}
