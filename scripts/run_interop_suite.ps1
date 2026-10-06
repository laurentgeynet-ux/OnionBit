# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# run_interop_suite.ps1 — execute une liste de bancs interop en
# sequence et imprime un recapitulatif PASS/FAIL par script.
#
# Usage :
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\run_interop_suite.ps1 -Scripts interop_ipv8,interop_dht

param(
    # Liste separee par virgules (PS5.1 ne splitte pas [string[]]
    # depuis la ligne de commande).
    [Parameter(Mandatory)] [string] $Scripts
)

$root = Split-Path -Parent $PSScriptRoot
$results = @()
foreach ($s in ($Scripts -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })) {
    Write-Host "====== $s ======" -ForegroundColor Cyan
    $started = Get-Date
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "$s.ps1") 2>&1 |
        Select-Object -Last 30
    $code = $LASTEXITCODE
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    $results += [pscustomobject]@{
        Banc    = $s
        Exit    = $code
        Duree_s = $elapsed
    }
    $color = if ($code -eq 0) { 'Green' } else { 'Red' }
    Write-Host ("====== {0} exit={1} ({2} s) ======" -f $s, $code, $elapsed) -ForegroundColor $color
}
Write-Host "`n===== RECAP ====="
$results | Format-Table -AutoSize
$fail = ($results | Where-Object { $_.Exit -ne 0 }).Count
if ($fail -gt 0) { exit 1 }
exit 0
