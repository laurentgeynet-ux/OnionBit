# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# tor_search_bench.ps1 - banc ADR-0028 etape 118 : pipeline Tor reelle
# sur un daemon compile `--features tor` (pas de mock).
#
#   - pre-seed un configuration.json avec la lane Tor + un backend
#     de recherche onion (miroir configurable, TPB par defaut)
#   - lance onionbit-daemon en mode normal
#   - bootstrap : poll /api/tor/status jusqu'a ready|error (borne)
#   - recherche : /api/search/external?source=...&q=... -> 200 ou
#     erreur typee documentee, jamais de repli clearnet
#   - refus politique : source inconnue -> 400 unknown_source
#   - passerelle EXIT_TOR : phase 2 non implementee -> verifie que
#     tor.gateway.enabled reste false dans les settings
#
# Invariants verifies : aucune fuite clearnet (le seul surface de
# fetch externe est la recherche allowlistee), etat toujours
# observable pendant le bootstrap, erreurs typees.
#
# Usage : pwsh -NoProfile -ExecutionPolicy RemoteSigned `
#   -File scripts\tor_search_bench.ps1 [-Port 8320] `
#   [-BootstrapTimeoutSec 420] [-Mirror <hote>.onion]
#
# NOTE encodage : fichier volontairement ASCII -- un caractere
# multi-octets lu en CP1252 par Windows PowerShell 5.1 peut casser le
# parsing (cf. fuzz_campaign.ps1).

