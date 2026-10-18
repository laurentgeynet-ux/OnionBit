# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# sec_browser_leak.ps1 - banc ADR-0030 etape 134 : fuite navigateur.
#
#   Lance une session .onion REELLE via le proxy Tor loopback
#   (onionbit-daemon --features tor + dillo.exe DilloRust), puis
#   surveille les sockets du PID navigateur pendant la navigation :
#
#     INTERDIT : toute connexion TCP dont l'endpoint distant n'est
#                PAS le proxy loopback ; tout endpoint UDP (DNS
#                inclus — le .onion ne doit jamais atteindre le
#                resolver local) ; tout processus enfant non attendu
#                (dpid, wget — ftp.dpi delegue a wget = connexion
#                directe hors proxy).
#     WATCH    : le profil ephemere `state\browser\run-*` doit
#                etre supprime a la fermeture du navigateur
#                (keep_profile=false, ADR-0030 §134).
#
# PRECONDITIONS : aucune elevation requise — Get-NetTCPConnection /
#   Get-NetUDPEndpoint exposent OwningProcess en mode utilisateur
#   (meme observabilite que netstat -ano). Bootstrap arti reel
#   requis : premier consensus souvent 1-3 min.
#
# Usage :
#   pwsh -NoProfile -ExecutionPolicy RemoteSigned `
#     -File scripts\sec_browser_leak.ps1 -OnionHost <hote>.onion
#
# NOTE encodage : fichier volontairement ASCII.

