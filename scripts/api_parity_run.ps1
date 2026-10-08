# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# api_parity_run.ps1 - lance Tribler.exe -s ET onionbit-daemon en etats
# isoles, attend que les deux API REST soient en ligne, puis execute le
# banc `api_parity.ps1`.
#
# Usage :
#   pwsh -NoProfile -ExecutionPolicy RemoteSigned -File scripts\api_parity_run.ps1 [-FailOnDiff]

[CmdletBinding()]
param(
    [switch] $FailOnDiff,
    [switch] $ShowDiff,
    [int] $StartupTimeoutSec = 120
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
$triblerExe = if ($env:TRIBLER_EXE) { $env:TRIBLER_EXE } else { "C:\Program Files (x86)\Tribler\Tribler.exe" }
$outDir = Join-Path $root "target\api-parity"
$triblerState = Join-Path $outDir "tribler-state"
$rustState = Join-Path $outDir "rust-state"
$confDir = Join-Path $triblerState "8.0"          # VERSION_SUBDIR
$confFile = Join-Path $confDir "configuration.json"

function Get-FreeTcpPort {
    $l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $l.Start()
    $p = $l.LocalEndpoint.Port
    $l.Stop()
    return $p
}
function Get-FreeUdpPort {
    $u = [System.Net.Sockets.UdpClient]::new([System.Net.IPAddress]::Loopback, 0)
    $p = $u.Client.LocalEndPoint.Port
    $u.Close()
    return $p
}

$pyApiPort = Get-FreeTcpPort
$rsApiPort = Get-FreeTcpPort
$pyUdpPort = Get-FreeUdpPort
$rsUdpPort = Get-FreeUdpPort
$pyApiKey = "paritybench"

New-Item -ItemType Directory -Force -Path $confDir | Out-Null

# Config Tribler minimale (comme interop_tribler.ps1) — toutefois
# tunnel + DHT actifs pour couvrir `/api/ipv8/tunnel/*` et
# `/api/ipv8/dht/*` dans le banc.
$conf = @{
    api = @{
        http_enabled = $true
        http_port = $pyApiPort
        http_host = "127.0.0.1"
        https_enabled = $false
        key = $pyApiKey
    }
    ipv8 = @{
        logger = @{ level = "INFO" }
        interfaces = @(
            @{ interface = "UDPIPv4"; ip = "0.0.0.0"; port = $pyUdpPort }
        )
        walker_interval = 5.0
        overlays = @(
            @{
                class = "DiscoveryCommunity"
                key = "anonymous id"
                walkers = @()
                bootstrappers = @(
                    @{ class = "DispersyBootstrapper"; init = @{ ip_addresses = @(); dns_addresses = @(); bootstrap_timeout = 1.0 } }
                )
                initialize = @{}
                on_start = @()
            }
        )
    }
    libtorrent = @{
        port = 0
        utp = $true
        dht = $false
        upnp = $false
        natpmp = $false
        lsd = $false
    }
    tunnel_community = @{ enabled = $true; min_circuits = 0; max_circuits = 8 }
    content_discovery = @{ enabled = $false }
    dht_discovery = @{ enabled = $true }
    recommender = @{ enabled = $false }
    rendezvous = @{ enabled = $false }
    # `rss`/`versioning` actives pour que les endpoints soient montes
    # cote Python (sinon 404/500 artificiels sur le banc).
    rss = @{ enabled = $true; urls = @() }
    torrent_checker = @{ enabled = $false }
    versioning = @{ enabled = $true }
    watch_folder = @{ enabled = $false }
    statistics = $false
}
# IMPORTANT : SANS BOM — `json.load` de TriblerConfigManager echoue
# sinon et repartirait sur DEFAULT_CONFIG.
$json = $conf | ConvertTo-Json -Depth 10
[System.IO.File]::WriteAllText($confFile, $json, [System.Text.UTF8Encoding]::new($false))

Write-Host "== build onionbit-daemon =="
cargo build -p onionbit-daemon
if ($LASTEXITCODE -ne 0) { throw "build echoue" }
$rustBin = Join-Path $root "target\debug\onionbit-daemon.exe"

Get-Process -Name "Tribler" -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $triblerExe } | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

