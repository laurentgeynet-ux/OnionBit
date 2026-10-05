# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# sec_leak_capture.ps1 - banc P0-17b : observation reseau au niveau OS.
#
#   Capture pktmon (paquets complets) pendant un telechargement
#   anonyme REEL a sauts libres sur le reseau Tribler public
#   (examples/interop_public_download, meme banc que
#   interop_public_dht.ps1), puis classification hors-ligne de chaque
#   endpoint distant par scripts/analyze_leak_capture.py :
#
#     OVERLAY  : endpoint vu dans le journal TAP du processus teste
#                (verite fil : le noeud ne parle qu'UDP IPv8/tunnel)
#     DNS      : port 53 -- qnames decodes et rapportes
#     INTERDIT : TCP vers WAN, UDP vers WAN hors overlay, routeurs DHT
#                mainline contactes en direct (dht.libtorrent.org etc.)
#
#   La capture inclut une fenetre post-arret (-PostExitSec) : tout
#   datagramme WAN emis apres la mort du processus = trafic fantome.
#
# PRECONDITIONS : droits administrateur (pktmon). Sans elevation, le
# script se relance via UAC sauf si -NoElevate.
#
# Usage (shell admin) :
#   powershell -NoProfile -ExecutionPolicy RemoteSigned `
#     -File scripts\sec_leak_capture.ps1 [-Hops 2] [-MinBytes 1048576]
#
# NOTE encodage : fichier volontairement ASCII.

