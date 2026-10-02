# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# check_i18n.ps1 — garde-fou anti-regression i18n.
#
# Signale tout litteral a consonance francaise subsistant dans
# app/lib/ : les chaines visibles par l'utilisateur doivent passer par
# les ARB (app_en.arb / app_fr.arb) et context.l10n, jamais en dur.
#
# Exceptions tolerees (allowlist ci-dessous) :
#   - commentaires et docstrings (code redige en francais, ADR-0005) ;
#   - fichiers generes gen_l10n (app_localizations*.dart) ;
#   - journaux developpeur (uiLog, debugPrint, log) ;
#   - autonyme 'Francais' (selecteur de langue) ;
#   - listes `keywords:` de recherche dans les reglages (non affichees
#     telles quelles — termes de correspondance bilingues).
#
# Usage : powershell -ExecutionPolicy Bypass -File scripts/check_i18n.ps1
# Code de sortie : 0 = propre, 1 = litteraux francais detectes.

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$libDir = Join-Path $root 'app\lib'

if (-not (Test-Path $libDir)) {
    Write-Error "Repertoire introuvable : $libDir"
}

# Plage Latin-1 supplementaire + ligatures (echappements u pour rester
# ASCII dans ce fichier) : couvre tous les accents francais, ou mots
# francais usuels d'UI sans accent (liste ciblee, faux positifs
# limites sur l'anglais). Signes mathematiques x (00D7) et
# division (00F7) exclus de la plage : notation numerique, pas du
# francais (ex. lanes de circuits dans diagnostic_page).
$frPattern = '[\u00C0-\u00D6\u00D8-\u00F6\u00F8-\u00FF\u0152\u0153\u0178]' +
    '|\b(Annuler|Fermer|Valider|Supprimer|Enregistrer|Appliquer|' +
    'Telechargement|Rechercher|Demarrer|Arreter|Ajouter|Importer|' +
    'Exporter|Ouvrir|Reglages|Parametres|Erreur|Aucun|Aucune|' +
    'Choisir|Confirmer|Retirer|Deplacer|Renommer|Inclure|Exclure|' +
    'Nouveau|Nouvelle|Charger|Ignorer)\b'

# Lignes tolerees : journaux dev, autonyme, mots-cles de recherche
# (reglages), imports.
$allowPattern = 'uiLog\(|debugPrint\(|''Fran.ais''|"Fran.ais"|' +
    'keywords\s*=|^\s*import\s|^\s*part\s'

$violations = @()

Get-ChildItem -Path $libDir -Recurse -Filter '*.dart' |
    Where-Object {
        $_.Name -notlike 'app_localizations*' -and
        $_.DirectoryName -notlike '*\l10n'
    } |
    ForEach-Object {
        $file = $_.FullName
        $rel = $file.Substring($root.Length + 1)
        $inBlockComment = $false
        $inKeywords = $false
        $lineNo = 0
        foreach ($line in [System.IO.File]::ReadLines($file)) {
            $lineNo++
            $trim = $line.Trim()
            # Sortie du bloc keywords: des que le parametre nomme
            # suivant apparait (les chaines sont concatenees sur
            # plusieurs lignes).
            if ($inKeywords -and
                $trim -match '^(sectionId|child|id|builder)\s*:') {
                $inKeywords = $false
            }
            if ($line -match 'keywords\s*:') {
                $inKeywords = $true
                continue
            }
            if ($inKeywords) { continue }
            if ($inBlockComment) {
                if ($trim.Contains('*/')) { $inBlockComment = $false }
                continue
            }
            if ($trim.StartsWith('//') -or $trim.StartsWith('*')) {
                continue
            }
            if ($trim.StartsWith('/*')) {
                if (-not $trim.Contains('*/')) { $inBlockComment = $true }
                continue
            }
            if ($line -match $allowPattern) { continue }
            # Ne garder que les litteraux '...' / "..." contenant du
            # francais.
            foreach ($m in [regex]::Matches($line,
                    '([''"])(?:\\.|(?!\1).)*\1')) {
                if ($m.Value -match $frPattern) {
                    $violations += "${rel}:${lineNo}: $($m.Value)"
                    break
                }
            }
        }
    }

if ($violations.Count -gt 0) {
    Write-Host "Litteraux francais detectes dans lib/ :" -ForegroundColor Red
    $violations | ForEach-Object { Write-Host "  $_" }
    Write-Host ""
    Write-Host ("Extraire ces chaines vers app_en.arb/app_fr.arb " +
        "(context.l10n.*) ou ajouter une exception justifiee.") -ForegroundColor Red
    exit 1
}

Write-Host "check_i18n : aucun litteral francais dans app/lib/" -ForegroundColor Green
exit 0
