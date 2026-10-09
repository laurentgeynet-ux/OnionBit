# This file is part of OnionBit.
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
#   - listes `keywords:`/map `*Keywords = {}` de recherche dans les
#     reglages et le catalogue de commandes (non affichees telles
#     quelles — termes de correspondance bilingues, ADR-0021 §6) ;
#   - le guide de style `/_style-guide` (ADR-0021 §7) : page de
#     specimen reservee aux developpeurs, montee uniquement en debug
#     (kDebugMode) ; ses chaines accentuees testent deliberement la
#     couverture de glyphes des polices auto-hebergees.
#
# Usage : pwsh -ExecutionPolicy Bypass -File scripts/check_i18n.ps1
# Code de sortie : 0 = propre, 1 = litteraux francais detectes.


# Encodage : sous Windows PowerShell 5.1, forcer UTF-8 (console +
# lectures Get-Content ; les ecritures gardent leur -Encoding explicite
# ou le defaut de l'hote). pwsh 7 est deja UTF-8 : bloc sans effet.
if ($PSVersionTable.PSVersion.Major -lt 7) {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $OutputEncoding = [System.Text.Encoding]::UTF8
    if ($null -eq $PSDefaultParameterValues) { $PSDefaultParameterValues = @{} }
    $PSDefaultParameterValues['Get-Content:Encoding'] = 'UTF8'
}

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
        $_.DirectoryName -notlike '*\l10n' -and
        $_.DirectoryName -notlike '*\style_guide'
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
            # plusieurs lignes) ou que la map `*Keywords = {…}` se
            # referme (settings_catalog.dart — ADR-0021 §6).
            if ($inKeywords -and
                ($trim -match '^(sectionId|child|id|builder)\s*:' -or
                 $trim -match '^\};?$')) {
                $inKeywords = $false
            }
            if ($line -match 'keywords\s*:') {
                $inKeywords = $true
                continue
            }
            if ($line -match 'Keywords\s*=\s*(const\s*)?\{') {
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