param(
    # Hote .onion autorise — la cible de navigation ET l'unique
    # entree de tor.allowed_hosts (la politique refuse tout le reste).
    [Parameter(Mandatory = $true)]
    [string]$OnionHost,
    # Source dillo : -DilloBin saute le build ; sinon -DilloSource
    # pointe le checkout dillorust (defaut : ..\dillorust).
    [string]$DilloBin = "",
    [string]$DilloSource = "",
    [switch]$SkipDaemonBuild,
    [int]$ApiPort = 28900,
    [int]$BootstrapTimeoutSec = 600,
    [int]$ObsSeconds = 90,
    [int]$PollMs = 500,
    [string]$StateDir = "",
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if ($OutDir -eq "") {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutDir = Join-Path $root "target\browser-leak-$stamp"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if ($StateDir -eq "") { $StateDir = Join-Path $OutDir 'daemon-state' }
New-Item -ItemType Directory -Force -Path $StateDir | Out-Null
$logFile = Join-Path $OutDir 'sec_browser_leak.log'

function Log([string]$m) {
    $line = '[{0:HH:mm:ss}] {1}' -f (Get-Date), $m
    Write-Host $line
    Add-Content -Path $logFile -Value $line
}
$script:fails = 0
$script:precondFails = 0
function Verdict([bool]$ok, [string]$name, [string]$detail = "") {
    if ($ok) { Log ("OK   {0} {1}" -f $name, $detail) }
    else     { Log ("FAIL {0} {1}" -f $name, $detail); $script:fails++ }
}
function Precond([bool]$ok, [string]$name, [string]$detail = "") {
    if ($ok) { Log ("OK   PRECOND {0} {1}" -f $name, $detail) }
    else     { Log ("FAIL PRECOND {0} {1}" -f $name, $detail); $script:precondFails++ }
}

$daemonProc = $null
$browserPid = 0
try {
    # ---------- 1) Binaires ----------
    if ($DilloBin -eq "") {
        if ($DilloSource -eq "") {
            $DilloSource = Join-Path (Split-Path -Parent $root) 'dillorust'
        }
        if (-not (Test-Path (Join-Path $DilloSource 'Cargo.toml'))) {
            throw "checkout dillorust introuvable : $DilloSource (-DilloBin pour un binaire pret)"
        }
        Log "build dillo.exe ($DilloSource)"
        & cmd /c "cargo build -p dillo --manifest-path `"$DilloSource\Cargo.toml`" 1> `"$OutDir\build_dillo.log`" 2>&1"
        if ($LASTEXITCODE -ne 0) { throw "build dillo echoue (voir build_dillo.log)" }
        $DilloBin = Join-Path $DilloSource 'target\debug\dillo.exe'
    }
    if (-not (Test-Path $DilloBin)) { throw "dillo.exe introuvable : $DilloBin" }

    if (-not $SkipDaemonBuild) {
        Log "build onionbit-daemon --features tor"
        & cmd /c "cargo build -p onionbit-daemon --features tor 1> `"$OutDir\build_daemon.log`" 2>&1"
        if ($LASTEXITCODE -ne 0) { throw "build daemon echoue (voir build_daemon.log)" }
    }
    $daemonExe = Join-Path $root 'target\debug\onionbit-daemon.exe'
    if (-not (Test-Path $daemonExe)) { throw "onionbit-daemon.exe introuvable : $daemonExe" }

    # Stage portable : binary_path relatif = resolu depuis le
    # repertoire de l'exe du daemon -> `bin\dillo.exe` a cote.
    $binStage = Join-Path (Split-Path -Parent $daemonExe) 'bin'
    New-Item -ItemType Directory -Force -Path $binStage | Out-Null
    Copy-Item $DilloBin (Join-Path $binStage 'dillo.exe') -Force
    Log "dillo stage -> $binStage\dillo.exe"

    # ---------- 2) Config + daemon ----------
    $cfg = @{
        tor = @{
            enabled = $true
            mode    = 'embedded'
            bootstrap_timeout_ms = $BootstrapTimeoutSec * 1000
            allowed_hosts = @($OnionHost)
            browser_proxy = @{
                enabled      = $true
                require_auth = $true
            }
            browser = @{
                binary_path  = 'bin\dillo.exe'
                keep_profile = $false
            }
        }
    }
    [System.IO.File]::WriteAllText((Join-Path $StateDir 'configuration.json'),
        ($cfg | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))
    $dLog = Join-Path $OutDir 'daemon.log'
    $dErr = Join-Path $OutDir 'daemon_err.log'
    $daemonProc = Start-Process -FilePath $daemonExe -PassThru -NoNewWindow `
        -ArgumentList "--state-dir `"$StateDir`" --listen 127.0.0.1:$ApiPort --no-tray" `
        -RedirectStandardOutput $dLog -RedirectStandardError $dErr
    Log "daemon demarre pid=$($daemonProc.Id) api=127.0.0.1:$ApiPort"

    $apiBase = "http://127.0.0.1:$ApiPort/api"
    $apiKey = $null
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline -and -not $apiKey) {
        try {
            $c = Get-Content (Join-Path $StateDir 'configuration.json') -Raw | ConvertFrom-Json
            if ($c.api.key) { $apiKey = $c.api.key }
        } catch {}
        if (-not $apiKey) { Start-Sleep -Milliseconds 500 }
    }
    Precond ($null -ne $apiKey) 'api_key' 'cle API lue dans configuration.json'
    if (-not $apiKey) { throw "cle API introuvable" }
    $hdr = @{ 'X-Api-Key' = $apiKey }

    # ---------- 3) Bootstrap Tor ----------
    Log "attente bootstrap Tor (timeout ${BootstrapTimeoutSec}s)"
    $deadline = (Get-Date).AddSeconds($BootstrapTimeoutSec)
    $proxyListen = $null
    $ready = $false
    while ((Get-Date) -lt $deadline -and -not $ready) {
        try {
            $st = Invoke-RestMethod -Uri "$apiBase/tor/status" -Headers $hdr -TimeoutSec 5
            if ($st.state -eq 'ready') {
                $ready = $true
                $proxyListen = $st.browser_proxy.listen
            } elseif ($st.state -eq 'error') {
                throw "bootstrap Tor en erreur : $($st.error)"
            }
        } catch { if ($_.Exception.Message -like 'bootstrap Tor*') { throw } }
        if (-not $ready) { Start-Sleep -Seconds 2 }
    }
    Precond $ready 'tor_ready' "etat=$($st.state)"
    if (-not $ready) { throw "Tor jamais ready en ${BootstrapTimeoutSec}s" }
    Precond ($null -ne $proxyListen) 'proxy_listen' "$proxyListen"
    $proxyPort = [int]($proxyListen.Split(':')[-1])
    Log "proxy loopback : $proxyListen"

    # ---------- 4) Lancement navigateur ----------
    $resp = Invoke-RestMethod -Method Post -Uri "$apiBase/browser/launch" `
        -Headers $hdr -ContentType 'application/json' `
        -Body (@{ url = "http://$OnionHost/" } | ConvertTo-Json) -TimeoutSec 10
    $browserPid = [int]$resp.pid
    Precond ($browserPid -gt 0) 'launch' "pid=$browserPid url=http://$OnionHost/"

    # ---------- 5) Observation sockets ----------
    # Ensembles : PID navigateur + descendants (un dpid/vwget enfant
    # aurait ses propres sockets — scruties aussi).
    $udpSeen = @()
    $badTcp = @()
    $childProcs = @{}
    $tcpToProxy = 0
    $deadline = (Get-Date).AddSeconds($ObsSeconds)
    while ((Get-Date) -lt $deadline) {
        $pids = @($browserPid)
        Get-CimInstance Win32_Process -Filter "ParentProcessId=$browserPid" `
            -ErrorAction SilentlyContinue | ForEach-Object {
                $pids += [int]$_.ProcessId
                # `conhost.exe` est attache par le sous-systeme console
                # Windows a tout processus GUI spawn depuis une
                # console heritee — artefact OS, pas un spawn de
                # dillo (ses sockets restent scruties plus bas).
                if ($_.Name -ne 'conhost.exe') {
                    $childProcs[[int]$_.ProcessId] = $_.Name
                }
            }
        foreach ($p in $pids) {
            foreach ($u in (Get-NetUDPEndpoint -OwningProcess $p -ErrorAction SilentlyContinue)) {
                $udpSeen += "pid=$p $($u.LocalAddress):$($u.LocalPort)"
            }
            foreach ($t in (Get-NetTCPConnection -OwningProcess $p -ErrorAction SilentlyContinue)) {
                # `Bound` = socket creee+bindee sans connect() : pas
                # de remote, pas de trafic — pas une fuite (un
                # socket() nu n'a rien emis). `Listen` : idem.
                if ($t.State -eq 'Listen' -or $t.State -eq 'Bound') { continue }
                $remote = "$($t.RemoteAddress):$($t.RemotePort)"
                $isProxy = ($t.RemotePort -eq $proxyPort) -and
                           ($t.RemoteAddress -eq '127.0.0.1' -or $t.RemoteAddress -eq '::1')
                if ($isProxy) { $tcpToProxy++ }
                else { $badTcp += "pid=$p $($t.State) $remote" }
            }
        }
        $alive = Get-Process -Id $browserPid -ErrorAction SilentlyContinue
        if (-not $alive) { Log "navigateur termine avant la fin de la fenetre"; break }
        Start-Sleep -Milliseconds $PollMs
    }

    # ---------- 6) Wipe du profil ephemere ----------
    if ($browserPid -gt 0) {
        Stop-Process -Id $browserPid -Force -ErrorAction SilentlyContinue
    }
    $browserRoot = Join-Path $StateDir 'browser'
    $wiped = $false
    for ($i = 0; $i -lt 40 -and -not $wiped; $i++) {
        $left = Get-ChildItem $browserRoot -Directory -Filter 'run-*' `
            -ErrorAction SilentlyContinue
        if (-not $left) { $wiped = $true } else { Start-Sleep -Milliseconds 250 }
    }

    # ---------- 7) Verdicts ----------
    Precond ($tcpToProxy -gt 0) 'tcp_to_proxy' "$tcpToProxy connexions loopback->$proxyListen observees"
    Verdict ($udpSeen.Count -eq 0) 'zero_udp' "$(@($udpSeen | Select-Object -First 5) -join '; ')"
    Verdict ($badTcp.Count -eq 0) 'tcp_proxy_seul' "$(@($badTcp | Select-Object -First 5) -join '; ')"
    Verdict ($childProcs.Count -eq 0) 'pas_de_processus_enfant' "$(($childProcs.Values | Select-Object -Unique) -join ', ')"
    Verdict $wiped 'profil_ephemere_wipe' "browser\run-* supprime apres exit=$wiped"
}
finally {
    if ($browserPid -gt 0) { Stop-Process -Id $browserPid -Force -ErrorAction SilentlyContinue }
    if ($daemonProc) { Stop-Process -Id $daemonProc.Id -Force -ErrorAction SilentlyContinue }
    $manifest = @{
        onion_host = $OnionHost
        daemon_pid = if ($daemonProc) { $daemonProc.Id } else { 0 }
        browser_pid = $browserPid
        fails = $script:fails
        precond_fails = $script:precondFails
        utc = (Get-Date).ToUniversalTime().ToString('o')
    }
    $manifest | ConvertTo-Json | Set-Content (Join-Path $OutDir 'manifest.json')
    $verdict = if ($script:precondFails -gt 0) { 'INVALID_PRECONDITION' }
               elseif ($script:fails -eq 0)   { 'NO_LEAK' }
               else                            { "LEAK ($($script:fails) FAIL)" }
    Log "VERDICT: $verdict  (manifest: $OutDir\manifest.json)"
}