Write-Host "== Tribler.exe -s (api :$pyApiPort) =="
$env:TSTATEDIR = $triblerState
$env:CORE_API_PORT = "$pyApiPort"
$env:CORE_API_KEY = $pyApiKey
$pyProc = Start-Process -FilePath $triblerExe -PassThru -NoNewWindow `
    -ArgumentList "-s", "--log-level", "INFO" `
    -RedirectStandardOutput (Join-Path $outDir "tribler_stdout.log") `
    -RedirectStandardError (Join-Path $outDir "tribler_stderr.log")

Write-Host "== onionbit-daemon (api :$rsApiPort, ipv8 :$rsUdpPort) =="
$rsProc = Start-Process -FilePath $rustBin -PassThru -NoNewWindow `
    -ArgumentList "--state-dir", "`"$rustState`"", "--listen", "127.0.0.1:$rsApiPort", "--ipv8-port", "$rsUdpPort", "--no-tray" `
    -RedirectStandardOutput (Join-Path $outDir "rust_stdout.log") `
    -RedirectStandardError (Join-Path $outDir "rust_stderr.log")

function Wait-Api {
    param([string] $Url, [string] $Key, [string] $Name)
    $deadline = (Get-Date).AddSeconds($StartupTimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $null = Invoke-RestMethod -Uri "$Url/api/ipv8/overlays" `
                -Headers @{ "X-Api-Key" = $Key } -TimeoutSec 3
            return $true
        } catch {
            Start-Sleep -Seconds 1
        }
    }
    Write-Host "API $Name jamais en ligne - voir $outDir" -ForegroundColor Red
    return $false
}

$rsKey = $null
try {
    # Cle API Rust : generee dans `configuration.json` au premier run.
    $rsConfFile = Join-Path $rustState "configuration.json"
    $deadline = (Get-Date).AddSeconds(30)
    while (-not (Test-Path $rsConfFile) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
    $rsKey = (Get-Content $rsConfFile -Raw | ConvertFrom-Json).api.key
    if (-not $rsKey) { throw "cle api absente de $rsConfFile" }

    $pyUp = Wait-Api -Url "http://127.0.0.1:$pyApiPort" -Key $pyApiKey -Name "Tribler"
    $rsUp = Wait-Api -Url "http://127.0.0.1:$rsApiPort" -Key $rsKey -Name "Rust"
    if (-not ($pyUp -and $rsUp)) { throw "un des daemons n'est pas en ligne" }

    Write-Host "== banc de parite =="
    $parityArgs = @{
        TriblerUrl = "http://127.0.0.1:$pyApiPort"
        TriblerKey = $pyApiKey
        RustUrl    = "http://127.0.0.1:$rsApiPort"
        RustKey    = $rsKey
    }
    if ($FailOnDiff) { $parityArgs.FailOnDiff = $true }
    if ($ShowDiff) { $parityArgs.ShowDiff = $true }
    & "$PSScriptRoot\api_parity.ps1" @parityArgs
    $parityExit = $LASTEXITCODE
} finally {
    if ($pyProc -and -not $pyProc.HasExited) { Stop-Process -Id $pyProc.Id -Force -ErrorAction SilentlyContinue }
    if ($rsProc -and -not $rsProc.HasExited) { Stop-Process -Id $rsProc.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item Env:TSTATEDIR -ErrorAction SilentlyContinue
    Remove-Item Env:CORE_API_PORT -ErrorAction SilentlyContinue
    Remove-Item Env:CORE_API_KEY -ErrorAction SilentlyContinue
}
exit ($parityExit | ForEach-Object { $_ })