param(
    [string]$Magnet = 'magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10',
    [int]$Hops = 2,
    [long]$MinBytes = 1048576,
    [int]$WalkSeconds = 30,
    [int]$DownloadTimeoutSec = 240,
    [int]$MaxCircuits = 4,
    [int]$PostExitSec = 30,
    [int]$TriblerWaitSec = 180,
    [int]$DaemonApiPort = 28700,
    [int]$DaemonIpv8Port = 28800,
    # P0-17c : injection de panne en plein transfert.
    #   normal         : chemin heureux + fenetre post-arret (P0-17b)
    #   kill           : tue le processus de banc a -FailAtBytes recus
    #                    (17c-4 : ports silencieux, 0 fantome)
    #   block          : regle pare-feu sortante+entrante sur les
    #                    premiers sauts reels observes (17c-1 : mort de
    #                    circuit, proxy vivant ; un fallback direct
    #                    resterait VISIBLE car seuls les endpoints
    #                    overlay sont bloques)
    #   wan            : Disable-NetAdapter sur l'interface physique
    #                    active puis retablissement (17c-3)
    #   kill-bootstrap : mort de Tribler.exe local en plein transfert
    #                    (17c-2 : le worker proxy est in-process, la
    #                    mort d'infrastructure en est le pendant OS)
    #   lane-reset     : destruction de la lane anonyme via
    #                    DELETE /api/ipv8/tunnel/anon_lanes/{hops}
    #                    pendant un download anonyme du DAEMON
    #                    (17c-5 : aucun paquet/mapping de l'ancienne
    #                    lane ne doit survivre ; reprise via une lane
    #                    recreee sur de nouveaux ports)
    [ValidateSet('normal','kill','block','wan','kill-bootstrap','lane-reset')]
    [string]$Scenario = 'normal',
    [long]$FailAtBytes = 262144,
    [int]$FailWindowSec = 45,
    # lane-reset (17c-5, rejeu) : -DedicatedSeed cree un torrent local
    # via /api/createtorrent du daemon et le fait seeder par le
    # Tribler.exe bootstrap - les octets FICHIER circulent
    # reellement ; -RequireFileBytes rend le declencheur strict sur le
    # payload (sinon max(fichier, trafic tunnel), fallback documente
    # du run initial ou la resolution magnet avait stalle).
    [switch]$DedicatedSeed,
    [string]$SeedFile = "",
    [switch]$RequireFileBytes,
    [string]$OutDir = "",
    [switch]$NoElevate,
    [switch]$Inner
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if ($OutDir -eq "") {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutDir = Join-Path $root "target\leak-capture-$stamp"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logFile = Join-Path $OutDir 'sec_leak_capture.log'

function Log([string]$m) {
    $line = '[{0:HH:mm:ss}] {1}' -f (Get-Date), $m
    Write-Host $line
    Add-Content -Path $logFile -Value $line
}

# ---------- Elevation ----------
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    if ($NoElevate) {
        throw "droits administrateur requis pour pktmon (relancer en shell eleve ou sans -NoElevate)"
    }
    Log "re-lancement en admin via UAC -> $logFile"
    $psArgs = @(
        '-NoProfile', '-ExecutionPolicy', 'RemoteSigned',
        '-File', "`"$PSCommandPath`"",
        '-Inner',
        '-Magnet', "`"$Magnet`"",
        '-Hops', "$Hops", '-MinBytes', "$MinBytes",
        '-WalkSeconds', "$WalkSeconds",
        '-DownloadTimeoutSec', "$DownloadTimeoutSec",
        '-MaxCircuits', "$MaxCircuits",
        '-PostExitSec', "$PostExitSec",
        '-Scenario', "$Scenario",
        '-FailAtBytes', "$FailAtBytes",
        '-FailWindowSec', "$FailWindowSec",
        '-TriblerWaitSec', "$TriblerWaitSec",
        '-DaemonApiPort', "$DaemonApiPort",
        '-DaemonIpv8Port', "$DaemonIpv8Port",
        '-OutDir', "`"$OutDir`""
    )
    if ($DedicatedSeed)     { $psArgs += '-DedicatedSeed' }
    if ($RequireFileBytes)  { $psArgs += '-RequireFileBytes' }
    if ($SeedFile -ne "")   { $psArgs += @('-SeedFile', "`"$SeedFile`"") }
    $p = Start-Process -FilePath 'pwsh' -Verb RunAs -Wait -PassThru `
        -ArgumentList $psArgs -WindowStyle Normal
    Write-Host "code de sortie du processus eleve : $($p.ExitCode)"
    exit $p.ExitCode
}

# ---------- Phase elevee ----------
$script:fails = 0
function Verdict([bool]$ok, [string]$name, [string]$detail = "") {
    if ($ok) { Log ("OK   {0} {1}" -f $name, $detail) }
    else     { Log ("FAIL {0} {1}" -f $name, $detail); $script:fails++ }
}

# Preconditions du banc (distinctes des oracles) : leur echec
# invalide le RESULTAT SECURITE sans etre un bug du code - le
# manifeste affiche alors INVALID_PRECONDITION meme si INTERDIT=0,
# car la fenetre observee n'a pas les preuves requises (ex. trigger
# payload jamais atteint, seeder absent, capture inanalysable).
$script:precond = [System.Collections.Generic.List[string]]::new()
$script:precondFails = 0
function Precond([bool]$ok, [string]$name, [string]$detail = "") {
    if ($ok) { Log ("OK   PRECOND {0} {1}" -f $name, $detail) }
    else {
        Log ("FAIL PRECOND {0} {1}" -f $name, $detail)
        $script:precondFails++
        $script:precond.Add("$name - $detail")
    }
}

$triblerExe = "C:\Program Files (x86)\Tribler\Tribler.exe"
$stateDir = Join-Path $env:APPDATA ".Tribler"
$confFile = Join-Path $stateDir "8.0\configuration.json"
$triblerProc = $null; $startedTribler = $false; $rsProc = $null; $daemonProc = $null
$captureStarted = $false
$t0 = Get-Date

try {
    if (-not (Test-Path $confFile)) { throw "configuration.json Tribler introuvable : $confFile" }
    $conf = Get-Content $confFile -Raw | ConvertFrom-Json
    $ipv4 = $conf.ipv8.interfaces | Where-Object { $_.interface -eq "UDPIPv4" } | Select-Object -First 1
    $triblerPort = [int] $ipv4.port
    $apiKey = $conf.api.key
    $apiPort = $conf.api.http_port_running

    # ---------- Tribler.exe : point d'entree de l'overlay ----------
    $triblerProc = Get-Process -Name "Tribler" -ErrorAction SilentlyContinue
    if (-not $triblerProc) {
        Log "lancement Tribler.exe (GUI, bootstrap reseau reel)"
        $triblerProc = Start-Process -FilePath $triblerExe -PassThru `
            -RedirectStandardOutput (Join-Path $OutDir "tribler_stdout.log") `
            -RedirectStandardError (Join-Path $OutDir "tribler_stderr.log")
        $startedTribler = $true
    } else {
        Log "Tribler.exe deja en cours (pid $($triblerProc.Id))"
    }
    $deadline = (Get-Date).AddSeconds($TriblerWaitSec)
    $peersOk = $false
    while ((Get-Date) -lt $deadline -and -not $peersOk) {
        try {
            $conf2 = Get-Content $confFile -Raw | ConvertFrom-Json
            if ($conf2.api.http_port_running -gt 0) { $apiPort = $conf2.api.http_port_running }
            if ($conf2.api.key) { $apiKey = $conf2.api.key }
            $net = Invoke-RestMethod -Uri "http://127.0.0.1:$apiPort/api/ipv8/network" `
                -Headers @{ "X-Api-Key" = $apiKey } -TimeoutSec 3
            $n = @($net.network.peers).Count
            if ($n -gt 0) { $peersOk = $true; Log "overlay Tribler : $n pair(s) connu(s)" }
        } catch { Start-Sleep -Seconds 2 }
        if (-not $peersOk) { Start-Sleep -Seconds 3 }
    }
    if (-not $peersOk) { throw "Tribler.exe n'a decouvert aucun pair en ${TriblerWaitSec}s" }

    # ---------- Build du banc ----------
    Log "build interop_public_download"
    cargo build -p onionbit-bittorrent --example interop_public_download 2>&1 |
        Out-File (Join-Path $OutDir 'build.log')
    if ($LASTEXITCODE -ne 0) { throw "build echoue (voir build.log)" }
    $exe = Join-Path $root "target\debug\examples\interop_public_download.exe"

    # ---------- Ensembles de classification ----------
    # Routeurs DHT mainline : interdits EN DIRECT (ils doivent passer
    # par le tunnel, donc n'apparaitre localement que vers le 1er saut).
    $routerHosts = @('dht.libtorrent.org', 'dht.transmissionbt.com',
                     'router.bittorrent.com', 'router.utorrent.com', 'dht.aelitis.com')
    $dhtForbidden = @()
    foreach ($h in $routerHosts) {
        try {
            foreach ($a in [System.Net.Dns]::GetHostAddresses($h)) {
                if ($a.AddressFamily -eq 'InterNetwork') { $dhtForbidden += "$($a.IPAddressToString)" }
            }
        } catch {}
    }
    Log ("routeurs DHT resolus : {0}" -f ($dhtForbidden -join ', '))
    $resolvers = @(Get-DnsClientServerAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        ForEach-Object { $_.ServerAddresses } | Where-Object { $_ -and $_ -ne '0.0.0.0' })
    Log ("resolveurs DNS : {0}" -f ($resolvers -join ', '))

    # ===================== Scenario lane-reset (17c-5) =====================
    # Flux distinct : le banc est le DAEMON (la lane anonyme y est un
    # objet AnonLane de la stack, absente de l'exemple de bench).
    # Destruction via l'endpoint de diagnostic, observation : ports de
    # lane liberes, zero paquet sortant des anciens ports, reprise via
    # une lane recreee. Attribution : les ports UDP du PID du daemon.
    if ($Scenario -eq 'lane-reset') {
        Log "build onionbit-daemon"
        cargo build -p onionbit-daemon 2>&1 | Out-File (Join-Path $OutDir 'build_daemon.log')
        if ($LASTEXITCODE -ne 0) { throw "build daemon echoue (voir build_daemon.log)" }
        $daemonExe = Join-Path $root 'target\debug\onionbit-daemon.exe'

        $dstate = Join-Path $OutDir 'daemon-state'
        New-Item -ItemType Directory -Force -Path $dstate | Out-Null
        $dcfg = @{
            tunnel_community = @{
                enabled          = $true
                exitnode_enabled = $false
                min_circuits     = 2
                max_circuits     = 4
            }
            ipv8 = @{
                bootstrap  = @{ override = @("127.0.0.1:$triblerPort") }
                interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $DaemonIpv8Port } )
            }
            dht_discovery = @{ enabled = $true }
            # Ambiant reduit a zero pour l'oracle : sans DHT mainline
            # ni UPnP/NAT-PMP sur la session en clair, TOUT paquet WAN
            # emis par un port du daemon est suspect par construction.
            # La lane anonyme garde sa DHT tunnelsee (`enable_dht`
            # force cote `anon_engine`, socket virtuelle -> ipv8).
            libtorrent    = @{ port = 0; dht = $false; upnp = $false
                               natpmp = $false; lsd = $false }
        }
        [System.IO.File]::WriteAllText((Join-Path $dstate 'configuration.json'),
            ($dcfg | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))
        $dLog = Join-Path $OutDir 'daemon.log'
        $dErr = Join-Path $OutDir 'daemon_err.log'
        $daemonProc = Start-Process -FilePath $daemonExe -PassThru -NoNewWindow `
            -ArgumentList "--state-dir `"$dstate`" --listen 127.0.0.1:$DaemonApiPort --no-tray" `
            -RedirectStandardOutput $dLog -RedirectStandardError $dErr
        $pidBench = $daemonProc.Id
        Log "daemon demarre pid=$pidBench api=127.0.0.1:$DaemonApiPort ipv8=$DaemonIpv8Port"

        # Cle API auto-generee dans configuration.json.
        $apiBase = "http://127.0.0.1:$DaemonApiPort/api"
        $daemonKey = $null
        $deadline = (Get-Date).AddSeconds(60)
        while ((Get-Date) -lt $deadline -and -not $daemonKey) {
            try {
                $c = Get-Content (Join-Path $dstate 'configuration.json') -Raw | ConvertFrom-Json
                if ($c.api -and $c.api.key) { $daemonKey = $c.api.key }
            } catch {}
            if (-not $daemonKey) { Start-Sleep -Milliseconds 500 }
        }
        if (-not $daemonKey) { throw "cle API du daemon introuvable dans configuration.json" }
        $H = @{ "X-Api-Key" = $daemonKey }
        function DApiGet([string]$p) {
            Invoke-RestMethod -Uri "$apiBase$p" -Headers $H -TimeoutSec 5
        }
        Verdict (-not $daemonProc.HasExited) 'daemon vivant' "pid=$pidBench"

        # ---------- Seed dedie (17c-5, rejeu optionnel) -------------
        # Le declencheur payload-reel exige que les octets FICHIER
        # circulent : on cree un torrent local puis on le fait seeder
        # par le Tribler.exe bootstrap (peer reel, joignable par les
        # exits via son adresse overlay). Sans seed dedie, la
        # resolution magnet sur mesh clairseme peut n'amener que du
        # trafic tunnel - le fallback max(fichier, tunnel) du run
        # initial masquait alors l'absence de payload.
        $seedIh = $null; $seedSha = $null; $seedSize = $null
        $seedingConfirmed = $false; $triblerLtPort = $null
        $bytesAtFailure = $null; $bytesAfter = $null; $payloadHashMatch = $null
        if ($DedicatedSeed) {
            $seedDir = Join-Path $OutDir 'seed'
            New-Item -ItemType Directory -Force -Path $seedDir | Out-Null
            if ($SeedFile -eq "") {
                $SeedFile = Join-Path $seedDir 'payload.bin'
                $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
                $buf = New-Object byte[] (4MB)
                $rng.GetBytes($buf)
                [System.IO.File]::WriteAllBytes($SeedFile, $buf)
            }
            # Empreinte de la source : le manifeste lie le verdict au
            # contenu exact seede (SHA-256 + taille attendue).
            $seedSize = (Get-Item $SeedFile).Length
            $seedSha = (Get-FileHash $SeedFile -Algorithm SHA256).Hash.ToLower()
            $cr = Invoke-RestMethod -Method Post -Uri "$apiBase/createtorrent" `
                -Headers $H -ContentType 'application/json' `
                -Body (@{ files = @($SeedFile); name = 'onionbit-17c5-seed';
                          export_dir = $seedDir } | ConvertTo-Json -Compress) `
                -TimeoutSec 60
            $torPath = $cr.results[0].path; $seedIh = $cr.results[0].infohash
            Verdict ($null -ne $seedIh) 'torrent dedie cree' "ih=$seedIh"
            try {
                $uri = 'file:///' + ($torPath -replace '\\', '/')
                Invoke-RestMethod -Method Put `
                    -Uri "http://127.0.0.1:$apiPort/api/downloads" `
                    -Headers @{ 'X-Api-Key' = $apiKey } -ContentType 'application/json' `
                    -Body (@{ uri = $uri; anon_hops = 0; safe_seeding = $false;
                              destination = $seedDir } | ConvertTo-Json -Compress) `
                    -TimeoutSec 15 | Out-Null
                Verdict $true 'seed dedie ajoute sur Tribler' "ih=$seedIh"
            } catch {
                Verdict $false 'seed dedie ajoute sur Tribler' $_.Exception.Message
            }
            try { $triblerLtPort = $conf.libtorrent.port } catch {}
            # Le seeder doit etre en etat seeding (verification
            # terminee) AVANT l'ajout du download anonyme - sinon les
            # octets < fichier > ne seraient que des metadonnees.
            $seedDeadline = (Get-Date).AddSeconds(60)
            while ((Get-Date) -lt $seedDeadline -and -not $seedingConfirmed) {
                try {
                    $td = Invoke-RestMethod `
                        -Uri "http://127.0.0.1:$apiPort/api/downloads" `
                        -Headers @{ 'X-Api-Key' = $apiKey } -TimeoutSec 10
                    $mine = @($td.downloads) | Where-Object { $_.infohash -eq $seedIh }
                    if ($mine) {
                        if ([double]$mine[0].progress -ge 1 `
                            -or [string]$mine[0].status -match 'seed') {
                            $seedingConfirmed = $true
                        }
                    }
                } catch {}
                if (-not $seedingConfirmed) { Start-Sleep -Seconds 2 }
            }
            Precond $seedingConfirmed 'seeder Tribler en etat seeding' `
                "ih=$seedIh lt_port=$triblerLtPort"
            Log ("seed dedie : ih={0} sha256={1}... size={2}" -f `
                $seedIh, $seedSha.Substring(0, 16), $seedSize)
            $Magnet = "magnet:?xt=urn:btih:$seedIh"
        }

        # Attente de pairs overlay reels via le bootstrap Tribler.
        $deadline = (Get-Date).AddSeconds($TriblerWaitSec)
        $peersOk = $false
        while ((Get-Date) -lt $deadline -and -not $peersOk) {
            try {
                $net = DApiGet '/ipv8/network'
                $n = @($net.network.peers).Count
                if ($n -gt 0) { $peersOk = $true; Log "daemon : $n pair(s) overlay connu(s)" }
            } catch { Start-Sleep -Seconds 2 }
            if (-not $peersOk) { Start-Sleep -Seconds 2 }
        }
        if (-not $peersOk) { throw "le daemon n'a decouvert aucun pair en ${TriblerWaitSec}s" }

        # Ports UDP du daemon AVANT la lane (baseline : ipv8 + dht +
        # libtorrent). Les nouveaux ports apres `add` = sockets de lane.
        $portsBefore = @(Get-NetUDPEndpoint -OwningProcess $pidBench -ErrorAction SilentlyContinue `
            | Where-Object { $_.LocalAddress -match '^\d+\.\d+\.\d+\.\d+$' } | ForEach-Object { $_.LocalPort } | Sort-Object -Unique)
        Log ("ports UDP daemon (avant lane) : {0}" -f ($portsBefore -join ', '))

        # ---------- Capture ----------
        $etl = Join-Path $OutDir 'capture.etl'
        $pcap = Join-Path $OutDir 'capture.pcapng'
        & pktmon filter remove 2>&1 | Out-Null
        & pktmon start --capture --pkt-size 0 --file-name $etl --file-size 512 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "pktmon start a echoue" }
        $captureStarted = $true
        $capStart = Get-Date
        Log "capture pktmon demarree -> $etl"

        # ---------- Download anonyme puis destruction de lane ----------
        $dlDir = Join-Path $OutDir 'dl'
        # Les sockets datagramme d'une lane anonyme sont VIRTUELLES
        # (TunnelUdpSocket : le trafic est encapsule en cellules IPv8
        # sur le port ipv8 partage) - la seule socket OS reelle creee
        # par la lane est le listener TCP SOCKS5 loopback. C'est le
        # mapping OS a observer : il doit mourir avec la lane et la
        # lane recreee doit binder un port DIFFERENT.
        $tcpBefore = @(Get-NetTCPConnection -OwningProcess $pidBench -State Listen `
            -ErrorAction SilentlyContinue | ForEach-Object { $_.LocalPort } | Sort-Object -Unique)
        $body = @{ uri = $Magnet; anon_hops = $Hops; safe_seeding = $true; destination = $dlDir } |
            ConvertTo-Json -Compress
        $addResp = Invoke-RestMethod -Method Put -Uri "$apiBase/downloads" -Headers $H `
            -ContentType 'application/json' -Body $body -TimeoutSec 15
        $infohashHex = [string]$addResp.infohash
        $triggerDesc = if ($RequireFileBytes) { 'octets FICHIER (strict)' }
                       else { 'max(fichier, trafic tunnel)' }
        Log (("download anonyme {0} saut(s) ajoute (ih={1}) - attente de {2} octets " +
            "[{3}]") -f $Hops, $infohashHex, $FailAtBytes, $triggerDesc)

        # Declencheur "mid-transfer" : la lane transporte du trafic
        # reel des que ses circuits DATA shuttent des cellules
        # (resolution magnet, DHT tunnelise, metadonnees uTP). Sur un
        # mesh clairseme les octets FICHIER peuvent tarder des minutes
        # alors que la lane transfere deja : l'oracle INTERDIT=0 porte
        # sur la lane detruite, pas sur la progression du torrent.
        # Par defaut on prend donc max(octets fichier, octets circuits
        # DATA READY) ; -RequireFileBytes (rejeu 17c-5 avec seed
        # dedie) exige les octets payload reels.
        $tFailure = $null; $lanePorts = @(); $tLaneGone = $null
        $tResume = $null; $tNewReady = $null
        $laneSocks = @(); $newSocks = @()
        $got = 0
        $fileBytes = 0; $verifiedBytes = 0; $tunnelBytes = 0
        $mine0 = @()
        $deadline = (Get-Date).AddSeconds($DownloadTimeoutSec)
        while ((Get-Date) -lt $deadline -and -not $tFailure) {
            try {
                if (-not $laneSocks) {
                    $tcpNow = @(Get-NetTCPConnection -OwningProcess $pidBench -State Listen `
                        -ErrorAction SilentlyContinue | ForEach-Object { $_.LocalPort })
                    $laneSocks = @($tcpNow | Where-Object { $tcpBefore -notcontains $_ })
                }
                $dls = @(DApiGet '/downloads' | ForEach-Object { $_.downloads })
                $fileBytes = [int64]($dls | ForEach-Object { [int64]$_.session_download } |
                    Measure-Object -Sum).Sum
                $cir = DApiGet '/ipv8/tunnel/circuits'
                $tunnelBytes = [int64](@($cir.circuits) |
                    Where-Object { $_.type -eq 'DATA' -and $_.state -eq 'READY' `
                        -and $_.goal_hops -eq $Hops } |
                    ForEach-Object { [int64]$_.bytes_up + [int64]$_.bytes_down } |
                    Measure-Object -Sum).Sum
                        # Octets < verifies > = progress * size de NOTRE
                # download (session_download = recus, non verifies -
                # une piece encore non hashee ne compte pas comme
                # payload livre).
                $mine0 = @($dls) | Where-Object { $_.infohash -eq $infohashHex }
                $verifiedBytes = [int64]($mine0 | ForEach-Object {
                    [double]$_.progress * [int64]$_.size } | Measure-Object -Sum).Sum
                $got = if ($RequireFileBytes) { $verifiedBytes }
                       else { [Math]::Max($fileBytes, $tunnelBytes) }
            } catch {}
            if ($got -ge $FailAtBytes -and $laneSocks.Count -gt 0) {
                $now = @(Get-NetUDPEndpoint -OwningProcess $pidBench -ErrorAction SilentlyContinue `
                    | Where-Object { $_.LocalAddress -match '^\d+\.\d+\.\d+\.\d+$' } | ForEach-Object { $_.LocalPort } | Sort-Object -Unique)
                $lanePorts = @($now | Where-Object { $portsBefore -notcontains $_ })
                $bytesAtFailure = @{
                    progress = if ($mine0) { [double]$mine0[0].progress } else { $null }
                    size_selected = if ($mine0) { [int64]$mine0[0].size } else { $null }
                    verified_estimated = $verifiedBytes
                    session_download = $fileBytes
                    tunnel = $tunnelBytes }
                $tFailure = Get-Date
                Log ("INJECTION lane-reset : DELETE anon_lanes/{0} a {1} octets ; socks=[{2}]" -f `
                    $Hops, $got, ($laneSocks -join ', '))
                $r = Invoke-RestMethod -Method Delete `
                    -Uri "$apiBase/ipv8/tunnel/anon_lanes/$Hops" -Headers $H -TimeoutSec 30
                Verdict ($r.success -eq $true) 'DELETE lane accepte' "hops=$Hops"
                try {
                    Invoke-RestMethod -Method Delete `
                        -Uri "$apiBase/ipv8/tunnel/anon_lanes/$Hops" -Headers $H -TimeoutSec 10 | Out-Null
                    Verdict $false 're-DELETE sans lane -> 404' 'reponse 200 inattendue'
                } catch {
                    Verdict ($_.Exception.Response.StatusCode.value__ -eq 404) `
                        're-DELETE sans lane -> 404' $_.Exception.Response.StatusCode
                }
            } else { Start-Sleep -Seconds 1 }
        }
        Precond ($laneSocks.Count -gt 0) 'lane anonyme active avant injection (SOCKS)' `
            "n=$($laneSocks.Count)"
        Precond ($got -ge $FailAtBytes) "transfert actif a l'injection (>= $FailAtBytes)" `
            "dernier=$got file=$fileBytes verified=$verifiedBytes tunnel=$tunnelBytes"
        Precond ($null -ne $tFailure) 'panne lane-reset injectee' `
            $(if ($tFailure) { "t=$($tFailure.ToString('HH:mm:ss.fff'))" } else { 'jamais' })
        # Toute la section destructive n'a de sens que si l'injection
        # a eu lieu ; sinon on saute directement a l'arret de capture
        # et le manifeste portera INVALID_PRECONDITION.
        if ($tFailure) {
        # Liberation effective : le listener TCP de la lane detruite
        # doit mourir ET l'API doit annoncer la lane absente.
        $deadline = (Get-Date).AddSeconds(20)
        $laneGoneApi = $false
        while ((Get-Date) -lt $deadline -and -not $tLaneGone) {
            $tcpNow = @(Get-NetTCPConnection -OwningProcess $pidBench -State Listen `
                -ErrorAction SilentlyContinue | ForEach-Object { $_.LocalPort })
            $socksGone = -not ($laneSocks | Where-Object { $tcpNow -contains $_ })
            if (-not $laneGoneApi) {
                try {
                    DApiGet "/libtorrent/session?hop=$Hops" | Out-Null
                } catch {
                    if ($_.Exception.Response.StatusCode.value__ -eq 404) { $laneGoneApi = $true }
                }
            }
            if ($socksGone -and $laneGoneApi) { $tLaneGone = Get-Date }
            else { Start-Sleep -Milliseconds 200 }
        }
        Verdict ($null -ne $tLaneGone) 'lane detruite (socks libere + session 404)' `
            $(if ($tLaneGone) { "t=$($tLaneGone.ToString('HH:mm:ss.fff'))" } else {
                "socks_gone=$socksGone api_gone=$laneGoneApi" })

        # Le daemon doit rester vivant (pas de crash a la destruction).
        Start-Sleep -Seconds 2
        Verdict (-not $daemonProc.HasExited) 'daemon vivant apres destruction' "pid=$pidBench"

        # Suppression explicite du download (spec : lane + download) -
        # libere le marqueur `pending` ; la tache de resolution du
        # magnet encore en vol avorte silencieusement au lieu de
        # materialiser une lane fantome plus tard.
        try {
            Invoke-RestMethod -Method Delete -Uri "$apiBase/downloads/$infohashHex" `
                -Headers $H -ContentType 'application/json' `
                -Body '{"remove_data": false}' -TimeoutSec 15 | Out-Null
            Log "download $infohashHex supprime (pending libere)"
        } catch { Log "suppression download : $($_.Exception.Message)" }

        # Recreation : le meme ajout recree une lane neuve sur un
        # nouveau port SOCKS. Nouvelle lane prouvee = session?hop 200
        # + listener TCP frais + circuit DATA READY ; reprise = octets
        # de circuit en croissance apres recreation (le daemon n'a
        # pas d'autre activite BitTorrent).
        $cirBase = 0
        try {
            $cir = DApiGet '/ipv8/tunnel/circuits'
            $cirBase = [int64](@($cir.circuits) |
                Where-Object { $_.type -eq 'DATA' -and $_.goal_hops -eq $Hops } |
                ForEach-Object { [int64]$_.bytes_up + [int64]$_.bytes_down } |
                Measure-Object -Sum).Sum
        } catch {}
        Invoke-RestMethod -Method Put -Uri "$apiBase/downloads" -Headers $H `
            -ContentType 'application/json' -Body $body -TimeoutSec 15 | Out-Null
        $fileBytes0 = 0
        $deadline = (Get-Date).AddSeconds($DownloadTimeoutSec)
        while ((Get-Date) -lt $deadline -and -not $tResume) {
            try {
                $laneApi = $false
                try { DApiGet "/libtorrent/session?hop=$Hops" | Out-Null; $laneApi = $true } catch {}
                if (-not $newSocks) {
                    $tcpNow = @(Get-NetTCPConnection -OwningProcess $pidBench -State Listen `
                        -ErrorAction SilentlyContinue | ForEach-Object { $_.LocalPort })
                    $cand = @($tcpNow | Where-Object { $tcpBefore -notcontains $_ `
                        -and $laneSocks -notcontains $_ })
                    if ($laneApi -and $cand) { $newSocks = $cand }
                }
                $cir = DApiGet '/ipv8/tunnel/circuits'
                $ready = @($cir.circuits | Where-Object {
                    $_.type -eq 'DATA' -and $_.state -eq 'READY' -and $_.goal_hops -eq $Hops })
                if (-not $tNewReady -and $newSocks.Count -gt 0 -and $ready.Count -gt 0) {
                    $tNewReady = Get-Date
                    Log ("nouvelle lane READY (socks {0} + circuit DATA {1} sauts) a {2}" -f `
                        ($newSocks -join ','), $Hops, $tNewReady.ToString('HH:mm:ss.fff'))
                }
                $newBytes = [int64](@($cir.circuits) | Where-Object {
                    $_.type -eq 'DATA' -and $_.goal_hops -eq $Hops } |
                    ForEach-Object { [int64]$_.bytes_up + [int64]$_.bytes_down } |
                    Measure-Object -Sum).Sum
                $dls = @(DApiGet '/downloads' | ForEach-Object { $_.downloads })
                $fileBytes = [int64]($dls | ForEach-Object { [int64]$_.session_download } |
                    Measure-Object -Sum).Sum
                $mine0 = @($dls) | Where-Object { $_.infohash -eq $infohashHex }
                $verifiedBytes = [int64]($mine0 | ForEach-Object {
                    [double]$_.progress * [int64]$_.size } | Measure-Object -Sum).Sum
                if ($fileBytes0 -eq 0) { $fileBytes0 = $fileBytes }
                if ($tNewReady -and ($newBytes -gt $cirBase -or $fileBytes -gt $fileBytes0)) {
                    $bytesAfter = @{
                    progress = if ($mine0) { [double]$mine0[0].progress } else { $null }
                    size_selected = if ($mine0) { [int64]$mine0[0].size } else { $null }
                    verified_estimated = $verifiedBytes
                    session_download = $fileBytes
                    tunnel = $newBytes }
                    $tResume = Get-Date; $got = $fileBytes; break
                }
            } catch {}
            Start-Sleep -Seconds 1
        }
        Verdict ($newSocks.Count -gt 0) 'nouvelle lane sur nouveau port SOCKS' `
            $(if ($newSocks) { "port(s) $($newSocks -join ', ') (ancien: $($laneSocks -join ', '))" } else { 'aucun' })
        Verdict ($null -ne $tNewReady) 'nouvelle lane READY' `
            $(if ($tNewReady) { "t=$($tNewReady.ToString('HH:mm:ss.fff'))" } else { 'jamais READY' })
        Verdict ($null -ne $tResume) 'reprise via lane recreee' `
            $(if ($tResume) { "t=$($tResume.ToString('HH:mm:ss.fff'))" } else { 'pas de reprise' })

        # Fenetre post-mortem de la lane : trafic fantome de ports morts.
        if ($PostExitSec -gt 0) {
            Log "fenetre post-mortem lane ${PostExitSec}s"
            Start-Sleep -Seconds $PostExitSec
        }
        }
        & pktmon stop 2>&1 | Out-Null
        $captureStarted = $false
        $capEnd = Get-Date
        & pktmon etl2pcap $etl -o $pcap 2>&1 | Out-Null
        $pcapOk = Test-Path $pcap
        Precond $pcapOk 'capture PCAP analysable' $pcap
        if ($pcapOk) { Log "pcapng : $pcap" }
        $report = Join-Path $OutDir 'leak_report.json'

        # ---------- Attribution + analyse --------------------------
        # L'analyse de fuite n'est interpretee que si l'injection a
        # reellement eu lieu ET que le pcap est convertible : la
        # fenetre [t_failure, t_fin] est le coeur de l'oracle.
        if ($tFailure -and $pcapOk) {
        $portsFinal = @(Get-NetUDPEndpoint -OwningProcess $pidBench -ErrorAction SilentlyContinue `
            | Where-Object { $_.LocalAddress -match '^\d+\.\d+\.\d+\.\d+$' } | ForEach-Object { $_.LocalPort })
        $benchPortsFile = Join-Path $OutDir 'bench_ports.txt'
        @($portsBefore + $lanePorts + $portsFinal) | Sort-Object -Unique |
            Set-Content $benchPortsFile -Encoding ascii
        Log "ports du banc : $((Get-Content $benchPortsFile) -join ', ')"
        # Ports UDP morts : la lane n'a pas de socket UDP reelle
        # (TunnelUdpSocket virtuelle) - la liste est vide par
        # construction mais le fichier doit exister pour l'analyseur.
        $deadPortsFile = Join-Path $OutDir 'dead_ports.txt'
        [System.IO.File]::WriteAllLines($deadPortsFile, [string[]]$lanePorts)
        $allowedFile = Join-Path $OutDir 'allowed_endpoints.txt'
        Set-Content $allowedFile "127.0.0.1:$triblerPort" -Encoding ascii

        $py = 'python'
        if ($env:TRIBLER_INTEROP_PY -and (Test-Path $env:TRIBLER_INTEROP_PY)) { $py = $env:TRIBLER_INTEROP_PY }
        $analyzer = Join-Path $root 'scripts\analyze_leak_capture.py'
        $w0 = [double]([DateTimeOffset]$tFailure).ToUnixTimeMilliseconds() / 1000
        $w1 = [double]([DateTimeOffset]$capEnd).ToUnixTimeMilliseconds() / 1000
        $anArgs = @($analyzer, $pcap, '--allowed', $allowedFile,
            '--bench-ports', $benchPortsFile, '--report', $report,
            '--window-start', "$w0", '--window-end', "$w1",
            '--dead-ports', $deadPortsFile, '--dead-since', "$w0")
        if ($resolvers.Count) { $anArgs += @('--dns-resolvers', ($resolvers -join ',')) }
        if ($dhtForbidden.Count) { $anArgs += @('--dht-routers', ($dhtForbidden -join ',')) }
        $anOut = & $py @anArgs 2>&1
        $anOut | Out-File (Join-Path $OutDir 'leak_analysis.txt')
        $anOut | Select-Object -Last 30 | ForEach-Object { Log $_ }
        Verdict ($LASTEXITCODE -eq 0) 'analyse de fuite (0 paquet interdit)'
        if (Test-Path $report) {
            $rep = Get-Content $report -Raw | ConvertFrom-Json
            Verdict ($rep.window.interdit -eq 0) 'INTERDIT dans la fenetre fail-closed = 0' "n=$($rep.window.interdit)"
            Verdict ($rep.dead_ports.tx -eq 0) 'paquets sortants de ports morts = 0' "n=$($rep.dead_ports.tx)"
        }
        }

        # ---------- Invariants de fin de run -----------------------
        # Aucun download direct (hops=0) ne doit exister cote daemon
        # banc : tout le trafic BitTorrent passe par les lanes
        # anonymes, sinon le pcap temoignerait d'un clair.
        # PRECONDITION : sa violation invalide l'interpretation de la
        # capture (un download en clair rend le pcap bruite).
        $dlsEnd = @()
        try {
            $dlsEnd = @(DApiGet '/downloads' | ForEach-Object { $_.downloads })
            $directDl = @($dlsEnd | Where-Object { -not $_.anon_download })
            Precond ($directDl.Count -eq 0) 'aucun download direct sur le daemon banc' `
                "n_direct=$($directDl.Count)"
        } catch { Log "verif downloads directs : $($_.Exception.Message)" }

        # Seed dedie : si le download anonyme est termine, le fichier
        # recu doit avoir le SHA-256 de la source seedee - preuve que
        # le payload a reellement transite (pas seulement des
        # metadonnees ou du trafic de cellules).
        if ($DedicatedSeed) {
            try {
                $mineEnd = @($dlsEnd) | Where-Object { $_.infohash -eq $seedIh }
                if ($mineEnd -and [double]$mineEnd[0].progress -ge 1) {
                    $f = Get-ChildItem $dlDir -Recurse -File -ErrorAction SilentlyContinue |
                        Where-Object { $_.Length -eq $seedSize } | Select-Object -First 1
                    if ($f) {
                        $payloadHashMatch = ((Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLower() -eq $seedSha)
                        Verdict $payloadHashMatch 'payload recu == payload seede (SHA-256)' `
                            "file=$($f.Name)"
                    }
                } else {
                    Log "download dedie non termine (progress inconnue) - hash final non verifiable"
                }
            } catch { Log "hash-match seed dedie : $($_.Exception.Message)" }
        }

        @{
            run_utc   = $t0.ToUniversalTime().ToString('o')
            script    = 'sec_leak_capture.ps1'
            commit    = (git -C $root rev-parse --short HEAD)
            scenario  = $Scenario
            hops      = $Hops; magnet = $Magnet
            trigger_mode = $(if ($RequireFileBytes) { 'file_bytes_verified' }
                             else { 'max_file_tunnel' })
            seed_dedie = @{
                infohash = $seedIh
                source_sha256 = $seedSha
                size_bytes = $seedSize
                seeder = @{ proc = 'Tribler.exe'; pid = $triblerProc.Id;
                            ipv8_port = $triblerPort;
                            libtorrent_port = $triblerLtPort }
                seeding_confirmed = $seedingConfirmed
                bytes_at_failure = $bytesAtFailure
                bytes_after_rebuild = $bytesAfter
                payload_hash_match = $payloadHashMatch
            }
            pid_bench = $pidBench
            ports     = @{ before = $portsBefore; lane = $lanePorts; final = $portsFinal;
                           socks_lane = $laneSocks; socks_new_lane = $newSocks }
            t_failure = if ($tFailure) { $tFailure.ToUniversalTime().ToString('o') } else { $null }
            t_lane_gone = if ($tLaneGone) { $tLaneGone.ToUniversalTime().ToString('o') } else { $null }
            t_first_new_ready = if ($tNewReady) { $tNewReady.ToUniversalTime().ToString('o') } else { $null }
            t_resume_payload = if ($tResume) { $tResume.ToUniversalTime().ToString('o') } else { $null }
            t_last_overlay = if (Test-Path $report) {
                (Get-Content $report -Raw | ConvertFrom-Json).milestones.t_last_overlay } else { $null }
            forbidden_packets_between_failure_and_ready = if (Test-Path $report) {
                (Get-Content $report -Raw | ConvertFrom-Json).window.interdit } else { $null }
            dns_queries_in_window = if (Test-Path $report) {
                (Get-Content $report -Raw | ConvertFrom-Json).window.dns_queries } else { $null }
            milestones = if (Test-Path $report) { (Get-Content $report -Raw | ConvertFrom-Json).milestones } else { $null }
            window    = if (Test-Path $report) { (Get-Content $report -Raw | ConvertFrom-Json).window } else { $null }
            dead_ports= if (Test-Path $report) { (Get-Content $report -Raw | ConvertFrom-Json).dead_ports } else { $null }
            processes_alive_after_failure = @($daemonProc.HasExited -eq $false)
            tribler   = @{ exe = $triblerExe; port_ipv8 = $triblerPort; started_by_bench = $startedTribler }
            capture   = @{ etl = $etl; pcapng = $pcap; start = $capStart.ToString('o'); end = $capEnd.ToString('o'); post_exit_sec = $PostExitSec }
            resolvers = $resolvers
            # INVALID_PRECONDITION : la fenetre n'a pas les preuves
            # requises (trigger payload, seeder, lane prete, capture) -
            # un INTERDIT=0 ne suffit pas a valider le run.
            verdict   = if ($script:precondFails -gt 0) { 'INVALID_PRECONDITION' }
                        elseif ($script:fails -eq 0) { 'OK' }
                        else { "FAIL($($script:fails))" }
            preconditions = $script:precond
            artifacts = @{ daemon_log = $dLog; daemon_err = $dErr; analysis = 'leak_analysis.txt'; report = 'leak_report.json' }
        } | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OutDir 'manifest.json') -Encoding UTF8

        Log ""
        if ($script:precondFails -gt 0) {
            Log "SEC LEAK CAPTURE INVALID_PRECONDITION ($($script:precondFails) precondition(s))"
            exit 2
        }
        if ($script:fails -eq 0) { Log "SEC LEAK CAPTURE OK"; exit 0 }
        Log "SEC LEAK CAPTURE ECHEC ($($script:fails) verdict(s))"
        exit 1
    }

    # ---------- Demarrage capture pktmon ----------
    $etl = Join-Path $OutDir 'capture.etl'
    $pcap = Join-Path $OutDir 'capture.pcapng'
    & pktmon filter remove 2>&1 | Out-Null
    & pktmon start --capture --pkt-size 0 --file-name $etl --file-size 512 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "pktmon start a echoue" }
    $captureStarted = $true
    $capStart = Get-Date
    Log "capture pktmon demarree -> $etl"

    # ---------- Banc : download anonyme reel ----------
    # stderr en fichier vivant : permet d'injecter la panne au bon
    # moment ET conserve le diagnostic meme en cas de kill.
    $rsLog = Join-Path $OutDir 'public_download.log'
    $rsErr = Join-Path $OutDir 'public_download_err.log'
    Remove-Item -Force -ErrorAction SilentlyContinue $rsLog, $rsErr
    $rsArgs = @(
        '--bootstrap', "127.0.0.1:$triblerPort",
        '--hops', "$Hops", '--walk-seconds', "$WalkSeconds",
        '--download-timeout', "$DownloadTimeoutSec",
        '--min-bytes', "$MinBytes", '--max-circuits', "$MaxCircuits",
        '--magnet', $Magnet, '--tap'
    )
    Log "download anonyme $Hops saut(s) scenario=$Scenario (min $MinBytes octets verifies)"
    $rsProc = Start-Process -FilePath $exe -PassThru -NoNewWindow `
        -ArgumentList ($rsArgs -join ' ') `
        -RedirectStandardOutput $rsLog -RedirectStandardError $rsErr
    $pidBench = $rsProc.Id
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $totalSec = ($WalkSeconds + $MaxCircuits * ($Hops + 1) * 30 + $DownloadTimeoutSec)
    $timedOut = $false
    $tFailure = $null
    $injected = $false
    $fwRule = "OnionBitLeakCapture-$pidBench"
    $blockedIps = @()

    while ($true) {
        # Injection de panne quand le transfert a demarre.
        if (-not $injected -and $Scenario -ne 'normal' -and -not $rsProc.HasExited `
                -and (Test-Path $rsErr)) {
            $tail = [string](Get-Content $rsErr -Raw -ErrorAction SilentlyContinue)
            $mLast = [regex]::Matches($tail, 'progression : (\d+)/')
            $got = if ($mLast.Count) { [int64]$mLast[$mLast.Count-1].Groups[1].Value } else { 0 }
            if ($got -ge $FailAtBytes) {
                $tFailure = Get-Date
                if ($Scenario -eq 'kill') {
                    Log "INJECTION kill : taskkill pid=$pidBench a ${got} octets verifies"
                    Stop-Process -Id $pidBench -Force -ErrorAction SilentlyContinue
                } elseif ($Scenario -eq 'wan') {
                    $nic = Get-NetAdapter -Physical -ErrorAction SilentlyContinue |
                        Where-Object Status -eq 'Up' | Select-Object -First 1
                    if (-not $nic) { throw "aucune interface physique active pour -Scenario wan" }
                    $nicName = $nic.Name
                    Disable-NetAdapter -Name $nicName -Confirm:$false
                    Log "INJECTION wan : interface '$nicName' coupee a $got octets verifies"
                } elseif ($Scenario -eq 'kill-bootstrap') {
                    if ($startedTribler -and $triblerProc -and -not $triblerProc.HasExited) {
                        Stop-Process -Id $triblerProc.Id -Force -ErrorAction SilentlyContinue
                    }
                    Log "INJECTION kill-bootstrap : Tribler.exe tue a $got octets verifies"
                } elseif ($Scenario -eq 'block') {
                    $hops1 = [regex]::Matches($tail, 'premier saut Ipv4\((\d+\.\d+\.\d+\.\d+):\d+\)') |
                        ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique
                    foreach ($ip in $hops1) {
                        & netsh advfirewall firewall add rule "name=$fwRule" dir=out action=block protocol=UDP "remoteip=$ip" enable=yes | Out-Null
                        & netsh advfirewall firewall add rule "name=$fwRule" dir=in action=block protocol=UDP "remoteip=$ip" enable=yes | Out-Null
                    }
                    $blockedIps = $hops1
                    Log ("INJECTION block : pare-feu bloque les premiers sauts [{0}] a {1} octets verifies" -f ($hops1 -join ', '), $got)
                }
                $injected = $true
            }
        }
        if ($rsProc.HasExited) { break }
        if ($sw.Elapsed.TotalSeconds -gt $totalSec) {
            $timedOut = $true
            Stop-Process -Id $rsProc.Id -Force -ErrorAction SilentlyContinue
            break
        }
        # Scenarios recuperables : apres la fenetre morte, lever la
        # panne pour observer la reprise puis laisser le banc conclure.
        if ($Scenario -eq 'block' -and $tFailure -and `
                ((Get-Date) - $tFailure).TotalSeconds -ge $FailWindowSec -and $blockedIps.Count) {
            & netsh advfirewall firewall delete rule "name=$fwRule" | Out-Null
            Log "regle pare-feu levee apres ${FailWindowSec}s -- observation reprise"
            $blockedIps = @()
        }
        if ($Scenario -eq 'wan' -and $tFailure -and $nicName -and `
                ((Get-Date) - $tFailure).TotalSeconds -ge $FailWindowSec) {
            Enable-NetAdapter -Name $nicName -Confirm:$false
            Log "interface '$nicName' retablie apres ${FailWindowSec}s -- observation reprise"
            $nicName = $null
        }
        Start-Sleep -Seconds 1
    }
    $rsProc.WaitForExit()
    Log "processus de banc termine (code $($rsProc.ExitCode), timeout=$timedOut)"
    if ($blockedIps.Count) {
        & netsh advfirewall firewall delete rule "name=$fwRule" | Out-Null
        $blockedIps = @()
    }

    # ---------- Fenetre post-arret : trafic fantome ----------
    if ($PostExitSec -gt 0) {
        Log "fenetre post-arret ${PostExitSec}s (tout paquet WAN = suspect)"
        Start-Sleep -Seconds $PostExitSec
    }
    & pktmon stop 2>&1 | Out-Null
    $captureStarted = $false
    $capEnd = Get-Date
    Log "capture arretee"

    & pktmon etl2pcap $etl -o $pcap 2>&1 | Out-Null
    if (-not (Test-Path $pcap)) { throw "etl2pcap a echoue" }
    Log "pcapng : $pcap"

    # ---------- Verite fil : endpoints autorises depuis le TAP ----------
    $stderr = Get-Content $rsErr -Raw -ErrorAction SilentlyContinue
    $allowedFile = Join-Path $OutDir 'allowed_endpoints.txt'
    [regex]::Matches($stderr, 'TAP\[dl\]\s+\w+\s+(\d+\.\d+\.\d+\.\d+:\d+)') |
        ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique |
        Set-Content $allowedFile -Encoding ascii
    Add-Content $allowedFile "127.0.0.1:$triblerPort"
    $nAllowed = @(Get-Content $allowedFile).Count
    Log "endpoints overlay autorises : $nAllowed (TAP + bootstrap)"

    # Ports locaux du processus de banc : attribution de chaque paquet
    # capture au processus teste (pktmon ne porte pas de PID). Un
    # endpoint UDP par socket : noeud IPv8/tunnel + ecoute uTP rqbit.
    $benchPortsFile = Join-Path $OutDir 'bench_ports.txt'
    @(
        [regex]::Matches($stderr, 'noeud downloader : \S+:(\d+)') |
            ForEach-Object { $_.Groups[1].Value }
        [regex]::Matches($stderr, 'Listening on UDP \S+:(\d+)') |
            ForEach-Object { $_.Groups[1].Value }
    ) | Sort-Object -Unique | Set-Content $benchPortsFile -Encoding ascii
    Log "ports locaux du banc : $((Get-Content $benchPortsFile) -join ', ')"

    # ---------- Verdicts ----------
    $verified = [regex]::Match($stderr, 'octets_verifies=(\d+)')
    if ($Scenario -eq 'normal') {
        $okBytes = $verified.Success -and [int64]$verified.Groups[1].Value -ge $MinBytes
        Verdict (-not $timedOut -and $rsProc.ExitCode -eq 0) 'download anonyme termine' "exit=$($rsProc.ExitCode)"
        $vBytes = if ($verified.Success) { $verified.Groups[1].Value } else { 'absent' }
        Verdict $okBytes 'octets verifies >= MinBytes' $vBytes
    } else {
        # Sous panne, le banc n'est pas cense finir : l'oracle est que
        # le transfert etait REELLEMENT en cours a l'injection.
        $mLast = [regex]::Matches($stderr, 'progression : (\d+)/')
        $got = if ($mLast.Count) { [int64]$mLast[$mLast.Count-1].Groups[1].Value } else { 0 }
        Verdict ($got -ge $FailAtBytes) "transfert actif a l'injection (>= $FailAtBytes)" "dernier=$got"
        Verdict $injected "panne $Scenario injectee" $(if ($tFailure) { "t=$($tFailure.ToString('HH:mm:ss.fff'))" } else { 'jamais' })
    }
    ($stderr -split "`n" | Select-String 'route observee' | Select-Object -Last 1) |
        ForEach-Object { Log $_.Line }

    $py = 'python'
    if ($env:TRIBLER_INTEROP_PY -and (Test-Path $env:TRIBLER_INTEROP_PY)) { $py = $env:TRIBLER_INTEROP_PY }
    $analyzer = Join-Path $root 'scripts\analyze_leak_capture.py'
    $report = Join-Path $OutDir 'leak_report.json'
    $anArgs = @($analyzer, $pcap, '--allowed', $allowedFile, '--bench-ports', $benchPortsFile, '--report', $report)
    if ($tFailure) {
        # Fenetre fail-closed : de l'injection a la fin de capture.
        $w0 = [double]([DateTimeOffset]$tFailure).ToUnixTimeMilliseconds() / 1000
        $w1 = [double]([DateTimeOffset]$capEnd).ToUnixTimeMilliseconds() / 1000
        $anArgs += @('--window-start', "$w0", '--window-end', "$w1")
    }
    if ($resolvers.Count) { $anArgs += @('--dns-resolvers', ($resolvers -join ',')) }
    if ($dhtForbidden.Count) { $anArgs += @('--dht-routers', ($dhtForbidden -join ',')) }
    $anOut = & $py @anArgs 2>&1
    $anOut | Out-File (Join-Path $OutDir 'leak_analysis.txt')
    $anOut | Select-Object -Last 30 | ForEach-Object { Log $_ }
    Verdict ($LASTEXITCODE -eq 0) 'analyse de fuite (0 paquet interdit)'
    if ($tFailure -and (Test-Path $report)) {
        $rep = Get-Content $report -Raw | ConvertFrom-Json
        Verdict ($rep.window.interdit -eq 0) 'INTERDIT dans la fenetre fail-closed = 0' "n=$($rep.window.interdit)"
    }

    # ---------- Manifeste ----------
    @{
        run_utc    = $t0.ToUniversalTime().ToString('o')
        script     = 'sec_leak_capture.ps1'
        commit     = (git -C $root rev-parse --short HEAD)
        hops       = $Hops; min_bytes = $MinBytes; magnet = $Magnet
        pid_bench  = $pidBench
        scenario   = $Scenario
        t_failure  = if ($tFailure) { $tFailure.ToUniversalTime().ToString('o') } else { $null }
        fail_at_bytes = $FailAtBytes; fail_window_sec = $FailWindowSec
        tribler    = @{ exe = $triblerExe; port_ipv8 = $triblerPort; started_by_bench = $startedTribler }
        capture    = @{ etl = $etl; pcapng = $pcap; start = $capStart.ToString('o'); end = $capEnd.ToString('o'); post_exit_sec = $PostExitSec }
        resolvers  = $resolvers
        dht_routers_forbidden = $dhtForbidden
        artifacts  = @{ rust_stderr = $rsErr; analysis = 'leak_analysis.txt'; report = 'leak_report.json' }
    } | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OutDir 'manifest.json') -Encoding UTF8

    Log ""
    if ($script:fails -eq 0) { Log "SEC LEAK CAPTURE OK"; exit 0 }
    Log "SEC LEAK CAPTURE ECHEC ($($script:fails) verdict(s))"
    exit 1
}
catch {
    Log "ERREUR: $_"
    Log ($_.ScriptStackTrace)
    exit 1
}
finally {
    if ($nicName) {
        Enable-NetAdapter -Name $nicName -Confirm:$false -ErrorAction SilentlyContinue
    }
    if ($blockedIps -and $blockedIps.Count) {
        & netsh advfirewall firewall delete rule "name=$fwRule" 2>&1 | Out-Null
    }
    if ($captureStarted) { & pktmon stop 2>&1 | Out-Null }
    if ($daemonProc -and -not $daemonProc.HasExited) { Stop-Process -Id $daemonProc.Id -Force -ErrorAction SilentlyContinue }
    if ($rsProc -and -not $rsProc.HasExited) { Stop-Process -Id $rsProc.Id -Force -ErrorAction SilentlyContinue }
    if ($startedTribler -and $triblerProc) { Stop-Process -Id $triblerProc.Id -Force -ErrorAction SilentlyContinue }
    Get-Process -Name 'interop_public_download' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}
