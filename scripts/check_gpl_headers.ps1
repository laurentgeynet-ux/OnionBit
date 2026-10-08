# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# check_gpl_headers.ps1 — oracle ADR-0020 : aucune notice GPL ne doit
# porter l'ancien descripteur « a Rust port of the Tribler daemon », et
# toute notice « part of OnionBit » doit etre de la forme nouvelle
# (« This file is part of OnionBit. »).
#
# Le motif est ancre sur la forme de commentaire (//, #, <!--, /*) en
# debut de ligne : une citation en prose (ADR, docs) ne declenche pas
# le gate. Balayage du depot entier hors exclusions (vendor/, target/,
# app/build/, .dart_tool/, .git/...).
#
# Usage : pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\check_gpl_headers.ps1
# Sortie : 0 si conforme, 1 sinon (liste les porteurs restants).

if ($PSVersionTable.PSVersion.Major -lt 7) {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $OutputEncoding = [System.Text.Encoding]::UTF8
    if ($null -eq $PSDefaultParameterValues) { $PSDefaultParameterValues = @{} }
    $PSDefaultParameterValues['Get-Content:Encoding'] = 'UTF8'
}

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

$markOld = '^\s*(//|#|<!--|/\*)\s*This file is part of OnionBit\s*-\s*a Rust port'
$markAny = '^\s*(//|#|<!--|/\*)\s*This file is part of OnionBit'
$exclTop = @('.git', 'vendor', 'target', '.idea', '.vs')
$exclAny = @('.dart_tool', 'node_modules', '__pycache__')

$bad = New-Object System.Collections.Generic.List[string]
Get-ChildItem -Path $root -Recurse -File -Force | ForEach-Object {
    $rel = $_.FullName.Substring($root.Length + 1)
    $seg = $rel -split '[\\/]'
    if ($exclTop -contains $seg[0]) { return }
    foreach ($x in $exclAny) { if ($seg -contains $x) { return } }
    if ($seg.Count -ge 2 -and $seg[0] -eq 'app' -and $seg[1] -eq 'build') { return }
    try {
        $i = 0
        foreach ($line in [System.IO.File]::ReadLines($_.FullName)) {
            $i++
            if ($line -match $markOld) {
                $bad.Add("${rel}:${i}: ancien descripteur") | Out-Null
            } elseif ($line -match $markAny -and $line -notmatch 'part of OnionBit\.') {
                $bad.Add("${rel}:${i}: forme inattendue") | Out-Null
            }
        }
    } catch { }
}

if ($bad.Count -gt 0) {
    $bad | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    exit 1
}
Write-Host "Notices GPL conformes (ADR-0020)." -ForegroundColor Green
exit 0
