# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

param(
    [int]$Payload = 200000,
    [int]$Hops = 3,
    [int]$ExitPort = 12100,
    [string]$OutDir = "target\interop-py-relay"
)

# Experience : pyipv8 en premier relais + relais Rust + sortie pyipv8.
# Topologie : Rust -> py(relais+sortie, port $ExitPort) -> Rust relay -> py(exit).

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Push-Location $repo
try {
    $pyipv8 = if ($env:TRIBLER_PYIPV8) { $env:TRIBLER_PYIPV8 } else { "D:\Projet\Tribler_sources\tribler\pyipv8" }
    $venvPy = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
    $out = Join-Path $repo $OutDir
    New-Item -ItemType Directory -Force -Path $out | Out-Null
    $pyKeyFile = Join-Path $out "py_key.txt"
    $pyLog = Join-Path $out "py_packets.log"
    Remove-Item -Force -ErrorAction SilentlyContinue $pyKeyFile

    $env:PYTHONPATH = $pyipv8
    $pyProc = Start-Process -FilePath $venvPy -PassThru -NoNewWindow `
        -ArgumentList "`"$repo\scripts\interop\py_tunnel_node.py`" --port $ExitPort --keyfile `"$pyKeyFile`" --log `"$pyLog`" --duration 120 --community-id a3591a6bd89bbaca0974062a1287afcfbc6fd6bc" `
        -RedirectStandardError (Join-Path $out "py_stderr.log")

    $deadline = (Get-Date).AddSeconds(10)
    while (-not (Test-Path $pyKeyFile) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if (-not (Test-Path $pyKeyFile)) { Write-Host "ECHEC : cle pyipv8 absente"; exit 1 }
    Write-Host "pyipv8 node up sur 127.0.0.1:$ExitPort"

    $rustStderr = Join-Path $out "rust_stderr.log"
    $proc = Start-Process -FilePath "cmd.exe" -PassThru -NoNewWindow -Wait `
        -ArgumentList "/c `"$repo\target\debug\examples\exit_download_interop.exe --keyfile `"$pyKeyFile`" --relay `"$pyKeyFile`" --tribler-id --hops $Hops --payload $Payload 2> `"$rustStderr`" & exit !ERRORLEVEL!" `
        -WorkingDirectory $repo
    Write-Host "rust exit code = $($proc.ExitCode)"
    Stop-Process -Id $pyProc.Id -Force -ErrorAction SilentlyContinue

    if ($proc.ExitCode -ne 0) {
        Write-Host "INTEROP PY RELAY ECHEC"
        if (Test-Path $rustStderr) {
            Select-String -Path $rustStderr -Pattern 'READY|ECHEC|timeout|extend' | Select-Object -Last 10 | ForEach-Object { $_.Line }
        }
        exit $proc.ExitCode
    }
    if (Test-Path $rustStderr) {
        Select-String -Path $rustStderr -Pattern 'READY|INTEROP|ECHEC' | Select-Object -Last 5 | ForEach-Object { $_.Line }
    }
    Write-Host "INTEROP PY RELAY OK"
}
finally {
    Pop-Location
    Get-Process -Name "exit_download_interop" -ErrorAction SilentlyContinue | Stop-Process -Force
}