param(
    [string]$Daemon = "",
    [int]$Port = 8320,
    [string]$OutDir = "",
    [string]$Mirror = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion",
    [string]$UrlTemplate = "http://{host}/q.php?q={q}",
    [int]$BootstrapTimeoutSec = 420
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

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if ($Daemon -eq "") { $Daemon = Join-Path $root 'target\debug\onionbit-daemon.exe' }
if ($OutDir -eq "") {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutDir = Join-Path $root "target\tor-bench-$stamp"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$stateDir = Join-Path $OutDir 'state'
New-Item -ItemType Directory -Force -Path $stateDir | Out-Null

# ---------- Pre-seed : lane Tor + backend onion ----------
# Le daemon fusionne ce fichier partiel avec ses defauts et genere
# api.key au premier run.
$seed = @{
    tor = @{
        enabled = $true
        mode = "embedded"
        bootstrap_timeout_ms = ($BootstrapTimeoutSec * 1000)
        allowed_hosts = @($Mirror)
        search = @{
            backends = @{
                tpb = @{
                    mirrors = @($Mirror)
                    url_template = $UrlTemplate
                    max_results = 50
                }
            }
        }
    }
}
$seed | ConvertTo-Json -Depth 6 | Set-Content -Path (Join-Path $stateDir 'configuration.json') -Encoding UTF8

$script:fails = 0
function Verdict([bool]$ok, [string]$name, [string]$detail = "") {
    if ($ok) { Write-Host ("OK   {0} {1}" -f $name, $detail) }
    else     { Write-Host ("FAIL {0} {1}" -f $name, $detail); $script:fails++ }
}

# Renvoie @{ code; body } pour n'importe quel statut (les erreurs 4xx
# sont des reponses attendues ici, pas des exceptions fatales).
function Probe([string]$method, [string]$path, $key, $body = $null) {
    $uri = "http://127.0.0.1:$Port/api$path"
    $h = @{}
    if ($key) { $h['X-Api-Key'] = $key }
    try {
        $p = @{ Method = $method; Uri = $uri; Headers = $h; TimeoutSec = 30 }
        if ($null -ne $body) {
            $p['ContentType'] = 'application/json'
            $p['Body'] = ($body | ConvertTo-Json -Compress)
        }
        $r = Invoke-WebRequest @p
        return @{ code = [int]$r.StatusCode; body = $r.Content }
    } catch {
        if ($_.ErrorDetails -and $_.ErrorDetails.Message) {
            $resp = $_.Exception.Response
            $code = if ($resp) { [int]$resp.StatusCode } else { -1 }
            return @{ code = $code; body = $_.ErrorDetails.Message }
        }
        $resp = $_.Exception.Response
        if ($resp) {
            $code = [int]$resp.StatusCode
            if ($resp.PSObject.Methods['GetResponseStream']) {
                $sr = New-Object IO.StreamReader($resp.GetResponseStream())
                return @{ code = $code; body = $sr.ReadToEnd() }
            }
            return @{ code = $code; body = "" }
        }
        return @{ code = -1; body = $_.Exception.Message }
    }
}

function Wait-ApiKey([string]$dir, [int]$sec = 60) {
    $dl = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $dl) {
        $cfg = Join-Path $dir 'configuration.json'
        if (Test-Path $cfg) {
            try {
                $k = (Get-Content $cfg -Raw | ConvertFrom-Json).api.key
                if ($k) { return $k }
            } catch {}
        }
        Start-Sleep -Milliseconds 500
    }
    throw "cle API absente dans $dir"
}

$proc = $null
try {
    # Pas de redirection stdout/stderr : un pipe non draine se
    # remplit (~4 Ko) et gele le daemon avant le bind de l'API.
    $psi = New-Object System.Diagnostics.ProcessStartInfo($Daemon)
    $psi.Arguments = ('--state-dir "{0}" --listen 127.0.0.1:{1} --no-tray' -f $stateDir, $Port)
    $psi.UseShellExecute = $false
    $proc = [System.Diagnostics.Process]::Start($psi)

    $key = Wait-ApiKey $stateDir
    $dl = (Get-Date).AddSeconds(60)
    $up = $false
    while ((Get-Date) -lt $dl -and -not $up) {
        $r = Probe 'GET' '/statistics/tribler' $key
        if ($r.code -eq 200) { $up = $true } else { Start-Sleep -Milliseconds 700 }
    }
    Verdict $up 'daemon demarre, API en ligne' "port=$Port"
    if (-not $up) { throw "API injoignable" }

    # ---------- 1. Lane Tor : status observable puis bootstrap ----------
    $st = Probe 'GET' '/tor/status' $key
    Verdict ($st.code -eq 200) 'GET /tor/status repond' "code=$($st.code)"
    if ($st.code -eq 200) {
        $s = $st.body | ConvertFrom-Json
        Verdict ($s.enabled -eq $true) 'tor.enabled=true relu' "state=$($s.state)"
    }

    # Poll borne : ready | error | dormant acceptes, timeout = echec.
    $deadline = (Get-Date).AddSeconds($BootstrapTimeoutSec + 60)
    $final = ""
    while ((Get-Date) -lt $deadline) {
        $st = Probe 'GET' '/tor/status' $key
        if ($st.code -eq 200) {
            $s = $st.body | ConvertFrom-Json
            $final = "$($s.state)"
            if ($final -in @('ready', 'error', 'dormant')) { break }
        }
        Start-Sleep -Seconds 3
    }
    Verdict ($final -eq 'ready') 'bootstrap Tor -> ready' "state=$final"
    $torReady = ($final -eq 'ready')

    # ---------- 2. Recherche externe ----------
    if ($torReady) {
        $r = Probe 'GET' '/search/external?source=tpb&q=test' $key
        $ok200 = ($r.code -eq 200)
        $typedErr = ($r.code -in @(502, 503) -and $r.body -match '"(mirror_unreachable|captcha_detected|structure_changed|no_results|bootstrap_failed|tor_not_ready|request_failed)"')
        Verdict ($ok200 -or $typedErr) 'recherche externe : 200 ou erreur typee' "code=$($r.code)"
        if ($ok200) {
            $j = $r.body | ConvertFrom-Json
            Verdict ($null -ne $j.results -and $j.source -eq 'tpb') 'reponse : {uuid, source, results}' "n=$($j.results.Count)"
        }
    } else {
        # Lane non prete : la recherche doit rendre une 503 typee,
        # jamais un fetch direct.
        $r = Probe 'GET' '/search/external?source=tpb&q=test' $key
        Verdict ($r.code -eq 503 -and $r.body -match '"(tor_bootstrapping|bootstrap_failed|tor_not_ready)"') `
            'lane non prete -> 503 typee (pas de repli)' "code=$($r.code)"
    }

    # ---------- 3. Refus politique ----------
    $r = Probe 'GET' '/search/external?source=inconnu&q=test' $key
    Verdict ($r.code -eq 400 -and $r.body -match 'unknown_source') 'source inconnue -> 400' "code=$($r.code)"
    $r = Probe 'GET' '/tor/probe?source=inconnu' $key
    Verdict ($r.code -eq 400) 'sonde source inconnue -> 400' "code=$($r.code)"

    # ---------- 4. Passerelle EXIT_TOR (phase 2 non implementee) ----------
    $r = Probe 'GET' '/settings' $key
    if ($r.code -eq 200) {
        $gw = ($r.body | ConvertFrom-Json).tor.gateway.enabled
        Verdict ($gw -ne $true) 'passerelle EXIT_TOR absente (phase 2)' "gateway.enabled=$gw"
    } else {
        Verdict $false 'GET /settings' "code=$($r.code)"
    }

    Write-Host ""
    if ($script:fails -eq 0) { Write-Host "TOR SEARCH BENCH OK"; exit 0 }
    Write-Host "TOR SEARCH BENCH ECHEC ($($script:fails) verdict(s))"
    exit 1
}
finally {
    if ($proc -and -not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}
