#requires -Version 5.1
<#
.SYNOPSIS
  Interop hidden-service sens B : downloader Rust anonyme telecharge
  depuis un seeder Tribler 8.4.3 REEL.

.DESCRIPTION
  Miroir de interop_hidden_tribler_download.ps1 (sens A) avec les
  roles inverses. On valide :
    - la publication DHT d'un intro-point initiee par pyipv8
      (c'est l'INTRO POINT qui annonce : notre A1 recoit
      establish-intro de Tribler puis fait dht_announce) ;
    - le chemin downloader Rust : swarm_lookup -> peers-request ->
      create-e2e -> created-e2e (RendezvousInfo NestedPayload) ->
      link-e2e -> linked-e2e -> uTP dial via add_peer -> pieces ->
      SHA-256.
    - le role intro-point de nos noeuds face a un seeder pyipv8
      (forwarding over-socket du create-e2e vers le circuit d'intro).

  Topologie CONTROLEE (identique au sens A) :

    A1 = ancre Rust EXIT_BT + intro point probable de T api 8097 ipv8 17787
    A2 = relais Rust pur                                api 8098 ipv8 17788
    A3 = relais Rust pur                                api 8099 ipv8 17789
    D  = downloader anonyme Rust (bootstrap -> A1)      api 8095 ipv8 17785
    T  = Tribler.exe -s REEL seeder (bootstrap -> A1 via
         exitnode_cache.dat, destination contenant deja donnee.bin)

  Sequence deterministe :
    mesh -> Tribler SEEDING -> IP_SEEDER READY chez T ->
    valeur DHT relue depuis A1 -> SEULEMENT ALORS demarrage de D
    (l'intro point de T peut etre A2/A3 : la reponse peers-request
    repart alors par la socket d'exit d'A1 reencapsulee dans le
    circuit de D — chemin couvert par exit_recv_data).
#>
param(
    [int]    $Bytes      = 6291456,
    [int]    $Hops       = 1,
    [int]    $TimeoutMin = 45,
    [string] $OutDir     = ("target\interop-hidden-seed-" + (Get-Date -Format 'yyyyMMdd-HHmmss')),
    # -DhtOnly : arreter apres le verdict DHT (publication de T
    # relue depuis A1) — pas de downloader.
    [switch] $DhtOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root   = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$out    = Join-Path $root $OutDir
$daemon = Join-Path $root 'target\debug\tribler-daemon.exe'
$mk     = Join-Path $root 'target\debug\examples\mk_torrent.exe'
$rep    = Join-Path $out 'report'
$triblerExe = if ($env:TRIBLER_EXE) { $env:TRIBLER_EXE } else { 'C:\Program Files (x86)\Tribler\Tribler.exe' }
$tstate = Join-Path $out 'tribler-state'
$confDir = Join-Path $tstate '8.0'
New-Item -ItemType Directory -Force -Path $out, $rep, $confDir | Out-Null

$P_D  = @{ Api = 8095; Ipv8 = 17785; Dir = Join-Path $out 'downloader'; Name = 'D' }
$P_A  = @{ Api = 8097; Ipv8 = 17787; Dir = Join-Path $out 'anchor';    Name = 'A' }
$P_A2 = @{ Api = 8098; Ipv8 = 17788; Dir = Join-Path $out 'relay2';    Name = 'A2' }
$P_A3 = @{ Api = 8099; Ipv8 = 17789; Dir = Join-Path $out 'relay3';    Name = 'A3' }
# Relais supplementaires a hops>=3 : IP_SEEDER / RP_DOWNLOADER font
# hops+1 sauts — il faut `hops` relais libres distincts en plus du
# dernier saut impose (cf. interop_hidden_tribler_download.ps1).
$relays = @($P_A2, $P_A3)
if ($Hops -ge 3) {
    $relays += @(
        @{ Api = 8100; Ipv8 = 17790; Dir = Join-Path $out 'relay4'; Name = 'A4' },
        @{ Api = 8101; Ipv8 = 17791; Dir = Join-Path $out 'relay5'; Name = 'A5' }
    )
}
$procs = @{}
$verdicts = New-Object System.Collections.Generic.List[string]

function Log([string]$m) { Write-Host ('[{0:HH:mm:ss}] {1}' -f (Get-Date), $m) }
function Verdict([bool]$ok, [string]$label, [string]$detail = '') {
    $tag = if ($ok) { 'OK  ' } else { 'FAIL' }
    $line = ('{0} {1} {2}' -f $tag, $label, $detail).TrimEnd()
    $script:verdicts.Add($line); Log $line
}
function ApiKey([string]$dir) {
    $cfg = Join-Path $dir 'configuration.json'
    if (-not (Test-Path $cfg)) { return $null }
    try { return (Get-Content $cfg -Raw | ConvertFrom-Json).api.key } catch { return $null }
}
function Wait-ApiKey([string]$dir, [int]$sec = 60) {
    $dl = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $dl) {
        $k = ApiKey $dir; if ($k) { return $k }
        Start-Sleep -Milliseconds 500
    }
    throw "cle API absente dans $dir"
}
function Api([string]$method, [int]$port, [string]$path, $key, $body = $null, [int]$sec = 15) {
    $uri = "http://127.0.0.1:$port/api$path"
    $h = @{ 'X-Api-Key' = $key }
    if ($null -ne $body) {
        return Invoke-RestMethod -Method $method -Uri $uri -Headers $h -TimeoutSec $sec `
            -ContentType 'application/json' -Body ($body | ConvertTo-Json -Compress)
    }
    Invoke-RestMethod -Method $method -Uri $uri -Headers $h -TimeoutSec $sec
}
function Wait-ApiUp([int]$port, $key, [int]$sec = 60) {
    $dl = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $dl) {
        try { Api 'GET' $port '/statistics/tribler' $key $null 5 | Out-Null; return } catch {}
        Start-Sleep -Milliseconds 700
    }
    throw "API 127.0.0.1:$port injoignable"
}
function Start-Daemon($p, [string[]]$boot) {
    # Pas de --ipv8-port : ce flag ecrase listen_addr en 0.0.0.0. Le port
    # et l'interface (127.0.0.1 — maillage ferme) viennent du fichier
    # ipv8.interfaces ecrit avant le lancement.
    $argList = @('--state-dir', ('"{0}"' -f $p.Dir), '--listen', "127.0.0.1:$($p.Api)",
              '--no-tray')
    foreach ($b in $boot) { $argList += @('--bootstrap', $b) }
    $psi = New-Object System.Diagnostics.ProcessStartInfo($daemon)
    $psi.Arguments = $argList -join ' '
    $psi.UseShellExecute = $false
    $psi.EnvironmentVariables['RUST_LOG'] = 'info,tribler_ipv8=debug,tribler_tunnel=debug,tribler_core=debug,librqbit_utp=debug,librqbit=debug'
    $proc = [System.Diagnostics.Process]::Start($psi)
    $procs[$p.Name] = $proc
    Log ("{0} demarre pid={1} bootstrap=[{2}]" -f $p.Name, $proc.Id, ($boot -join ', '))
}
function Assert-ConfigOk($p) {
    $logF = Get-ChildItem (Join-Path $p.Dir 'logs') -Filter '*.log' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime | Select-Object -Last 1
    if ($logF -and (Select-String -Path $logF.FullName -Pattern 'corrompu' -Quiet)) {
        throw "configuration.json de $($p.Name) rejetee par le daemon"
    }
}
function Get-FreeTcpPort {
    $l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $l.Start(); $p = $l.LocalEndpoint.Port; $l.Stop(); return $p
}
function Get-FreeUdpPort {
    $u = [System.Net.Sockets.UdpClient]::new([System.Net.IPAddress]::Loopback, 0)
    $p = $u.Client.LocalEndPoint.Port; $u.Close(); return $p
}
function Circuits([int]$port, $key, [string]$ctype = $null, [string]$state = 'READY') {
    try { $r = Api 'GET' $port '/ipv8/tunnel/circuits' $key } catch { return @() }
    $c = @($r.circuits)
    if ($ctype) { $c = @($c | Where-Object { $_.type -eq $ctype }) }
    if ($state) { $c = @($c | Where-Object { $_.state -eq $state }) }
    return $c
}
function Download-State([int]$port, $key, [string]$ih) {
    $d = @((Api 'GET' $port '/downloads' $key).downloads) | Where-Object { $_.infohash -eq $ih }
    if ($d) { return $d[0] } else { return $null }
}
function TApi([string]$method, [string]$path, [int]$sec = 15) {
    $uri = "http://127.0.0.1:$($script:TApiPort)/api$path"
    Invoke-RestMethod -Method $method -Uri $uri -Headers @{ 'X-Api-Key' = $script:TApiKey } -TimeoutSec $sec
}
function TDownload-State([string]$ih) {
    try { $d = @((TApi 'GET' '/downloads' 15).downloads) | Where-Object { $_.infohash -eq $ih } } catch { return $null }
    if ($d) { return $d[0] } else { return $null }
}
function TCircuits([string]$ctype = $null) {
    try { $c = @((TApi 'GET' '/ipv8/tunnel/circuits' 15).circuits) } catch { return @() }
    if ($ctype) { $c = @($c | Where-Object { $_.type -eq $ctype }) }
    return $c
}
function LogGrep([string]$file, [string]$pattern) {
    if (-not (Test-Path $file)) { return @() }
    return @(Select-String -Path $file -Pattern $pattern -AllMatches | ForEach-Object { $_.Line })
}

# ---------- Phase 0 : contenu + torrent ----------
Log '=== Phase 0 : contenu de test ==='
if (-not (Test-Path $daemon)) { throw 'tribler-daemon.exe absent - cargo build -p tribler-daemon' }
if (-not (Test-Path $mk)) { throw 'mk_torrent.exe absent - cargo build -p tribler-bittorrent --example mk_torrent' }
if (-not (Test-Path $triblerExe)) { throw "Tribler.exe absent : $triblerExe" }
$content = Join-Path $out 'content'
foreach ($p in (@($P_D, $P_A) + $relays)) {
    if (Test-Path $p.Dir) { Remove-Item -Recurse -Force $p.Dir }
}
# tseed = repertoire "seeder" de Tribler : contient le fichier complet
# AVANT le PUT -> hashcheck -> SEEDING direct (join_swarm seeding).
$tseed = Join-Path $out 'tribler-seed'
$dld   = Join-Path $out 'rust-dl'
foreach ($d in @($content, $tstate, $tseed, $dld)) {
    if (Test-Path $d) { Remove-Item -Recurse -Force $d }
}
New-Item -ItemType Directory -Force -Path $confDir, $tseed, $dld | Out-Null

$nonce = Get-Random -Minimum 1 -Maximum ([int]::MaxValue)
$mkOut = & $mk $content $Bytes $nonce
$mkOut | Tee-Object (Join-Path $rep 'mk_torrent.txt') | Out-Null
$torrent = Join-Path $content 'test.torrent'
$ih = (($mkOut | Select-String 'infohash=([0-9a-f]+)').Matches.Groups[1].Value)
if (-not $ih) { throw 'infohash non obtenu' }
Log "infohash=$ih"
$lookup = ([System.BitConverter]::ToString(
    [System.Security.Cryptography.SHA1]::Create().ComputeHash(
        [System.Text.Encoding]::ASCII.GetBytes('tribler anonymous download' + $ih)))
).Replace('-', '').ToLower()
Log "lookup_dht=$lookup"
$srcHash = (Get-FileHash -Algorithm SHA256 (Join-Path $content 'donnee.bin')).Hash.ToLower()
Log "sha256 source=$srcHash"
# Le contenu complet est pose dans la destination du seeder Tribler.
Copy-Item (Join-Path $content 'donnee.bin') (Join-Path $tseed 'donnee.bin') -Force

# ---------- Config Tribler isolee (seeder) ----------
$tApiPort = Get-FreeTcpPort
$tIpv8 = Get-FreeUdpPort
while ($tIpv8 -eq $tApiPort) { $tIpv8 = Get-FreeUdpPort }
$script:TApiPort = $tApiPort
$script:TApiKey = "interop$nonce"
$conf = @{
    api = @{
        http_enabled = $true
        http_port = $tApiPort
        http_host = '127.0.0.1'
        https_enabled = $false
        key = $script:TApiKey
    }
    ipv8 = @{
        logger = @{ level = 'DEBUG' }
        interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $tIpv8 } )
        walker_interval = 5.0
        overlays = @(
            @{
                class = 'DiscoveryCommunity'
                key = 'anonymous id'
                walkers = @(
                    @{ strategy = 'RandomWalk'; peers = 20; init = @{ timeout = 3.0 } }
                    @{ strategy = 'RandomChurn'; peers = -1; init = @{ sample_size = 8; ping_interval = 10.0; inactive_time = 27.5; drop_time = 57.5 } }
                    @{ strategy = 'PeriodicSimilarity'; peers = -1; init = @{} }
                )
                bootstrappers = @(
                    @{ class = 'DispersyBootstrapper'; init = @{
                        ip_addresses = ,@('127.0.0.1', 17787)
                        dns_addresses = @()
                        bootstrap_timeout = 30.0
                    } }
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
        download_defaults = @{ saveas = $tseed }
    }
    tunnel_community = @{ enabled = $true; min_circuits = 3; max_circuits = 8 }
    dht_discovery = @{ enabled = $true }
    content_discovery = @{ enabled = $false }
    recommender = @{ enabled = $false }
    rendezvous = @{ enabled = $false }
    rss = @{ enabled = $false }
    torrent_checker = @{ enabled = $false }
    versioning = @{ enabled = $false }
    watch_folder = @{ enabled = $false }
    statistics = $false
}
[System.IO.File]::WriteAllText((Join-Path $confDir 'configuration.json'),
    ($conf | ConvertTo-Json -Depth 10), [System.Text.UTF8Encoding]::new($false))

$deadline = (Get-Date).AddMinutes($TimeoutMin)
try {
    # ---------- Phase 1 : maillage A1(EXIT_BT)/A2/A3 ----------
    Log '=== Phase 1 : ancre A1 (EXIT_BT) + relais A2/A3 ==='
    foreach ($p in (@($P_A, $P_D) + $relays)) {
        New-Item -ItemType Directory -Force -Path $p.Dir | Out-Null
    }
    [System.IO.File]::WriteAllText((Join-Path $P_A.Dir 'configuration.json'),
        (@{ tunnel_community = @{ exitnode_enabled = $true };
            ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($P_A2.Ipv8)") };
                      interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $P_A.Ipv8 } );
                      estimated_wan = "127.0.0.1:$($P_A.Ipv8)" } } |
            ConvertTo-Json -Compress -Depth 5))
    Start-Daemon $P_A @()
    $kA = Wait-ApiKey $P_A.Dir; Wait-ApiUp $P_A.Api $kA; Assert-ConfigOk $P_A

    foreach ($p in $relays) {
        [System.IO.File]::WriteAllText((Join-Path $p.Dir 'configuration.json'),
            (@{ ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($P_A.Ipv8)") };
                          interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $p.Ipv8 } );
                          estimated_wan = "127.0.0.1:$($p.Ipv8)" } } |
                ConvertTo-Json -Compress -Depth 5))
        Start-Daemon $p @()
        $k = Wait-ApiKey $p.Dir; Wait-ApiUp $p.Api $k; Assert-ConfigOk $p
    }

    # ---------- Phase 2 : Tribler seeder ----------
    Log "=== Phase 2 : Tribler.exe -s SEEDER (etat vierge : $tstate, ipv8 :$tIpv8, api :$tApiPort) ==="
    $snap = [byte[]]@(0x01) + [System.Net.IPAddress]::Parse('127.0.0.1').GetAddressBytes() +
            [byte[]]@([byte]($P_A.Ipv8 -shr 8), [byte]($P_A.Ipv8 -band 0xFF))
    [System.IO.File]::WriteAllBytes((Join-Path $confDir 'exitnode_cache.dat'), $snap)
    $env:TSTATEDIR = $tstate
    $env:CORE_API_PORT = "$tApiPort"
    $env:CORE_API_KEY = $script:TApiKey
    $tProc = Start-Process -FilePath $triblerExe -PassThru -NoNewWindow `
        -ArgumentList '-s','--log-level','DEBUG' `
        -RedirectStandardOutput (Join-Path $out 'tribler_stdout.log') `
        -RedirectStandardError (Join-Path $out 'tribler_stderr.log')
    $procs['T'] = $tProc
    Log "T demarre pid=$($tProc.Id)"

    $manifest = [ordered]@{
        run_utc    = [datetime]::UtcNow.ToString('o')
        script     = 'interop_hidden_tribler_seed.ps1'
        mode       = $(if ($DhtOnly) { 'DhtOnly' } else { 'Full' })
        sens       = 'B (Tribler seeder -> Rust downloader)'
        daemon     = $daemon
        tribler    = $triblerExe
        infohash   = $ih
        lookup_dht = $lookup
        sha256_src = $srcHash
        hops       = $Hops
        outdir     = $out
        nodes      = @(
            @{ name = 'T';  api = $tApiPort;   ipv8 = $tIpv8;       dir = $tstate;    pid = $tProc.Id;          role = 'seeder anonyme Tribler 8.4.3 reel' },
            @{ name = 'A1'; api = $P_A.Api;    ipv8 = $P_A.Ipv8;    dir = $P_A.Dir;   pid = $procs['A'].Id;     role = 'EXIT_BT + intro point probable' }
        ) + @($relays | ForEach-Object {
            @{ name = $_.Name; api = $_.Api; ipv8 = $_.Ipv8; dir = $_.Dir; pid = $procs[$_.Name].Id; role = 'relais' }
        }) + @(
            @{ name = 'D';  api = $P_D.Api;    ipv8 = $P_D.Ipv8;    dir = $P_D.Dir;   pid = $null;              role = 'downloader anonyme Rust (demarre apres gate DHT)' }
        )
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'manifest.json') -Encoding UTF8
    Log "manifeste ecrit : $(Join-Path $out 'manifest.json')"

    $tApiUp = $false; $tWait = (Get-Date).AddSeconds(180)
    while ((Get-Date) -lt $tWait -and -not $tApiUp) {
        try { TApi 'GET' '/ipv8/overlays' 5 | Out-Null; $tApiUp = $true } catch { Start-Sleep -Seconds 2 }
    }
    Verdict $tApiUp 'Tribler : API REST en ligne'
    if (-not $tApiUp) { throw 'API Tribler jamais en ligne - voir tribler_stderr.log' }

    # Maillage controle : >= 3 pairs tunnel dont >= 1 exit (A1).
    $tPeers = $false; $nP = 0; $nTunnel = 0; $exits = 0; $apiErr = $null; $pWait = (Get-Date).AddMinutes(6)
    while ((Get-Date) -lt $pWait -and -not $tPeers) {
        try {
            $ov = (TApi 'GET' '/ipv8/overlays' 10).overlays
            $nP = ($ov | ForEach-Object { @($_.peers).Count } | Measure-Object -Sum).Sum
            $nTunnel = @($ov | Where-Object { $_.overlay_name -match 'Tunnel' } | ForEach-Object { @($_.peers).Count } | Measure-Object -Sum).Sum
            $exits = @((TApi 'GET' '/ipv8/tunnel/peers' 10).peers | Where-Object { @($_.flags) | Where-Object { $_ -band 6 } }).Count
            Log ("Tribler overlays : {0} | peers_total={1} tunnel_peers={2} exits={3}" -f @($ov).Count, $nP, $nTunnel, $exits)
            if ($nTunnel -ge 3 -and $exits -ge 1) { $tPeers = $true }
        } catch {
            $em = $_.Exception.Message
            if ($em -ne $apiErr) { $apiErr = $em; Log "TApi : $em" }
            Start-Sleep -Seconds 3
        }
        if (-not $tPeers) { Start-Sleep -Seconds 5 }
    }
    Verdict ($nP -gt 0) 'Tribler : bootstrap maillage controle' "peers=$nP tunnel=$nTunnel exits=$exits"

    # ---------- Phase 3 : PUT torrent chez Tribler (SEEDING direct) ----------
    Log '=== Phase 3 : PUT torrent chez Tribler (destination = contenu complet) ==='
    $dest = [System.Uri]::EscapeDataString($tseed)
    $putUri = "http://127.0.0.1:$tApiPort/api/downloads?anon_hops=$Hops&destination=$dest"
    $tAdd = $null; $putEnd = (Get-Date).AddMinutes(5)
    while (-not $tAdd -and (Get-Date) -lt $putEnd -and (Get-Date) -lt $deadline) {
        try {
            $tAdd = Invoke-RestMethod -Method PUT -Uri $putUri -Headers @{ 'X-Api-Key' = $script:TApiKey } `
                -ContentType 'applications/x-bittorrent' -InFile $torrent -TimeoutSec 30
            if ($tAdd.PSObject.Properties['error'] -and $tAdd.error) {
                Log "PUT reponse : $($tAdd.error | ConvertTo-Json -Compress -Depth 4)"; $tAdd = $null; Start-Sleep -Seconds 10
            }
        } catch { Log "PUT : $($_.Exception.Message)"; Start-Sleep -Seconds 10 }
    }
    $addOk = ($tAdd -and $tAdd.started -eq $true -and $tAdd.infohash -eq $ih)
    Verdict $addOk 'Tribler : download (seed) accepte' "started=$($tAdd.started) infohash=$($tAdd.infohash)"
    if (-not $addOk) { throw 'ajout download refuse' }

    # Hashcheck puis SEEDING -> join_swarm(seeding) + create_introduction_point.
    $seedOk = $false; $sWait = (Get-Date).AddMinutes(3)
    while ((Get-Date) -lt $sWait) {
        $dt = TDownload-State $ih
        if ($dt) {
            Log ("T : status={0} progress={1:P1} ul={2}B" -f $dt.status, $dt.progress, $dt.all_time_upload)
            if ($dt.status -eq 'SEEDING' -or $dt.progress -ge 1.0) { $seedOk = $true; break }
        }
        Start-Sleep -Seconds 3
    }
    Verdict $seedOk 'Tribler : SEEDING (hidden swarm joined)'
    if (-not $seedOk) { throw 'Tribler jamais SEEDING - pas de join_swarm' }

    # IP_SEEDER : le(s) circuit(s) d'introduction de T vers A?.
    $ipOk = $false; $nIp = 0; $ipWait = (Get-Date).AddMinutes(6)
    while ((Get-Date) -lt $ipWait) {
        $nIp = @(TCircuits 'IP_SEEDER' | Where-Object { $_.state -eq 'READY' }).Count
        if ($nIp -ge 1) { $ipOk = $true; break }
        Start-Sleep -Seconds 5
    }
    Verdict $ipOk 'Tribler : circuit(s) IP_SEEDER READY' "n=$nIp"

    # ---------- Gate DHT : la valeur annoncee par l'intro point est relue ----------
    # on_establish_intro -> l'INTRO POINT (un de nos noeuds) fait le
    # dht_announce ; la valeur doit etre relisible via find_values
    # depuis un AUTRE noeud avant de lancer D.
    Log 'attente annonce DHT du point d introduction de T (sonde A1)...'
    $dhtOk = $false; $dhtWait = (Get-Date).AddMinutes(12)
    if ($dhtWait -gt $deadline) { $dhtWait = $deadline }
    while ((Get-Date) -lt $dhtWait) {
        try {
            $vA = Api 'GET' $P_A.Api "/ipv8/dht/values/$lookup" $kA $null 30
            Log ("DHT {0} : A1 req={1} val={2} n={3} t={4:N1}s" -f
                $lookup.Substring(0, 12), $vA.debug.requests, $vA.debug.responses_with_values,
                @($vA.values).Count, $vA.debug.time)
            if (@($vA.values).Count -gt 0) { $dhtOk = $true; break }
        } catch { Log "DHT lookup : $($_.Exception.Message)" }
        Start-Sleep -Seconds 15
    }
    Verdict $dhtOk 'DHT : intro-point de T stocke et relu'
    if (-not $dhtOk) { throw 'annonce DHT de T non propagee - D non lance' }

    if ($DhtOnly) {
        Log '=== -DhtOnly : collecte DHT terminee, phase downloader ignoree ==='
        return
    }

    # ---------- Phase 4 : downloader Rust ----------
    Log '=== Phase 4 : downloader anonyme Rust D ==='
    [System.IO.File]::WriteAllText((Join-Path $P_D.Dir 'configuration.json'),
        (@{ ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($P_A.Ipv8)") };
                      interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $P_D.Ipv8 } );
                      estimated_wan = "127.0.0.1:$($P_D.Ipv8)" } } |
            ConvertTo-Json -Compress -Depth 5))
    Start-Daemon $P_D @()
    $kD = Wait-ApiKey $P_D.Dir; Wait-ApiUp $P_D.Api $kD; Assert-ConfigOk $P_D

    # Le manifeste est mis a jour avec le pid de D.
    $manifest.nodes = @($manifest.nodes | ForEach-Object {
        if ($_.name -eq 'D') { $_.pid = $procs['D'].Id }; $_ })
    $manifest | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'manifest.json') -Encoding UTF8

    Api 'PUT' $P_D.Api '/downloads' $kD @{ torrent = $torrent; destination = $dld;
        anon_hops = $Hops; safe_seeding = $true } | Out-Null
    Verdict $true 'D : download anonyme accepte' "infohash=$ih"

    # ---------- Phase 5 : attente + collecte de preuves ----------
    $dLog = Join-Path $P_D.Dir 'logs\tribler.log'
    $aLog = Join-Path $P_A.Dir 'logs\tribler.log'
    $seen = @{ dht = $false; presp = $false; created = $false; linke = $false; linked = $false; lane = $false; utp = $false; bytes = $false }
    $dt0 = TDownload-State $ih
    $tulBefore = if ($dt0) { [int64]$dt0.all_time_upload } else { 0 }
    Log "T : upload baseline=$tulBefore octets"

    $done = $false
    while ((Get-Date) -lt $deadline -and -not $done) {
        Start-Sleep -Seconds 10
        # -- D : progression + circuits
        $dd = Download-State $P_D.Api $kD $ih
        $dc = Circuits $P_D.Api $kD $null $null
        $nRpD = @($dc | Where-Object { $_.type -eq 'RP_DOWNLOADER' }).Count
        if ($dd) {
            $dlb = [int64]$dd.all_time_download
            Log ("D : progress={0:P1} dl={1}B peers={2} st={3} | circuits RPDL={4} total={5}" -f
                $dd.progress, $dlb, $dd.num_peers, $dd.status, $nRpD, @($dc).Count)
            if ($dlb -gt 0 -and -not $seen.bytes) {
                $seen.bytes = $true
                Verdict $true 'D : octets recus via le hidden service' "dl=$dlb peers=$($dd.num_peers)"
            }
            if ($dd.progress -ge 1.0) { $done = $true }
        }
        # -- Bornes protocolaires dans le log de D
        if (-not $seen.dht) {
            $m = @(LogGrep $dLog 'dht_lookup du swarm : valeur')
            if ($m.Count -gt 0) { $seen.dht = $true; Verdict $true 'D : lookup DHT a trouve l intro-point' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
        if (-not $seen.presp) {
            $m = @(LogGrep $dLog 'peers-response recu')
            if ($m.Count -gt 0) { $seen.presp = $true; Verdict $true 'D : peers-response recu' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
        if (-not $seen.created) {
            $m = @(LogGrep $dLog 'created-e2e valide')
            if ($m.Count -gt 0) { $seen.created = $true; Verdict $true 'D : created-e2e valide (Rust<-Tribler)' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
        if (-not $seen.linke) {
            $m = @(LogGrep $dLog 'link-e2e envoye')
            if ($m.Count -gt 0) { $seen.linke = $true; Verdict $true 'D : link-e2e envoye au RP' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
        if (-not $seen.linked) {
            $m = @(LogGrep $dLog 'linked-e2e : circuit e2e pret')
            if ($m.Count -gt 0) { $seen.linked = $true; Verdict $true 'D : linked-e2e (e2e etabli)' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
        if (-not $seen.lane) {
            $m = @(LogGrep $dLog 'e2e listener : lane branchee')
            if ($m.Count -gt 0) { $seen.lane = $true; Verdict $true 'D : lane e2e branchee (add_peer)' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
        if (-not $seen.utp) {
            $m = @(LogGrep $dLog 'utp_stream\{')
            if ($m.Count -gt 0) { $seen.utp = $true; Verdict $true 'D : stream uTP actif sur lane e2e' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
    }

    # ---------- Phase 6 : integrite ----------
    $ddE = Download-State $P_D.Api $kD $ih
    $dtE = TDownload-State $ih
    $tulNow = if ($dtE) { [int64]$dtE.all_time_upload } else { 0 }
    $tulDelta = $tulNow - $tulBefore
    Verdict ($tulDelta -gt 0) 'T upload (delta all_time_upload)' "delta=$tulDelta"
    if ($ddE) {
        Verdict ($ddE.progress -ge 1.0) 'D : telechargement termine (pieces verifiees)' ("progress={0:P1} dl={1}" -f $ddE.progress, $ddE.all_time_download)
    }
    if ($done) {
        Start-Sleep -Seconds 3
        $f = Get-ChildItem -Recurse -Filter 'donnee.bin' $dld -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($f) {
            $h = (Get-FileHash -Algorithm SHA256 $f.FullName).Hash.ToLower()
            Verdict ($h -eq $srcHash) 'integrite : SHA256 du fichier recu' "got=$h want=$srcHash"
        } else { Verdict $false 'integrite : SHA256 du fichier recu' 'donnee.bin introuvable dans la destination de D' }
    }
}
finally {
    foreach ($p in (@($P_D, $P_A) + $relays)) {
        try {
            $k = ApiKey $p.Dir
            if ($k) {
                foreach ($ep in @('/downloads','/ipv8/tunnel/circuits','/ipv8/tunnel/relays','/ipv8/tunnel/exits','/ipv8/tunnel/swarms','/ipv8/tunnel/peers')) {
                    $name = ($p.Name + ($ep -replace '/','_') + '.json')
                    try { Api 'GET' $p.Api $ep $k $null 5 | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $rep $name) -Encoding UTF8 } catch {}
                }
            }
        } catch {}
        if ($procs.ContainsKey($p.Name) -and -not $procs[$p.Name].HasExited) {
            try { $procs[$p.Name].Kill(); $procs[$p.Name].WaitForExit() } catch {}
        }
        $log = Join-Path $p.Dir 'logs\tribler.log'
        if (Test-Path $log) { Copy-Item $log (Join-Path $rep ("tribler_{0}.log" -f $p.Name)) -Force }
    }
    # Instantanes Tribler avant extinction.
    try { TCircuits | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $rep 'T_circuits.json') -Encoding UTF8 } catch {}
    try { TApi 'GET' "/downloads?infohash=$ih" 10 | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $rep 'T_download_detail.json') -Encoding UTF8 } catch {}
    try { TApi 'GET' '/ipv8/overlays' 10 | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $rep 'T_overlays.json') -Encoding UTF8 } catch {}
    if ($procs.ContainsKey('T') -and -not $procs['T'].HasExited) {
        try { $procs['T'].Kill(); $procs['T'].WaitForExit() } catch {}
    }
    foreach ($f in @('tribler_stdout.log','tribler_stderr.log')) {
        $src = Join-Path $out $f
        if (Test-Path $src) { Copy-Item $src (Join-Path $rep $f) -Force }
    }
    Get-ChildItem -Recurse -Filter '*.log' $tstate -ErrorAction SilentlyContinue |
        ForEach-Object { Copy-Item $_.FullName (Join-Path $rep ('T_' + $_.Name)) -Force }
    Remove-Item Env:TSTATEDIR, Env:CORE_API_PORT, Env:CORE_API_KEY -ErrorAction SilentlyContinue
    Write-Host ''
    Write-Host '================ VERDICTS ================'
    $verdicts | ForEach-Object { Write-Host $_ }
    Write-Host "rapport : $rep"
}
