# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# sync_public.ps1 - synchronise le repo public (GitHub) depuis le repo dev.
#
# Methode deterministe : `git archive HEAD` n'exporte QUE les fichiers trackes
# du commit - impossible de toucher .git, target/, les corpus fuzz, etc.
# NE PAS utiliser robocopy /MIR : ses exclusions /XD /XF sont fragiles et /MIR
# detruit les fichiers "en trop" (incident du 2026-10-02 : pack .git supprimes).
#
# Usage : pwsh scripts/sync_public.ps1 [-Commit]
#   Sans -Commit : synchronise puis affiche `git status` pour revue manuelle.
#   Avec -Commit : commit + push automatiques (message passe en -Message).

param(
    [string]$DevRepo   = 'D:\Projet\Tribler-Rust-Torrent',
    [string]$PublicRepo = 'D:\Projet\OnionBit',
    [switch]$Commit,
    [string]$Message = 'sync from dev repo'
)

$ErrorActionPreference = 'Stop'

# Fichiers trackes dans les DEUX repos mais volontairement divergents
# (le depot public garde sa propre version, ex. README/AGENTS en anglais).
$PublicDivergent = @('README.md', 'AGENTS.md', '.gitignore')

# Motifs des fichiers public-only (absents du dev : ne JAMAIS supprimer).
$PublicOnlyPatterns = @(
    '^\.gitattributes$',
    '^\.github/',
    '^CODE_OF_CONDUCT\.md$',
    '^CONTRIBUTING\.md$',
    '^SECURITY\.md$',
    '^assets/',
    '^promo/',
    '^docs/ARCHITECTURE\.md$',
    '^docs/BUILDING\.md$',
    '^docs/THREAT-MODEL\.md$',
    '^docs/interop/README\.md$',
    '^scripts/gh_create_release\.ps1$',
    '^vendor/README\.md$'
)

function Test-PublicOnly([string]$path) {
    foreach ($p in $PublicOnlyPatterns) { if ($path -match $p) { return $true } }
    return $false
}

# --- verifications ---------------------------------------------------
foreach ($r in @($DevRepo, $PublicRepo)) {
    if (-not (Test-Path (Join-Path $r '.git'))) { throw "pas un repo git : $r" }
}
if (git -C $DevRepo status --porcelain) {
    Write-Warning "repo dev non propre - l'archive portera sur HEAD (pas le working tree)"
}

# --- 1. export des fichiers trackes du dev ---------------------------
# format zip + Expand-Archive : natif Windows, aucun piege de parsing de
# chemin (tar/bsdtar interprete 'D:\...' comme une syntaxe host:fichier).
$zip = Join-Path $env:TEMP ("onionbit_sync_" + [guid]::NewGuid().ToString('N') + '.zip')
try {
    git -C $DevRepo archive HEAD -o $zip --format=zip
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $zip)) { throw 'git archive a echoue' }

    # --- 2. extraction par-dessus le working tree public -------------
    # -Force ecrase les fichiers communs, ajoute les nouveaux, ne supprime rien.
    Expand-Archive -Path $zip -DestinationPath $PublicRepo -Force
} finally {
    Remove-Item $zip -ErrorAction SilentlyContinue
}

# --- 3. restaure les fichiers divergents cote public -----------------
foreach ($f in $PublicDivergent) {
    git -C $PublicRepo checkout -- $f 2>$null
}

# --- 4. supprime les fichiers retires du dev (sauf public-only) ------
$devFiles = @{}
git -C $DevRepo ls-files | ForEach-Object { $devFiles[$_] = $true }
$removed = @()
foreach ($f in (git -C $PublicRepo ls-files)) {
    if (-not $devFiles.ContainsKey($f) -and -not (Test-PublicOnly $f)) {
        $removed += $f
        Remove-Item (Join-Path $PublicRepo $f) -Force -ErrorAction SilentlyContinue
    }
}
if ($removed) { Write-Output "supprimes (absents du dev):`n  $($removed -join "`n  ")" }

# --- 5. rapport / commit ---------------------------------------------
git -C $PublicRepo add -A
$status = git -C $PublicRepo status --short
if (-not $status) { Write-Output 'deja synchronise - rien a faire'; exit 0 }

Write-Output "`n=== changements a publier ==="
$status
Write-Output "============================`n"

if ($Commit) {
    git -C $PublicRepo commit -q -m $Message
    git -C $PublicRepo push origin HEAD
    Write-Output "commite + pousse : $Message"
} else {
    Write-Output 'revue manuelle : verifiez puis commitez/poussez dans le repo public.'
    exit 1  # code non-zero = changements en attente, a revoir
}
