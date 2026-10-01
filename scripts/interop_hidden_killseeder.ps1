# interop_hidden_killseeder.ps1 — Resilience interop : kill du seeder
# anonyme en plein transfert, drain borne, fenetre morte stricte,
# redemarrage puis reprise jusqu'au SHA-256.
#
#   -Sens A : seeder = tribler-daemon Rust, downloader = Tribler 8.4.3.
#             Mesure : Tribler re-decouvre le seeder via
#             `do_peer_discovery`/`swarm_lookup_interval` (30 s) apres
#             que le seeder a re-public sur la DHT via son nouvel IP.
#   -Sens B : seeder = Tribler 8.4.3, downloader = tribler-daemon Rust.
#             Mesure : notre `do_peer_discovery` re-looke la DHT, cree
#             un nouvel e2e et reprend le telechargement.
#
# Preuve par frontiere :
#   S SEEDING -> IP_SEEDER -> DHT -> download demarre -> dl >= KillAt ->
#   KILL -> drain borne (octets in-flight s'arretent) -> fenetre morte
#   stricte (0 octet pendant DeadSec) -> restart -> SEEDING + IP_SEEDER
#   restaurés -> reprise (dl repart) -> 100 % -> SHA-256 identique.
param(
    [ValidateSet('A','B')]
    [string] $Sens         = 'A',
    # Cible du kill : 'seeder' (processus seeder entier, restart +
    # reprise), 'intro' (le noeud hebergeant le point d'introduction —
    # resolu via verified_hops[-1] ; le seeder reste vivant et doit
    # reconstruire un intro point AILLEURS puis re-annoncer sur la DHT)
    # ou 'anchor' (A1 bootstrap+EXIT_BT : meme flux qu'intro plus un
    # verdict de decouverte post-kill sans le noeud d'amorcage).
    [ValidateSet('seeder','intro','anchor')]
    [string] $KillTarget   = 'seeder',
    # 24 Mo : assez pour que le kill tombe en plein transfert (~370 ko/s
    # observes sur le mesh loopback -> ~70 s de transfert nominal).
    [int]    $Bytes        = 25165824,
    [int]    $Hops         = 1,
    # Seuil du kill : le downloader doit avoir recu au moins ca pour
    # prouver que le data plane vivait avant le crash.
    [int]    $KillAtBytes  = 3145728,
    # Fenetre morte stricte : zero octet supplementaire pendant DeadSec.
    [int]    $DeadSec      = 30,
    # Borne du drain : temps max apres le kill pendant lequel des octets
    # in-flight peuvent encore arriver.
    [int]    $DrainSec     = 20,
    # Temps max pour que le download reparte apres le retour du seeder.
    [int]    $ResumeSec    = 360,
    [int]    $TimeoutMin   = 50,
    [string] $OutDir       = ("target\interop-killseed-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
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

# Sens A : le daemon seede -> port 8095 ; sens B : c'est le downloader.
$P_N  = @{ Api = 8095; Ipv8 = 17785; Dir = Join-Path $out $(if ($Sens -eq 'A') {'seed'} else {'downloader'}); Name = $(if ($Sens -eq 'A') {'S'} else {'D'}) }
$P_A  = @{ Api = 8097; Ipv8 = 17787; Dir = Join-Path $out 'anchor';  Name = 'A' }
$P_A2 = @{ Api = 8098; Ipv8 = 17788; Dir = Join-Path $out 'relay2';  Name = 'A2' }
$P_A3 = @{ Api = 8099; Ipv8 = 17789; Dir = Join-Path $out 'relay3';  Name = 'A3' }
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
function Wait-ApiUp([int]$port, $key, [int]$sec = 90) {
    $dl = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $dl) {
        try { Api 'GET' $port '/statistics/tribler' $key $null 5 | Out-Null; return } catch {}
        Start-Sleep -Milliseconds 700
    }
    throw "API 127.0.0.1:$port injoignable"
}
function Start-Daemon($p, [string[]]$boot) {
    $argList = @('--state-dir', ('"{0}"' -f $p.Dir), '--listen', "127.0.0.1:$($p.Api)", '--no-tray')
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
    try { $d = @((Api 'GET' $port '/downloads' $key).downloads) | Where-Object { $_.infohash -eq $ih } } catch { return $null }
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
# Parse les valeurs DHT du swarm : DHTIntroPointPayload
# ["ip_address","I","varlenH","varlenH"] = addr(type+donnees) +
# last_seen u32 + varlenH intro_pk + varlenH seeder_pk. Retourne
# @{IntroMid; SeederPkHex} par valeur (IntroMid = sha1 de la cle
# prefixee, comparable aux mids de verified_hops).
function Dht-IntroValues([int]$port, $key, [string]$lookup, $sha1) {
    $out = @()
    try { $v = Api 'GET' $port "/ipv8/dht/values/$lookup" $key $null 30 } catch { return @() }
    foreach ($val in @($v.values)) {
        $h = $val.value; if (-not $h) { continue }
        $b = [byte[]]::new($h.Length / 2)
        for ($i = 0; $i -lt $b.Length; $i++) { $b[$i] = [Convert]::ToByte($h.Substring($i * 2, 2), 16) }
        try {
            $o = 0
            switch ($b[$o]) {
                1 { $o += 7 }      # IPv4 : type + 4 + port(2)
                3 { $o += 19 }     # IPv6 : type + 16 + port(2)
                2 { $l = ([int]$b[$o+1] -shl 8) -bor [int]$b[$o+2]; $o += 3 + $l + 2 }
                default { continue }
            }
            $o += 4               # last_seen
            if ($o + 2 -gt $b.Length) { continue }
            $li = ([int]$b[$o] -shl 8) -bor [int]$b[$o+1]; $o += 2
            if ($o + $li -gt $b.Length) { continue }
            $introPk = $b[$o..($o+$li-1)]; $o += $li
            if ($o + 2 -gt $b.Length) { continue }
            $ls = ([int]$b[$o] -shl 8) -bor [int]$b[$o+1]; $o += 2
            if ($o + $ls -gt $b.Length) { continue }
            $seederPk = $b[$o..($o+$ls-1)]
            $fullIntro = [byte[]]@(0x4c,0x69,0x62,0x4e,0x61,0x43,0x4c,0x50,0x4b,0x3a) + $introPk
            $out += @{
                IntroMid   = ([BitConverter]::ToString($sha1.ComputeHash($fullIntro))).Replace('-','').ToLower()
                SeederPkHex = ([BitConverter]::ToString($seederPk)).Replace('-','').ToLower()
            }
        } catch { continue }
    }
    return $out
}
# Lecture du compteur de download quel que soit le sens.
function Dl-Bytes([string]$ih) {
    if ($Sens -eq 'A') { $d = TDownload-State $ih } else { $d = Download-State $P_N.Api $script:kN $ih }
    if ($d) { return [int64]$d.all_time_download } else { return -1 }
}
function Dl-Progress([string]$ih) {
    if ($Sens -eq 'A') { $d = TDownload-State $ih } else { $d = Download-State $P_N.Api $script:kN $ih }
    return $d
}
function Start-Tribler {
    $script:TApiPort = Get-FreeTcpPort
    $script:TIpv8 = Get-FreeUdpPort
    while ($script:TIpv8 -eq $script:TApiPort) { $script:TIpv8 = Get-FreeUdpPort }
    $conf.ipv8.interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $script:TIpv8 } )
    $conf.api.http_port = $script:TApiPort
    [System.IO.File]::WriteAllText((Join-Path $confDir 'configuration.json'),
        ($conf | ConvertTo-Json -Depth 10), [System.Text.UTF8Encoding]::new($false))
    $snap = [byte[]]@(0x01) + [System.Net.IPAddress]::Parse('127.0.0.1').GetAddressBytes() +
            [byte[]]@([byte]($P_A.Ipv8 -shr 8), [byte]($P_A.Ipv8 -band 0xFF))
    [System.IO.File]::WriteAllBytes((Join-Path $confDir 'exitnode_cache.dat'), $snap)
    $env:TSTATEDIR = $tstate
    $env:CORE_API_PORT = "$($script:TApiPort)"
    $env:CORE_API_KEY = $script:TApiKey
    $tProc = Start-Process -FilePath $triblerExe -PassThru -NoNewWindow `
        -ArgumentList '-s','--log-level','DEBUG' `
        -RedirectStandardOutput (Join-Path $out 'tribler_stdout.log') `
        -RedirectStandardError (Join-Path $out 'tribler_stderr.log')
    $procs['T'] = $tProc
    Log "T demarre pid=$($tProc.Id)"
    $up = $false; $w = (Get-Date).AddSeconds(180)
    while ((Get-Date) -lt $w -and -not $up) {
        try { TApi 'GET' '/ipv8/overlays' 5 | Out-Null; $up = $true } catch { Start-Sleep -Seconds 2 }
    }
    if (-not $up) { throw 'API Tribler jamais en ligne' }
    Log 'Tribler : API REST en ligne'
}

# ---------- Phase 0 : contenu + torrent ----------
Log "=== Phase 0 : contenu de test (sens $Sens, hops=$Hops) ==="
if (-not (Test-Path $daemon)) { throw 'tribler-daemon.exe absent - cargo build -p tribler-daemon' }
if (-not (Test-Path $mk)) { throw 'mk_torrent.exe absent' }
if (-not (Test-Path $triblerExe)) { throw "Tribler.exe absent : $triblerExe" }
$content = Join-Path $out 'content'
foreach ($p in (@($P_N, $P_A) + $relays)) {
    if (Test-Path $p.Dir) { Remove-Item -Recurse -Force $p.Dir }
}
$tseed = Join-Path $out 'tribler-seed'
$dld   = Join-Path $out $(if ($Sens -eq 'A') {'tribler-dl'} else {'rust-dl'})
foreach ($d in @($content, $tstate, $tseed, $dld)) {
    if (Test-Path $d) { Remove-Item -Recurse -Force $d }
}
New-Item -ItemType Directory -Force -Path $confDir, $tseed, $dld | Out-Null

$nonce = Get-Random -Minimum 1 -Maximum ([int]::MaxValue)
$script:TApiKey = "interop$nonce"
$mkOut = & $mk $content $Bytes $nonce
$mkOut | Tee-Object (Join-Path $rep 'mk_torrent.txt') | Out-Null
$torrent = Join-Path $content 'test.torrent'
$ih = (($mkOut | Select-String 'infohash=([0-9a-f]+)').Matches.Groups[1].Value)
if (-not $ih) { throw 'infohash non obtenu' }
# `get_lookup_info_hash` pyipv8 : SHA1('tribler anonymous download' + ih).
$lookup = ([System.BitConverter]::ToString(
    [System.Security.Cryptography.SHA1]::Create().ComputeHash(
        [System.Text.Encoding]::ASCII.GetBytes('tribler anonymous download' + $ih)))
).Replace('-', '').ToLower()
Log "infohash=$ih lookup_dht=$lookup"
$srcHash = (Get-FileHash -Algorithm SHA256 (Join-Path $content 'donnee.bin')).Hash.ToLower()
Log "sha256 source=$srcHash"

# ---------- Config Tribler isolee (meme topo que les bancs nominaux) ----------
$conf = @{
    api = @{ http_enabled = $true; http_port = 0; http_host = '127.0.0.1';
             https_enabled = $false; key = $script:TApiKey }
    ipv8 = @{
        logger = @{ level = 'DEBUG' }
        interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = 0 } )
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
        port = 0; utp = $true; dht = $false; upnp = $false; natpmp = $false; lsd = $false
        download_defaults = @{ saveas = $dld }
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

$deadline = (Get-Date).AddMinutes($TimeoutMin)
$script:kN = $null
try {
    # ---------- Phase 1 : maillage + demarrage du seeder ----------
    Log '=== Phase 1 : ancre A1 (EXIT_BT) + relais + seeder ==='
    foreach ($p in (@($P_A, $P_N) + $relays)) {
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
        # A2 est un second exit (EXIT_BT) : sans lui le kill de A1
        # laisserait le maillage sans aucune sortie et les circuits
        # DATA 1 saut ne pourraient jamais etre reconstruits —
        # resultat conforme a pyipv8 mais trivial. Un vrai reseau a
        # plusieurs exits.
        $rConf = @{ ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($P_A.Ipv8)") };
                             interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $p.Ipv8 } );
                             estimated_wan = "127.0.0.1:$($p.Ipv8)" } }
        if ($p.Name -eq 'A2') { $rConf.tunnel_community = @{ exitnode_enabled = $true } }
        [System.IO.File]::WriteAllText((Join-Path $p.Dir 'configuration.json'),
            ($rConf | ConvertTo-Json -Compress -Depth 5))
        Start-Daemon $p @()
        $k = Wait-ApiKey $p.Dir; Wait-ApiUp $p.Api $k; Assert-ConfigOk $p
    }

    $manifest = [ordered]@{
        run_utc = [datetime]::UtcNow.ToString('o')
        script  = 'interop_hidden_killseeder.ps1'
        sens    = $Sens
        daemon  = $daemon
        tribler = $triblerExe
        hops    = $Hops
        outdir  = $out
    }

    if ($Sens -eq 'A') {
        # S = seeder Rust. Pin de l'intro point sur A1 pour le scenario
        # 'seeder' (determinisme) ; en 'intro' le pin imposerait le pair
        # mort comme required_exit et empecherait toute reconstruction
        # — pyipv8 n'epingle jamais l'intro point en production.
        $sConf = @{ ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($P_A.Ipv8)") };
                             interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $P_N.Ipv8 } );
                             estimated_wan = "127.0.0.1:$($P_N.Ipv8)" } }
        if ($KillTarget -eq 'seeder') {
            $sConf.tunnel_community = @{ intro_point_peer = "127.0.0.1:$($P_A.Ipv8)" }
        }
        [System.IO.File]::WriteAllText((Join-Path $P_N.Dir 'configuration.json'),
            ($sConf | ConvertTo-Json -Compress -Depth 5))
        Start-Daemon $P_N @()
        $script:kN = Wait-ApiKey $P_N.Dir; Wait-ApiUp $P_N.Api $script:kN; Assert-ConfigOk $P_N

        Api 'PUT' $P_N.Api '/downloads' $script:kN @{ torrent = $torrent; destination = $content;
            anon_hops = $Hops; safe_seeding = $true } | Out-Null
        $seeding = $false
        while ((Get-Date) -lt $deadline) {
            $d = Download-State $P_N.Api $script:kN $ih
            if ($d -and $d.status -eq 'SEEDING') { $seeding = $true; break }
            Start-Sleep -Seconds 3
        }
        Verdict $seeding 'S passe en SEEDING' "infohash=$ih"
        if (-not $seeding) { throw 'jamais SEEDING' }
        $ipOk = $false; $ipWait = (Get-Date).AddMinutes(6)
        while ((Get-Date) -lt $ipWait) {
            $nIp = @(Circuits $P_N.Api $script:kN 'IP_SEEDER').Count
            if ($nIp -ge 1) { $ipOk = $true; break }
            Start-Sleep -Seconds 5
        }
        Verdict $ipOk 'S : circuit(s) IP_SEEDER READY' "n=$nIp"
        # Gate DHT : la valeur de l'intro point est relue depuis A1.
        $dhtOk = $false; $dhtWait = (Get-Date).AddMinutes(8)
        while ((Get-Date) -lt $dhtWait) {
            try {
                $vA = Api 'GET' $P_A.Api "/ipv8/dht/values/$lookup" $kA $null 30
                if (@($vA.values).Count -gt 0) { $dhtOk = $true; break }
            } catch {}
            Start-Sleep -Seconds 15
        }
        Verdict $dhtOk 'DHT : annonce du point d introduction propagee'

        Start-Tribler
        # Bootstrap mesh : >= 3 pairs tunnel dont >= 1 exit.
        $tPeers = $false; $nP = 0; $nTunnel = 0; $exits = 0; $pWait = (Get-Date).AddMinutes(6)
        while ((Get-Date) -lt $pWait -and -not $tPeers) {
            try {
                $ov = TApi 'GET' '/ipv8/overlays' 15
                $nP = 0; $nTunnel = 0; $exits = 0
                foreach ($o in @($ov.overlays)) {
                    $nP += @($o.peers).Count
                    if ($o.overlay_name -match 'Tunnel') {
                        foreach ($pr in @($o.peers)) { $nTunnel++; if (($pr.flags -band 32) -ne 0) { $exits++ } }
                    }
                }
                if ($nTunnel -ge 3 -and $exits -ge 1) { $tPeers = $true }
            } catch { Start-Sleep -Seconds 3 }
            if (-not $tPeers) { Start-Sleep -Seconds 5 }
        }
        Verdict $tPeers 'Tribler : bootstrap maillage controle' "peers=$nP tunnel=$nTunnel exits=$exits"

        $dest = [System.Uri]::EscapeDataString($dld)
        $putUri = "http://127.0.0.1:$($script:TApiPort)/api/downloads?anon_hops=$Hops&safe_seeding=true&destination=$dest"
        $tAdd = $null; $putEnd = (Get-Date).AddMinutes(5)
        while (-not $tAdd -and (Get-Date) -lt $putEnd) {
            try {
                $tAdd = Invoke-RestMethod -Method PUT -Uri $putUri -Headers @{ 'X-Api-Key' = $script:TApiKey } `
                    -ContentType 'applications/x-bittorrent' -InFile $torrent -TimeoutSec 30
                if ($tAdd.PSObject.Properties['error'] -and $tAdd.error) {
                    Log "PUT reponse : $($tAdd.error | ConvertTo-Json -Compress -Depth 4)"; $tAdd = $null; Start-Sleep -Seconds 10
                }
            } catch { Log "PUT : $($_.Exception.Message)"; Start-Sleep -Seconds 10 }
        }
        $addOk = ($tAdd -and $tAdd.started -eq $true -and $tAdd.infohash -eq $ih)
        Verdict $addOk 'Tribler : download anonyme accepte' "started=$($tAdd.started)"
        if (-not $addOk) { throw 'ajout download refuse' }
    } else {
        # ---------- Sens B : Tribler est le seeder ----------
        Start-Tribler
        $tPeers = $false; $nP = 0; $nTunnel = 0; $exits = 0; $pWait = (Get-Date).AddMinutes(6)
        while ((Get-Date) -lt $pWait -and -not $tPeers) {
            try {
                $ov = TApi 'GET' '/ipv8/overlays' 15
                $nP = 0; $nTunnel = 0; $exits = 0
                foreach ($o in @($ov.overlays)) {
                    $nP += @($o.peers).Count
                    if ($o.overlay_name -match 'Tunnel') {
                        foreach ($pr in @($o.peers)) { $nTunnel++; if (($pr.flags -band 32) -ne 0) { $exits++ } }
                    }
                }
                if ($nTunnel -ge 3 -and $exits -ge 1) { $tPeers = $true }
            } catch { Start-Sleep -Seconds 3 }
            if (-not $tPeers) { Start-Sleep -Seconds 5 }
        }
        Verdict $tPeers 'Tribler : bootstrap maillage controle' "peers=$nP tunnel=$nTunnel exits=$exits"

        # PUT "seed" : destination = le dossier contenant deja donnee.bin
        # -> hashcheck -> SEEDING -> join_swarm -> IP_SEEDER.
        Copy-Item (Join-Path $content 'donnee.bin') (Join-Path $tseed 'donnee.bin')
        $dest = [System.Uri]::EscapeDataString($tseed)
        $putUri = "http://127.0.0.1:$($script:TApiPort)/api/downloads?anon_hops=$Hops&safe_seeding=true&destination=$dest"
        $tAdd = $null; $putEnd = (Get-Date).AddMinutes(5)
        while (-not $tAdd -and (Get-Date) -lt $putEnd) {
            try {
                $tAdd = Invoke-RestMethod -Method PUT -Uri $putUri -Headers @{ 'X-Api-Key' = $script:TApiKey } `
                    -ContentType 'applications/x-bittorrent' -InFile $torrent -TimeoutSec 30
                if ($tAdd.PSObject.Properties['error'] -and $tAdd.error) {
                    Log "PUT reponse : $($tAdd.error)"; $tAdd = $null; Start-Sleep -Seconds 10
                }
            } catch { Log "PUT : $($_.Exception.Message)"; Start-Sleep -Seconds 10 }
        }
        Verdict ($tAdd -and $tAdd.started) 'Tribler : download (seed) accepte' "started=$($tAdd.started)"
        $sWait = (Get-Date).AddMinutes(10); $seeding = $false
        while ((Get-Date) -lt $sWait) {
            $dt = TDownload-State $ih
            if ($dt -and $dt.status -eq 'SEEDING') { $seeding = $true; break }
            Start-Sleep -Seconds 3
        }
        Verdict $seeding 'Tribler : SEEDING (hidden swarm joined)'
        if (-not $seeding) { throw 'T jamais SEEDING' }
        $ipOk = $false; $ipWait = (Get-Date).AddMinutes(6)
        while ((Get-Date) -lt $ipWait) {
            $nIp = @(TCircuits 'IP_SEEDER').Count
            if ($nIp -ge 1) { $ipOk = $true; break }
            Start-Sleep -Seconds 5
        }
        Verdict $ipOk 'Tribler : circuit(s) IP_SEEDER READY' "n=$nIp"
        # Gate DHT : la valeur publiee par l'intro point de T est relue.
        $dhtOk = $false; $dhtWait = (Get-Date).AddMinutes(10)
        while ((Get-Date) -lt $dhtWait) {
            try {
                $vA = Api 'GET' $P_A.Api "/ipv8/dht/values/$lookup" $kA $null 30
                if (@($vA.values).Count -gt 0) { $dhtOk = $true; break }
            } catch {}
            Start-Sleep -Seconds 15
        }
        Verdict $dhtOk 'DHT : intro-point de T stocke et relu'

        # Downloader Rust D.
        [System.IO.File]::WriteAllText((Join-Path $P_N.Dir 'configuration.json'),
            (@{ ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($P_A.Ipv8)") };
                          interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $P_N.Ipv8 } );
                          estimated_wan = "127.0.0.1:$($P_N.Ipv8)" } } |
                ConvertTo-Json -Compress -Depth 5))
        Start-Daemon $P_N @()
        $script:kN = Wait-ApiKey $P_N.Dir; Wait-ApiUp $P_N.Api $script:kN; Assert-ConfigOk $P_N
        Api 'PUT' $P_N.Api '/downloads' $script:kN @{ torrent = $torrent; destination = $dld;
            anon_hops = $Hops; safe_seeding = $true } | Out-Null
        Verdict $true 'D : download anonyme accepte' "infohash=$ih"
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'manifest.json') -Encoding UTF8

    # ---------- Phase 2 : data plane vivant, puis KILL ----------
    $alive = $false; $dlNow = -1; $aWait = (Get-Date).AddMinutes(8)
    while ((Get-Date) -lt $aWait -and -not $alive) {
        Start-Sleep -Seconds 3
        $dlNow = Dl-Bytes $ih
        if ($dlNow -ge $KillAtBytes) { $alive = $true }
    }
    Verdict $alive "data plane : downloader >= $KillAtBytes octets avant kill" "dl=$dlNow"
    if (-not $alive) { throw 'data plane jamais demarre' }

    # ---------- Resolution du noeud cible ----------
    # Port ipv8 -> nom de noeud du banc. mid = sha1(pubkey maitresse),
    # resolu deterministement via /ipv8/overlays (my_peer) de chaque
    # noeud — couvre A1, absent de sa propre liste de peers.
    $portMap = @{}; $midMap = @{}
    $sha1 = [System.Security.Cryptography.SHA1]::Create()
    foreach ($p in (@($P_N, $P_A) + $relays)) {
        $portMap[[int]$p.Ipv8] = $p.Name
        $ov = @((Api 'GET' $p.Api '/ipv8/overlays' (ApiKey $p.Dir)).overlays)
        if ($ov.Count -gt 0 -and $ov[0].my_peer) {
            $pkHex = $ov[0].my_peer
            $pkBytes = [byte[]]::new($pkHex.Length / 2)
            for ($i = 0; $i -lt $pkBytes.Length; $i++) {
                $pkBytes[$i] = [Convert]::ToByte($pkHex.Substring($i * 2, 2), 16)
            }
            $midMap[([BitConverter]::ToString($sha1.ComputeHash($pkBytes))).Replace('-', '').ToLower()] = $p.Name
        }
    }

    $killTime = Get-Date
    $killedNode = $null; $killedMid = $null
    if ($KillTarget -eq 'seeder') {
        if ($Sens -eq 'A') {
            $procs['S'].Kill(); $procs['S'].WaitForExit()
            Log "KILL seeder Rust S (pid force-killed) a dl=$dlNow"
        } else {
            $procs['T'].Kill(); $procs['T'].WaitForExit()
            Log "KILL seeder Tribler T (pid force-killed) a dl=$dlNow"
        }
        Verdict $true 'seeder tue en plein transfert' "dl_au_kill=$dlNow"

        # Drain borne : les octets in-flight peuvent encore arriver,
        # mais la croissance doit s'arreter dans la fenetre DrainSec.
        $lastGrow = $killTime; $dlLast = $dlNow; $drainEnd = $killTime.AddSeconds($DrainSec)
        while ((Get-Date) -lt $drainEnd) {
            Start-Sleep -Seconds 1
            $d2 = Dl-Bytes $ih
            if ($d2 -gt $dlLast) { $lastGrow = Get-Date; $dlLast = $d2 }
        }
        $drainStop = ($lastGrow - $killTime).TotalSeconds
        $drained = $dlLast - $dlNow
        Verdict $true 'drain borne : croissance in-flight arretee' ("dernier octet +{0:N1}s apres kill, +{1} o drains" -f $drainStop, $drained)

        # Fenetre morte stricte : 0 octet pendant DeadSec.
        $deadBase = Dl-Bytes $ih
        $deadEnd = (Get-Date).AddSeconds($DeadSec)
        $deadClean = $true
        while ((Get-Date) -lt $deadEnd) {
            Start-Sleep -Seconds 3
            if ((Dl-Bytes $ih) -gt $deadBase) { $deadClean = $false; break }
        }
        Verdict $deadClean "fenetre morte stricte : 0 octet pendant ${DeadSec}s" "dl fige a $deadBase"
    } else {
        # ---------- Kill de l'intro point / de l'ancre ----------
        # Le dernier saut du circuit IP_SEEDER du seeder = le point
        # d'introduction. verified_hops = mids ; on mappe mid->port via
        # sha1(pubkey) -> nom. Pour 'anchor', la cible est toujours A1
        # (bootstrap + EXIT_BT) quel que soit le dernier saut du
        # premier IP_SEEDER — le test mesure la survivabilite du flux
        # etabli et la decouverte SANS le noeud d'amorcage.
        $ipCircs = if ($Sens -eq 'A') { @(Circuits $P_N.Api $script:kN 'IP_SEEDER') } else { @(TCircuits 'IP_SEEDER') }
        # Snapshot des ids : le verdict de reconstruction exige un
        # circuit NOUVEAU (d'autres IP_SEEDER peuvent deja avoir un
        # dernier saut different du noeud tue — le seeder maintient
        # plusieurs circuits d'introduction). Tous les etats sont
        # captures (un EXTENDING qui completerait apres le kill ne
        # compte pas comme reconstruction).
        $preIpIds = if ($Sens -eq 'A') {
            @((Circuits $P_N.Api $script:kN 'IP_SEEDER' $null) | ForEach-Object { $_.circuit_id })
        } else {
            @((TCircuits 'IP_SEEDER') | ForEach-Object { $_.circuit_id })
        }
        if ($KillTarget -eq 'anchor') {
            $killedNode = 'A'
            $killedMid = ($midMap.GetEnumerator() | Where-Object { $_.Value -eq 'A' } |
                Select-Object -First 1).Key
            if (-not $killedMid) { throw 'mid de A1 introuvable dans midMap' }
            # La reconstruction n'est exigible que si A1 figurait dans
            # le chemin d'au moins un IP_SEEDER du seeder (ces circuits
            # meurent avec lui). Si l'intro point vivait ailleurs, le
            # circuit survit et aucun rebuild n'est attendu.
            $anchorInIpPath = @($ipCircs | Where-Object {
                $_.verified_hops -and ($_.verified_hops -contains $killedMid) }).Count -gt 0
        } else {
            $anchorInIpPath = $true   # cible = dernier saut par definition
            $killedMid = $ipCircs[0].verified_hops[-1]
            $killedNode = $midMap[$killedMid]
            if (-not $killedNode) { throw "intro point mid=$killedMid hors maillage" }
        }
        # Snapshot des annonces PRE-kill : seeder_pk commun + ensemble
        # des mids d'intro points — sert a attribuer la re-annonce au
        # SEEDER (et non a un autre seeder du meme swarm, ex. le
        # downloader devenu seeder apres completion).
        $preVals = @(Dht-IntroValues $P_A.Api $kA $lookup $sha1)
        $seederPkHex = if ($preVals.Count -gt 0) { $preVals[0].SeederPkHex } else { $null }
        $preIntroMids = @{}; foreach ($v in $preVals) { $preIntroMids[$v.IntroMid] = $true }
        Log ("DHT pre-kill : n=$($preVals.Count) annonce(s) seeder_pk=" +
             $(if ($seederPkHex) { $seederPkHex.Substring(0,[Math]::Min(12,$seederPkHex.Length)) + '…' } else { 'absent' }))
        $procs[$killedNode].Kill(); $procs[$killedNode].WaitForExit()
        $cibleTxt = if ($KillTarget -eq 'anchor') { 'ancre A1' } else { 'intro point' }
        Log "KILL $cibleTxt : noeud $killedNode (mid=$($killedMid.Substring(0,12))…) a dl=$dlNow"
        Verdict $true "$cibleTxt tue en plein transfert" "noeud=$killedNode dl_au_kill=$dlNow"

        # Observation (non bloquante) : le flux e2e etabli est
        # theoriquement independant du circuit d'introduction — la
        # mesure indique si le chemin e2e traversait le noeud tue.
        $deadBase = Dl-Bytes $ih
        Start-Sleep -Seconds $DeadSec
        $deadGrew = (Dl-Bytes $ih) -gt $deadBase
        Log ("post-kill ${DeadSec}s : flux e2e " + $(if ($deadGrew) { 'A SURVENU (chemin e2e independant)' } else { 'fige (e2e traversait le noeud tue)' }))
        Verdict $true "post-kill ${DeadSec}s : flux e2e" $(if ($deadGrew) { "survecu - $cibleTxt seul detruit" } else { 'fige - reconstruction necessaire' })

        # Ancre seulement : la decouverte doit continuer sans le
        # noeud d'amorcage — le seeder garde des pairs verifies
        # (A2/A3) dans l'overlay tunnel apres elagage du pair mort.
        if ($KillTarget -eq 'anchor') {
            Start-Sleep -Seconds 60   # fenetre du churn (drop_time=57,5 s)
            $discOk = $false; $nSeen = 0
            if ($Sens -eq 'A') {
                $ov = @((Api 'GET' $P_N.Api '/ipv8/overlays' $script:kN).overlays)
                foreach ($o in $ov) { if ($o.overlay_name -match 'Tunnel') { $nSeen = @($o.peers).Count } }
            } else {
                $ov = @((TApi 'GET' '/ipv8/overlays' 15).overlays)
                foreach ($o in $ov) { if ($o.overlay_name -match 'Tunnel') { $nSeen = @($o.peers).Count } }
            }
            # A1 elague mais A2/A3 encore vus : la decouverte ne
            # depend pas du bootstrap.
            $discOk = ($nSeen -ge 2)
            Verdict $discOk "decouverte sans ancre : pairs tunnel encore visibles" "n=$nSeen (A1 elague attendu)"
        }
        $restartTime = Get-Date
    }

    # ---------- Phase 3 : reconstruction / restart ----------
    $restartTime = Get-Date
    if ($KillTarget -eq 'seeder' -and $Sens -eq 'A') {
        Start-Daemon $P_N @()
        $script:kN = Wait-ApiKey $P_N.Dir; Wait-ApiUp $P_N.Api $script:kN
        # Le download seede est restaure par fastresume (meme infohash) :
        # transition d'etat -> join_swarm -> nouveaux IP_SEEDER.
        $rs = $false; $rsWait = (Get-Date).AddMinutes(5)
        while ((Get-Date) -lt $rsWait) {
            $d = Download-State $P_N.Api $script:kN $ih
            if ($d -and $d.status -eq 'SEEDING') { $rs = $true; break }
            Start-Sleep -Seconds 3
        }
        Verdict $rs 'S redemarre : download restaure SEEDING' "infohash=$ih"
        $rip = $false; $ripWait = (Get-Date).AddMinutes(6)
        while ((Get-Date) -lt $ripWait) {
            $nIp = @(Circuits $P_N.Api $script:kN 'IP_SEEDER').Count
            if ($nIp -ge 1) { $rip = $true; break }
            Start-Sleep -Seconds 5
        }
        Verdict $rip 'S redemarre : IP_SEEDER reconstruit(s)' "n=$nIp"
    } elseif ($KillTarget -in @('intro', 'anchor')) {
        # Pas de restart : si le noeud tue portait un IP_SEEDER du
        # seeder, celui-ci (vivant) doit reconstruire un IP_SEEDER sur
        # un AUTRE noeud et re-annoncer sur la DHT. Si l'ancre n'etait
        # dans aucun chemin IP_SEEDER, les circuits survivent et c'est
        # cette survie qu'on verifie.
        if ($anchorInIpPath) {
            $rip = $false; $ripWait = (Get-Date).AddMinutes(8); $newMid = $null
            while ((Get-Date) -lt $ripWait) {
                $ipCircs = if ($Sens -eq 'A') { @(Circuits $P_N.Api $script:kN 'IP_SEEDER') } else { @(TCircuits 'IP_SEEDER') }
                $newIp = @($ipCircs | Where-Object {
                    ($preIpIds -notcontains $_.circuit_id) -and
                    $_.verified_hops -and $_.verified_hops[-1] -ne $killedMid })
                if ($newIp.Count -ge 1) { $rip = $true; $newMid = $newIp[0].verified_hops[-1]; break }
                Start-Sleep -Seconds 5
            }
            Verdict $rip 'IP_SEEDER reconstruit sur un noeud different' $(if ($newMid) { "mid=$($newMid.Substring(0,12))…" } else { '' })
        } else {
            # L'ancre n'etait dans aucun chemin IP_SEEDER : les circuits
            # d'introduction doivent etre restes intacts.
            Start-Sleep -Seconds 10   # laisser le churn travailler
            $ipNow = if ($Sens -eq 'A') { @(Circuits $P_N.Api $script:kN 'IP_SEEDER') } else { @(TCircuits 'IP_SEEDER') }
            $alive = @($ipNow | Where-Object { $_.verified_hops -and $_.verified_hops[-1] -ne $killedMid })
            $newMid = if ($alive.Count -gt 0) { $alive[0].verified_hops[-1] } else { $null }
            Verdict ($alive.Count -ge 1) 'IP_SEEDER intacts : ancre hors chemin, intro vivant' "n=$($alive.Count)"
        }
        # Annonce DHT attribuable au seeder : meme seeder_pk que le
        # snapshot pre-kill. Si le noeud tue portait un intro point on
        # exige en plus un intro_mid NOUVEAU (hors ensemble pre-kill —
        # idealement == newMid si deja resolu) ; sinon l'annonce
        # preexistante doit rester lisible/re-publiee. Une annonce du
        # downloader devenu seeder (seeder_pk different) ne satisfait
        # PAS ce critere.
        $probe = $P_A; if ($killedNode -eq 'A') { $probe = $relays[0] }
        $probeKey = ApiKey $probe.Dir
        $dhtOk = $false; $dhtWait = (Get-Date).AddMinutes(6); $dhtDetail = ''
        while ((Get-Date) -lt $dhtWait) {
            $vals = @(Dht-IntroValues $probe.Api $probeKey $lookup $sha1)
            foreach ($v in $vals) {
                $sameSeeder = ($seederPkHex -and $v.SeederPkHex -eq $seederPkHex)
                $newIntro = -not $preIntroMids.ContainsKey($v.IntroMid)
                $introOk = if ($anchorInIpPath) { $newIntro -and (-not $newMid -or $v.IntroMid -eq $newMid) } else { $true }
                if ($sameSeeder -and $introOk) {
                    $dhtOk = $true
                    $dhtDetail = "intro_mid=$($v.IntroMid.Substring(0,12))… seeder=identique"
                    break
                }
            }
            if ($dhtOk) { break }
            Start-Sleep -Seconds 10
        }
        Verdict $dhtOk "DHT : annonce du NOUVEL intro point du seeder (sonde $($probe.Name))" $dhtDetail

        # Si le flux s'etait fige, la reprise doit venir de la
        # re-decouverte : le downloader re-looke la DHT et recree un e2e.
        if (-not $deadGrew) {
            $resumed = $false; $resumeAt = $null
            $rEnd = (Get-Date).AddSeconds($ResumeSec)
            while ((Get-Date) -lt $rEnd -and (Get-Date) -lt $deadline) {
                Start-Sleep -Seconds 5
                $d3 = Dl-Bytes $ih
                if ($d3 -gt $deadBase) { $resumed = $true; $resumeAt = Get-Date; break }
            }
            Verdict $resumed 'reprise : download repart apres reconstruction intro' `
                $(if ($resumed) { "dl=$d3 delai={0:N0}s" -f ($resumeAt - $restartTime).TotalSeconds } else { "toujours fige a $deadBase" })
            if (-not $resumed) { throw 'jamais repris apres kill intro' }
        }
    } else {
        # Sens B, kill seeder : restart Tribler.
        Start-Tribler
        $rs = $false; $rsWait = (Get-Date).AddMinutes(8)
        while ((Get-Date) -lt $rsWait) {
            $dt = TDownload-State $ih
            if ($dt -and $dt.status -eq 'SEEDING') { $rs = $true; break }
            Start-Sleep -Seconds 4
        }
        Verdict $rs 'Tribler redemarre : download restaure SEEDING' "infohash=$ih"
        $rip = $false; $ripWait = (Get-Date).AddMinutes(6)
        while ((Get-Date) -lt $ripWait) {
            $nIp = @(TCircuits 'IP_SEEDER').Count
            if ($nIp -ge 1) { $rip = $true; break }
            Start-Sleep -Seconds 5
        }
        Verdict $rip 'Tribler redemarre : IP_SEEDER reconstruit(s)' "n=$nIp"
    }

    # ---------- Phase 4 : reprise (sens seeder uniquement) ----------
    if ($KillTarget -eq 'seeder') {
        $resumed = $false; $resumeAt = $null
        $rEnd = (Get-Date).AddSeconds($ResumeSec)
        while ((Get-Date) -lt $rEnd -and (Get-Date) -lt $deadline) {
            Start-Sleep -Seconds 5
            $d3 = Dl-Bytes $ih
            if ($d3 -gt $deadBase) { $resumed = $true; $resumeAt = Get-Date; break }
            if ((($deadline - (Get-Date)).TotalSeconds % 30) -lt 6) {
                Log ("attente reprise... dl={0} (fige a {1})" -f $d3, $deadBase)
            }
        }
        Verdict $resumed 'reprise : le download repart apres restart du seeder' `
            $(if ($resumed) { "dl=$d3 delai={0:N0}s apres restart" -f ($resumeAt - $restartTime).TotalSeconds } else { "toujours fige a $deadBase" })
        if (-not $resumed) { throw 'jamais repris' }
    }

    # ---------- Phase 5 : completion + integrite ----------
    $done = $false
    while ((Get-Date) -lt $deadline -and -not $done) {
        Start-Sleep -Seconds 8
        $d4 = Dl-Progress $ih
        if ($d4) {
            Log ("download : progress={0:P1} dl={1}B st={2}" -f $d4.progress, $d4.all_time_download, $d4.status)
            if ($d4.progress -ge 1.0) { $done = $true }
        }
    }
    Verdict $done 'telechargement termine apres reprise' "progress=$(if($d4){$d4.progress}else{'?'})"
    if ($done) {
        Start-Sleep -Seconds 3
        $f = Get-ChildItem -Recurse -Filter 'donnee.bin' $dld -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($f) {
            # Le fichier peut rester verrouille par le moteur qui
            # seede (handle/mmap exclusif cote Rust) — ouverture en
            # FileShare.ReadWrite puis retry borne.
            $h = $null; $hWait = (Get-Date).AddSeconds(45)
            while (-not $h -and (Get-Date) -lt $hWait) {
                try {
                    $fs = [System.IO.File]::Open($f.FullName, 'Open', 'Read', 'ReadWrite')
                    try { $h = ([BitConverter]::ToString(
                        [System.Security.Cryptography.SHA256]::Create().ComputeHash($fs)
                    )).Replace('-', '').ToLower() }
                    finally { $fs.Close() }
                } catch { Start-Sleep -Seconds 3 }
            }
            if ($h) {
                Verdict ($h -eq $srcHash) 'integrite : SHA256 du fichier recu' "got=$h want=$srcHash"
            } else { Verdict $false 'integrite : SHA256 du fichier recu' 'hash illisible (fichier verrouille)' }
        } else { Verdict $false 'integrite : SHA256 du fichier recu' 'donnee.bin introuvable' }
    }
}
finally {
    foreach ($p in (@($P_N, $P_A) + $relays)) {
        try {
            $k = ApiKey $p.Dir
            if ($k) {
                foreach ($ep in @('/downloads','/ipv8/tunnel/circuits','/ipv8/tunnel/swarms','/ipv8/tunnel/peers')) {
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
    try { TCircuits | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $rep 'T_circuits.json') -Encoding UTF8 } catch {}
    try { TApi 'GET' "/downloads?infohash=$ih" 10 | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $rep 'T_download_detail.json') -Encoding UTF8 } catch {}
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
    # Persistance : le stdout des bancs peut etre tronque en tampon
    # circulaire — les verdicts doivent survivre dans le rapport.
    $verdicts | Set-Content (Join-Path $rep 'verdicts.txt') -Encoding UTF8
    Write-Host ''
    Write-Host '================ VERDICTS ================'
    $verdicts | ForEach-Object { Write-Host $_ }
    Write-Host "rapport : $rep"
}
