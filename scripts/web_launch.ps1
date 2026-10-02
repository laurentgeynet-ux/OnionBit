# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# web-launch.ps1 — lanceur de l'interface web OnionBit (dist\).
#
# Miroir du lanceur desktop (`daemon_launcher`) : sonde l'API ; si
# le daemon ne repond pas, demarre `onionbit-daemon.exe
# --state-dir <dist>\state` puis ouvre le navigateur sur
# `http://127.0.0.1:<port>/` (port reel = api/http_port_running de
# configuration.json, relu pendant le demarrage — http_port peut
# valoir 0 = aleatoire). Si le daemon tourne deja, il n'est PAS
# relance : on ouvre juste l'URL.
#
# La cle API n'a pas a etre saisie : le daemon l'injecte dans
# l'index.html servi (api/web_ui_inject_key).
#
# Installe dans dist\ par build_dist.ps1 a cote de
# « OnionBit Web.cmd ».

$ErrorActionPreference = "Stop"

$root   = Split-Path -Parent $MyInvocation.MyCommand.Path
$daemon = Join-Path $root 'onionbit-daemon.exe'
$state  = Join-Path $root 'state'
$config = Join-Path $state 'configuration.json'

function Get-ApiPort {
    # Port reel lie en priorite ; sinon port demande ; sinon 8085.
    if (Test-Path $config) {
        try {
            $api = (Get-Content $config -Raw | ConvertFrom-Json).api
            if ($api.http_port_running -gt 0) { return [int]$api.http_port_running }
            if ($api.http_port -gt 0)         { return [int]$api.http_port }
        } catch {}
    }
    return 8085
}

function Test-Api {
    param([int]$Port)
    # Code HTTP si un daemon repond (meme 401/404 = vivant),
    # $null si rien n'ecoute.
    try {
        return (Invoke-WebRequest -Uri "http://127.0.0.1:$Port/" `
            -TimeoutSec 3 -UseBasicParsing).StatusCode
    } catch {
        if ($_.Exception.Response) {
            return [int]$_.Exception.Response.StatusCode
        }
        return $null
    }
}

$port = Get-ApiPort
$code = Test-Api $port

if ($null -eq $code) {
    if (-not (Test-Path $daemon)) {
        throw "onionbit-daemon.exe introuvable dans $root"
    }
    Write-Host "Demarrage de onionbit-daemon..."
    Start-Process -FilePath $daemon `
        -ArgumentList ('--state-dir "{0}"' -f $state)
    $deadline = (Get-Date).AddSeconds(60)
    do {
        Start-Sleep -Milliseconds 500
        $port = Get-ApiPort
        $code = Test-Api $port
    } while ($null -eq $code -and (Get-Date) -lt $deadline)
}

if ($null -eq $code) {
    throw "Le daemon n'a pas demarre dans le delai imparti (http://127.0.0.1:$port/)."
}
if ($code -ne 200) {
    throw "Le daemon repond ($code) mais ne sert pas l'interface web - verifier api/web_ui_enabled et web/."
}

Start-Process "http://127.0.0.1:$port/"
