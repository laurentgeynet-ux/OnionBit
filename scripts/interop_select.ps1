# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# interop_select.ps1 - banc remote-select/health/version Rust <->
# vraie ContentDiscoveryCommunity Tribler (MetadataStore pony en
# memoire) — etape 95.
#
# Directions eprouvees :
#   Python -> Rust : send_remote_select (reponses parsees .mdblob
#     + LZ4 cote Rust), VersionRequest, HealthRequest.
#   Rust -> Python : select servi par le provider Rust (chunks
#     signes mdblob+LZ4 ingeres par process_compressed_mdblob),
#     VersionRequest servi, HealthRequest servi.
#
# Preuves attendues (marqueurs stderr) :
#   Rust   : RUST_PEER_OK, RUST_SELECT_RESP_OK, RUST_SERVED_SELECT,
#            RUST_HEALTH_REQ_SERVED, RUST_HEALTH_RESP
#   Python : PY_PEER_OK, PY_SELECT_RESP_OK, PY_RECV_SELECT_REQ,
#            PY_VERSION_RESP, PY_RECV_VERSION_REQ, PY_RECV_HEALTH_REQ
#
# Usage : pwsh -NoProfile -ExecutionPolicy RemoteSigned -File scripts\interop_select.ps1
# Prerequis : venv interop (TRIBLER_INTEROP_PY) avec pony+lz4,
#             TRIBLER_SRC (tribler/src) et TRIBLER_PYIPV8.


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
$triblerSrc = if ($env:TRIBLER_SRC) { $env:TRIBLER_SRC } else { "D:\Projet\Tribler_sources\tribler\src" }
$venvPy = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
$outDir = Join-Path $root "target\interop-select"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$pyPort = 12120      # endpoint content-discovery pyipv8
$rsPort = 12121      # endpoint content-discovery Rust
$pyLog = Join-Path $outDir "py_packets.log"
$pyErr = Join-Path $outDir "py_stderr.log"
$rsLog = Join-Path $outDir "rust_stderr.log"

Remove-Item -Force -ErrorAction SilentlyContinue $pyLog, $pyErr, $rsLog

Write-Host "== build select_interop_node (rust) =="
cargo build -p onionbit-ipv8 --example select_interop_node
if ($LASTEXITCODE -ne 0) { throw "build echoue" }

$env:PYTHONPATH = "$triblerSrc;$pyipv8"
Write-Host "== noeud Python content-discovery sur 127.0.0.1:$pyPort, cible Rust $rsPort =="
$pyProc = Start-Process -FilePath $venvPy -PassThru -NoNewWindow `
    -ArgumentList "`"$PSScriptRoot\interop\py_select_node.py`" --port $pyPort --target 127.0.0.1:$rsPort --duration 12 --log `"$pyLog`"" `
    -RedirectStandardError $pyErr

Write-Host "== noeud Rust content-discovery sur 127.0.0.1:$rsPort, cible Python $pyPort =="
$rsProc = Start-Process -FilePath ".\target\debug\examples\select_interop_node.exe" -PassThru -NoNewWindow `
    -ArgumentList "--port $rsPort --target 127.0.0.1:$pyPort --duration 12" `
    -RedirectStandardError $rsLog

Wait-Process -Id $pyProc.Id -Timeout 40 -ErrorAction SilentlyContinue | Out-Null
Wait-Process -Id $rsProc.Id -Timeout 40 -ErrorAction SilentlyContinue | Out-Null
if (-not $pyProc.HasExited) { Stop-Process -Id $pyProc.Id -Force }
if (-not $rsProc.HasExited) { Stop-Process -Id $rsProc.Id -Force }

Write-Host "`n===== stderr Python ====="
Get-Content $pyErr -ErrorAction SilentlyContinue | Select-Object -Last 40
Write-Host "`n===== stderr Rust ====="
Get-Content $rsLog -ErrorAction SilentlyContinue | Select-Object -Last 40

$py = (Get-Content $pyErr -ErrorAction SilentlyContinue) -join "`n"
$rs = (Get-Content $rsLog -ErrorAction SilentlyContinue) -join "`n"
$checks = @(
    @{ name = "PY_PEER_OK";            hay = $py },
    @{ name = "PY_SELECT_RESP_OK";     hay = $py },
    @{ name = "PY_RECV_SELECT_REQ";    hay = $py },
    @{ name = "PY_VERSION_RESP";       hay = $py },
    @{ name = "PY_RECV_VERSION_REQ";   hay = $py },
    @{ name = "PY_RECV_HEALTH_REQ";    hay = $py },
    @{ name = "RUST_PEER_OK";          hay = $rs },
    @{ name = "RUST_SELECT_RESP_OK";   hay = $rs },
    @{ name = "RUST_SERVED_SELECT";    hay = $rs },
    @{ name = "RUST_HEALTH_REQ_SERVED";hay = $rs },
    @{ name = "RUST_HEALTH_RESP";      hay = $rs }
)
$fail = 0
foreach ($c in $checks) {
    if ($c.hay -match $c.name) {
        Write-Host "PASS $($c.name)"
    } else {
        Write-Host "FAIL $($c.name)"
        $fail++
    }
}
if ($fail -gt 0) { throw "$fail marqueurs manquants" }
Write-Host "`ninter select OK"
