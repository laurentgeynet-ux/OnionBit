# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# run_interop_suite.ps1 — harnais d'interop pyipv8 automatise :
# execute les bancs interop en sequence et imprime un recapitulatif
# PASS/FAIL/SKIP par script.
#
# Sans -Scripts, TOUS les bancs `scripts\interop_*.ps1` sont joues.
# Les prerequis de chaque banc sont detectes en lisant les variables
# d'environnement qu'il consulte (TRIBLER_INTEROP_PY, TRIBLER_PYIPV8,
# TRIBLER_SRC, TRIBLER_EXE) — un banc dont le prerequis manque est
# marque SKIP, pas FAIL.
#
# Usage :
#   pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\run_interop_suite.ps1
#   ... [-Scripts interop_ipv8,interop_dht] [-ListOnly]
#
# Prerequis resolus (variables d'environnement ou defauts) :
#   TRIBLER_INTEROP_PY -> D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe
#   TRIBLER_PYIPV8     -> D:\Projet\Tribler_sources\tribler\pyipv8
#   TRIBLER_SRC        -> D:\Projet\Tribler_sources\tribler\src
#   TRIBLER_EXE        -> requis par les bancs Tribler reels

param(
    # Liste separee par virgules ; vide = tous les interop_*.ps1.
    [string] $Scripts = "",
    # Affiche la suite resolue sans l'executer.
    [switch] $ListOnly
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

$root = Split-Path -Parent $PSScriptRoot

# Prerequis resolvables une fois (env var ou chemin par defaut).
$prereqs = @{
    TRIBLER_INTEROP_PY = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
    TRIBLER_PYIPV8     = if ($env:TRIBLER_PYIPV8)     { $env:TRIBLER_PYIPV8 }     else { "D:\Projet\Tribler_sources\tribler\pyipv8" }
    TRIBLER_SRC        = if ($env:TRIBLER_SRC)        { $env:TRIBLER_SRC }        else { "D:\Projet\Tribler_sources\tribler\src" }
    TRIBLER_EXE        = $env:TRIBLER_EXE   # pas de defaut : install Tribler
}

# Suite : liste explicite ou tous les scripts interop_*.ps1.
$suite = @()
if ($Scripts) {
    $suite = $Scripts -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
} else {
    $suite = Get-ChildItem (Join-Path $PSScriptRoot 'interop_*.ps1') |
        ForEach-Object { $_.BaseName }
}

if ($ListOnly) {
    $suite | ForEach-Object { Write-Host $_ }
    exit 0
}

Write-Host ("Suite : {0} banc(s)" -f $suite.Count)
foreach ($k in $prereqs.Keys) {
    $v = $prereqs[$k]
    $tag = if ($v -and (Test-Path $v)) { 'OK  ' } else { 'ABSENT' }
    Write-Host ("  {0} {1} = {2}" -f $tag, $k, $(if ($v) { $v } else { '(non defini)' }))
}
Write-Host ""

$results = @()
foreach ($s in $suite) {
    $file = Join-Path $PSScriptRoot "$s.ps1"
    if (-not (Test-Path $file)) {
        Write-Host "====== $s — fichier absent, SKIP" -ForegroundColor Yellow
        $results += [pscustomobject]@{ Banc = $s; Exit = 'SKIP'; Duree_s = 0 }
        continue
    }
    # Prerequis du banc : variables TRIBLER_* lues par le script.
    $body = Get-Content $file -Raw
    $missing = @()
    foreach ($k in $prereqs.Keys) {
        if ($body -match [regex]::Escape("`$$k") -or $body -match "env:$k") {
            if (-not $prereqs[$k] -or -not (Test-Path $prereqs[$k])) { $missing += $k }
        }
    }
    if ($missing) {
        Write-Host ("====== {0} — prerequis absents ({1}), SKIP" -f $s, ($missing -join ', ')) -ForegroundColor Yellow
        $results += [pscustomobject]@{ Banc = $s; Exit = 'SKIP'; Duree_s = 0 }
        continue
    }
    Write-Host "====== $s ======" -ForegroundColor Cyan
    $started = Get-Date
    & pwsh -NoProfile -ExecutionPolicy Bypass -File $file 2>&1 |
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
$fail = ($results | Where-Object { $_.Exit -ne 0 -and $_.Exit -ne 'SKIP' }).Count
$skip = ($results | Where-Object { $_.Exit -eq 'SKIP' }).Count
if ($fail -gt 0) { Write-Host "$fail FAIL, $skip SKIP"; exit 1 }
Write-Host "$skip SKIP"; exit 0
