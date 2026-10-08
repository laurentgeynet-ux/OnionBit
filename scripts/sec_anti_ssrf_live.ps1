# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# sec_anti_ssrf_live.ps1 - banc P0-17a : auth API + anti-SSRF live sur
# un daemon REEL (pas de mock, pas de test unitaire).
#
#   - lance onionbit-daemon en mode normal (politique IpPolicy STRICTE)
#   - verifie que toute requete sans cle ou avec une mauvaise cle -> 401
#   - verifie que PUT /api/downloads refuse fermement les URI http(s)
#     dont l'hote resout vers loopback / prive / link-local / unspecified
#
# PRECONDITION CRITIQUE : la politique effective doit etre STRICTE.
# `--offline` selectionne IpPolicy::permissive() (config.rs) : un tel
# daemon laisserait la politique accepter les adresses privees et le
# rejet surviendrait plus tard dans le moteur (faux negatif anti-SSRF).
# Le script verifie donc explicitement la politique effective : la
# sonde temoin 127.0.0.1 doit produire "politique reseau: destination
# refusee", pas "moteur bittorrent" ; sinon le run est invalide.
#
# Usage : pwsh -NoProfile -ExecutionPolicy RemoteSigned `
#   -File scripts\sec_anti_ssrf_live.ps1 [-Port 8310]
#
# NOTE encodage : fichier volontairement ASCII -- un caractere
# multi-octets lu en CP1252 par Windows PowerShell 5.1 peut casser le
# parsing (cf. fuzz_campaign.ps1).

param(
    [string]$Daemon = "",
    [int]$Port = 8310,
    [string]$OutDir = ""
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
    $OutDir = Join-Path $root "target\sec-ssrf-$stamp"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$stateDir = Join-Path $OutDir 'state'
New-Item -ItemType Directory -Force -Path $stateDir | Out-Null

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
        $p = @{ Method = $method; Uri = $uri; Headers = $h; TimeoutSec = 15 }
        if ($null -ne $body) {
            $p['ContentType'] = 'application/json'
            $p['Body'] = ($body | ConvertTo-Json -Compress)
        }
        $r = Invoke-WebRequest @p
        return @{ code = [int]$r.StatusCode; body = $r.Content }
    } catch {
        # pwsh 7 expose le corps dans ErrorDetails ; PS 5.1 l'exige via
        # le stream de la HttpWebResponse.
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
    # ---------- Daemon reel, politique stricte ----------
    # PAS de --offline : ce mode bascule en IpPolicy::permissive() et
    # rendrait le banc anti-SSRF inerte.
    $psi = New-Object System.Diagnostics.ProcessStartInfo($Daemon)
    $psi.Arguments = ('--state-dir "{0}" --listen 127.0.0.1:{1} --no-tray' -f $stateDir, $Port)
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
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

    # ---------- Precondition : politique STRICTE effective ----------
    # La sonde temoin DOIT etre refusee par la couche politique. Un
    # message "moteur bittorrent" prouverait que la requete a traverse
    # la politique -> le banc est invalide (daemon offline/permissif).
    $temoin = Probe 'PUT' '/downloads' $key @{ uri = 'http://127.0.0.1:1/sonde.torrent' }
    $stricte = ($temoin.code -eq 400 -and $temoin.body -match 'politique reseau')
    Verdict $stricte 'PRECONDITION : IpPolicy stricte effective' ($temoin.body -replace '\s+',' ')
    if (-not $stricte) {
        throw "politique permissive ou offline detectee -- run anti-SSRF invalide"
    }

    # ---------- Auth API ----------
    $r = Probe 'GET' '/downloads' $null
    Verdict ($r.code -eq 401) 'GET /downloads sans cle -> 401' "code=$($r.code)"
    $r = Probe 'GET' '/downloads' 'cle-invalide'
    Verdict ($r.code -eq 401) 'GET /downloads mauvaise cle -> 401' "code=$($r.code)"
    $r = Probe 'GET' '/downloads' $key
    Verdict ($r.code -eq 200) 'GET /downloads bonne cle -> 200' "code=$($r.code)"

    # ---------- Anti-SSRF : refus ferme avant toute connexion ----------
    $cibles = @(
        @{ uri = 'http://169.254.169.254/latest/meta-data'; attendu = 'link-local' },
        @{ uri = 'http://[::1]:8080/x.torrent';             attendu = 'loopback' },
        @{ uri = 'http://localhost:1/x.torrent';            attendu = 'loopback' },
        @{ uri = 'http://10.0.0.1/x.torrent';               attendu = 'priv' },
        @{ uri = 'http://192.168.0.1/x.torrent';            attendu = 'priv' },
        @{ uri = 'http://0.0.0.0/x.torrent';                attendu = 'non specifiee' }
    )
    foreach ($c in $cibles) {
        $r = Probe 'PUT' '/downloads' $key @{ uri = $c.uri }
        $refuse = ($r.code -eq 400 -and $r.body -match 'politique reseau' -and $r.body -match $c.attendu)
        Verdict $refuse "SSRF refuse : $($c.uri)" ("code=$($r.code) raison=" + ($r.body -replace '\s+',' '))
    }

    Write-Host ""
    if ($script:fails -eq 0) { Write-Host "SEC ANTI-SSRF LIVE OK"; exit 0 }
    Write-Host "SEC ANTI-SSRF LIVE ECHEC ($($script:fails) verdict(s))"
    exit 1
}
finally {
    if ($proc -and -not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}
