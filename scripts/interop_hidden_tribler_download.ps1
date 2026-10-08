# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

#requires -Version 5.1

<#
.SYNOPSIS
  Interop hidden-service sens A : Tribler 8.4.3 REEL telecharge depuis
  un seeder anonyme Rust.

.DESCRIPTION
  Contrairement a live_hidden_upload.ps1 (Rust<->Rust relaye par le
  reseau reel), le downloader ici est le client Tribler officiel non
  modifie : on valide la compatibilite filaire avec pyipv8
  (DHTIntroPointPayload, create-e2e, link-e2e), le uTP entrant d'un
  vrai client, et l'acceptation du handshake/session BitTorrent.

  Topologie CONTROLEE (apres echec du mode public : les exits WAN
  ne repondaient jamais aux peers-request -- controle Py<->Py meme
  symptome, donc environnement, pas wire) :

    A1 = ancre Rust EXIT_BT + intro point epingle de S  api 8097 ipv8 17787
    A2 = relais Rust pur                                api 8098 ipv8 17788
    A3 = relais Rust pur                                api 8099 ipv8 17789
    S  = seeder anonyme Rust  (bootstrap -> A1, intro -> A1)
                                                   api 8095 ipv8 17785
    T  = Tribler.exe -s REEL (etat isole, bootstrap -> A1 UNIQUEMENT,
         decouvre A2/A3/S via les introduction-response de A1)

  Tout le chemin transite par nos noeuds : le circuit DATA 1-hop de
  Tribler sort forcement chez A1 (seul EXIT_BT) ; A1 repond au
  peers-request depuis son `intro_point_for` local (il EST le point
  d'introduction de S) ou via le fallback find_values ajoute cote
  exit. RP_DOWNLOADER (hops+1 = 2 sauts) utilise A2/A3.

  Chaine de preuve attendue :
    S SEEDING -> IP_SEEDER -> annonce DHT -> Tribler bootstrap ->
    PUT /downloads (torrent binaire, anon_hops, safe_seeding) ->
    peers-request via A1 -> peers-response (intro point) ->
    create-e2e -> linked-e2e chez S -> uTP accept -> handshake BT ->
    pieces verifiees -> hash identique.

  Pas de kill/restart ici : ce banc prouve l'interop de base d'abord.
#>
param(
    [int]    $Bytes      = 6291456,
    [int]    $Hops       = 1,
    [int]    $TimeoutMin = 45,
    # Nouveau repertoire d'etat par defaut a chaque run (horodate) :
    # un ancien run ne doit jamais partager metadata.db / exitnode_cache
    # avec le suivant (les handles restants verrouillaient les fichiers).
    [string] $OutDir     = ("target\interop-hidden-dl-" + (Get-Date -Format 'yyyyMMdd-HHmmss')),
    # -DhtOnly : run de collecte cible — mesh ferme + S SEEDING +
    # IP_SEEDER + annonce DHT + relecture de la valeur par lookup sur
    # un autre noeud. Pas de Tribler.exe : verdict binaire sur le
    # maillon store-request avant tout E2E.
    [switch] $DhtOnly,
    # -Guards : active tunnel_community.guards_enabled sur tous les
    # noeuds Rust du maillage (selection de premier saut gardee) et
    # ajoute les verdicts guard-set dans le rapport : set non vide et
    # premier hop de chaque circuit multi-hop dans le set persiste.
    [switch] $Guards
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

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root   = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$out    = Join-Path $root $OutDir
$daemon = Join-Path $root 'target\debug\onionbit-daemon.exe'
$mk     = Join-Path $root 'target\debug\examples\mk_torrent.exe'
$rep    = Join-Path $out 'report'
$triblerExe = if ($env:TRIBLER_EXE) { $env:TRIBLER_EXE } else { 'C:\Program Files (x86)\Tribler\Tribler.exe' }
$tstate = Join-Path $out 'tribler-state'
$confDir = Join-Path $tstate '8.0'
New-Item -ItemType Directory -Force -Path $out, $rep, $confDir | Out-Null

$P_S  = @{ Api = 8095; Ipv8 = 17785; Dir = Join-Path $out 'seed';    Name = 'S' }
$P_A  = @{ Api = 8097; Ipv8 = 17787; Dir = Join-Path $out 'anchor';  Name = 'A' }
$P_A2 = @{ Api = 8098; Ipv8 = 17788; Dir = Join-Path $out 'relay2';  Name = 'A2' }
$P_A3 = @{ Api = 8099; Ipv8 = 17789; Dir = Join-Path $out 'relay3';  Name = 'A3' }
# Relais supplementaires a hops>=3 : les circuits IP_SEEDER et
# RP_DOWNLOADER font hops+1 sauts (`swarm.hops + 1` cote Rust,
# `download_hops + 1` cote pyipv8) — il faut donc `hops` relais
# libres distincts, le dernier saut impose (required_exit) etant
# exclu des candidats. Avec 2 relais seulement, hops=3 ne peut
# jamais s'etendre ("no candidates to extend").
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
function Assert-Guards($p, $key) {
    # Criteres de la matrice guards : set actif non vide + premier hop
    # de chaque circuit multi-hop (`verified_hops[0]`, meme `mid` hex
    # que `guards[].mid`) appartenant au set adopte. Sur loopback la
    # dedup /24 n'admet qu'un guard — l'assertion reste exacte.
    try {
        $g = Api 'GET' $p.Api '/ipv8/tunnel/guards' $key $null 10
        $mids = @($g.guards | ForEach-Object { $_.mid })
        Verdict (($g.enabled -eq $true) -and ($mids.Count -gt 0)) "$($p.Name) : guard set actif" "n=$($mids.Count)"
        $cs = Api 'GET' $p.Api '/ipv8/tunnel/circuits' $key $null 10
        $multi = @($cs.circuits | Where-Object { $_.actual_hops -ge 2 -and @($_.verified_hops).Count -gt 0 })
        if ($multi.Count -eq 0) { Verdict $true "$($p.Name) : premier hop dans le guard set (aucun circuit multi-hop)"; return }
        $bad = @($multi | Where-Object { $mids -notcontains $_.verified_hops[0] })
        Verdict ($bad.Count -eq 0) "$($p.Name) : premier hop dans le guard set" "multi=$($multi.Count) hors_set=$($bad.Count)"
    } catch { Verdict $false "$($p.Name) : diagnostic guards" $_.Exception.Message }
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
    $psi.EnvironmentVariables['RUST_LOG'] = 'info,onionbit_ipv8=debug,onionbit_tunnel=debug,onionbit_core=debug,librqbit_utp=debug,librqbit=debug'
    $proc = [System.Diagnostics.Process]::Start($psi)
    $procs[$p.Name] = $proc
    Log ("{0} demarre pid={1} bootstrap=[{2}]" -f $p.Name, $proc.Id, ($boot -join ', '))
}
function Assert-ConfigOk($p) {
    # Le daemon normalise configuration.json au demarrage ; si le fichier
    # qu'on a ecrit etait invalide il logge "corrompu" et retombe sur les
    # defaults -> le banc serait sterile. Fail-fast plutot que 20 min
    # d'attente pour rien.
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
if (-not (Test-Path $daemon)) { throw 'onionbit-daemon.exe absent - cargo build -p onionbit-daemon' }
if (-not (Test-Path $mk)) { throw 'mk_torrent.exe absent - cargo build -p onionbit-bittorrent --example mk_torrent' }
if (-not (Test-Path $triblerExe)) { throw "Tribler.exe absent : $triblerExe" }
$content = Join-Path $out 'content'
foreach ($p in (@($P_S, $P_A) + $relays)) {
    if (Test-Path $p.Dir) { Remove-Item -Recurse -Force $p.Dir }
}
foreach ($d in @($content, $tstate, (Join-Path $out 'tribler-dl'))) {
    if (Test-Path $d) { Remove-Item -Recurse -Force $d }
}
New-Item -ItemType Directory -Force -Path $confDir | Out-Null

# Etat Tribler vierge : aucun cache metainfo, aucune piece, aucun peer.
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

# ---------- Config Tribler isolee ----------
# - DiscoveryCommunity : bootstrappeur = A1 UNIQUEMENT (127.0.0.1:
#   17787). Tout le maillage est controle : T decouvre A2/A3/S via
#   les introduction-response de A1, ses circuits DATA ne peuvent
#   sortir que chez A1 (seul noeud EXIT_BT du pool) et ses circuits
#   RP_DOWNLOADER (hops+1 sauts) passent par A2/A3.
# - dht_discovery OBLIGATOIRE : c'est le `dht_provider` injecte dans
#   TriblerTunnelCommunity (composant Tunnel depend de DHTDiscovery).
# - libtorrent : dht/lsd/upnp/natpmp off -> aucune decouverte de pair
#   directe possible ; tout flux BT passe forcement par les circuits.
# - ipv8.interfaces : port libre (eviter collision avec un Tribler
#   utilisateur deja lance sur 8090).
$tApiPort = Get-FreeTcpPort
$tIpv8 = Get-FreeUdpPort
while ($tIpv8 -eq $tApiPort) { $tIpv8 = Get-FreeUdpPort }
$script:TApiPort = $tApiPort
$script:TApiKey = "interop$nonce"
$dlDir = Join-Path $out 'tribler-dl'
$conf = @{
    api = @{
        http_enabled = $true
        http_port = $tApiPort
        http_host = '127.0.0.1'
        https_enabled = $false
        key = $script:TApiKey
    }
    ipv8 = @{
        # DEBUG : on doit voir partir les introduction-request et ce que
        # pyipv8 fait de nos reponses (le coeur du diagnostic mesh).
        logger = @{ level = 'DEBUG' }
        # ip=127.0.0.1 (et non 0.0.0.0) : isole Tribler au loopback.
        # Les paquets sortants vers des IP WAN partent avec une source
        # loopback -> aucune reponse -> les pairs publics que le
        # bootstrapper EN DUR de TunnelCommunity (DISPERSY_BOOTSTRAPPER
        # dans components.py, non configurable) decouvrira mourront,
        # seuls les noeuds du maillage restent eligibles. Sans admin,
        # c'est le seul moyen de fermer la topologie.
        interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $tIpv8 } )
        walker_interval = 5.0
        overlays = @(
            # Bloc DiscoveryCommunity = copie du defaut pyipv8
            # (configuration.py) : les 3 strategies de marche sont
            # OBLIGATOIRES -- sans walkers Tribler ne decouvre personne.
            @{
                class = 'DiscoveryCommunity'
                key = 'anonymous id'
                walkers = @(
                    @{ strategy = 'RandomWalk'; peers = 20; init = @{ timeout = 3.0 } }
                    @{ strategy = 'RandomChurn'; peers = -1; init = @{ sample_size = 8; ping_interval = 10.0; inactive_time = 27.5; drop_time = 57.5 } }
                    @{ strategy = 'PeriodicSimilarity'; peers = -1; init = @{} }
                )
                # init explicite : DispersyBootstrapper exige ip/dns
                # en arguments (pas de defaut implicite). Topologie
                # controlee : seule l'ancre A1 est connue au depart ;
                # le reste du maillage arrive par introduction-response.
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
        download_defaults = @{ saveas = $dlDir }
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
# Sans BOM : json.load de Tribler rejette le BOM et repart en defauts.
[System.IO.File]::WriteAllText((Join-Path $confDir 'configuration.json'),
    ($conf | ConvertTo-Json -Depth 10), [System.Text.UTF8Encoding]::new($false))

$deadline = (Get-Date).AddMinutes($TimeoutMin)
try {
    # ---------- Phase 1 : maillage controle A1/A2/A3 + seeder ----------
    Log '=== Phase 1 : ancre A1 (EXIT_BT) + relais A2/A3 + seeder S ==='
    # A1 : racine du maillage (pas de bootstrap) + SEUL noeud EXIT_BT
    # -> tous les circuits DATA de Tribler sortiront chez A1, qui est
    # aussi le point d'introduction epingle de S : le peers-request
    # y est servi depuis `intro_point_for` (chemin local, pas de DHT).
    foreach ($p in (@($P_A, $P_S) + $relays)) {
        New-Item -ItemType Directory -Force -Path $p.Dir | Out-Null
    }
    # `ipv8.bootstrap.override` REMPLACE les bootstrappeurs publics
    # par defaut (contrairement a --bootstrap qui s'y AJOUTE) : mesh
    # ferme, aucun pair WAN ne doit entrer dans nos tables.
    # ATTENTION : `ConvertTo-Json -Depth` est OBLIGATOIRE ici — sans lui
    # `override` est serialise en chaine ("127.0.0.1:x") au lieu d'un
    # tableau, serde rejette le fichier entier et le daemon retombe sur
    # les defaults (bootstrap public, pas d'EXIT_BT) silencieusement.
    [System.IO.File]::WriteAllText((Join-Path $P_A.Dir 'configuration.json'),
        (@{ tunnel_community = @{ exitnode_enabled = $true; guards_enabled = [bool]$Guards };
            ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($P_A2.Ipv8)") };
                      # interface loopback : le maillage est ferme aussi
                      # en ENTREE — les noeuds publics qui nous connaissent
                      # de runs precedents pingent encore (le Network
                      # partage saturait a 30 pairs -> "trop de pairs").
                      interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $P_A.Ipv8 } );
                      # WAN force : sur loopback pur `destination_address`
                      # des intros est LAN -> my_estimated_wan jamais appris
                      # -> on_node_discovered refuse tout noeud (DHT muet,
                      # comme pyipv8). Sans ca aucun `dht_announce`.
                      estimated_wan = "127.0.0.1:$($P_A.Ipv8)" } } |
            ConvertTo-Json -Compress -Depth 5))
    Start-Daemon $P_A @()
    $kA = Wait-ApiKey $P_A.Dir; Wait-ApiUp $P_A.Api $kA; Assert-ConfigOk $P_A

    # Relais purs (flag RELAY de base) pour les circuits multi-hop
    # (RP_DOWNLOADER / IP_SEEDER = hops+1 sauts cote pyipv8).
    foreach ($p in $relays) {
        [System.IO.File]::WriteAllText((Join-Path $p.Dir 'configuration.json'),
            (@{ tunnel_community = @{ guards_enabled = [bool]$Guards };
                ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($P_A.Ipv8)") };
                          interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $p.Ipv8 } );
                          estimated_wan = "127.0.0.1:$($p.Ipv8)" } } |
                ConvertTo-Json -Compress -Depth 5))
        Start-Daemon $p @()
        $k = Wait-ApiKey $p.Dir; Wait-ApiUp $p.Api $k; Assert-ConfigOk $p
    }

    # S : bootstrap A1 uniquement + point d'introduction epingle = A1
    # (`required_ip` de create_introduction_point pyipv8).
    [System.IO.File]::WriteAllText((Join-Path $P_S.Dir 'configuration.json'),
        (@{ tunnel_community = @{ intro_point_peer = "127.0.0.1:$($P_A.Ipv8)"; guards_enabled = [bool]$Guards };
            ipv8 = @{ bootstrap = @{ override = @("127.0.0.1:$($P_A.Ipv8)") };
                      interfaces = @( @{ interface = 'UDPIPv4'; ip = '127.0.0.1'; port = $P_S.Ipv8 } );
                      estimated_wan = "127.0.0.1:$($P_S.Ipv8)" } } |
            ConvertTo-Json -Compress -Depth 5))
    Start-Daemon $P_S @()
    $kS = Wait-ApiKey $P_S.Dir; Wait-ApiUp $P_S.Api $kS; Assert-ConfigOk $P_S

    # Manifeste de run : versions, pids, ports, infohash, topologie —
    # tout ce qu'il faut pour corréler les logs d'un run donné.
    $manifest = [ordered]@{
        run_utc    = [datetime]::UtcNow.ToString('o')
        script     = 'interop_hidden_tribler_download.ps1'
        mode       = $(if ($DhtOnly) { 'DhtOnly' } else { 'Full' })
        daemon     = $daemon
        tribler    = $triblerExe
        infohash   = $ih
        lookup_dht = $lookup
        sha256_src = $srcHash
        hops       = $Hops
        guards     = [bool]$Guards
        outdir     = $out
        nodes      = @(
            @{ name = 'S';  api = $P_S.Api;  ipv8 = $P_S.Ipv8;  dir = $P_S.Dir;  pid = $procs['S'].Id;  role = 'seeder anonyme, intro point epingle -> A1' },
            @{ name = 'A1'; api = $P_A.Api;  ipv8 = $P_A.Ipv8;  dir = $P_A.Dir;  pid = $procs['A'].Id;  role = 'EXIT_BT + intro point' }
        ) + @($relays | ForEach-Object {
            @{ name = $_.Name; api = $_.Api; ipv8 = $_.Ipv8; dir = $_.Dir; pid = $procs[$_.Name].Id; role = 'relais' }
        })
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'manifest.json') -Encoding UTF8
    Log "manifeste ecrit : $(Join-Path $out 'manifest.json')"

    Api 'PUT' $P_S.Api '/downloads' $kS @{ torrent = $torrent; destination = $content;
        anon_hops = $Hops; safe_seeding = $true } | Out-Null
    $seeding = $false
    while ((Get-Date) -lt $deadline) {
        $d = Download-State $P_S.Api $kS $ih
        if ($d -and $d.status -eq 'SEEDING') { $seeding = $true; break }
        Start-Sleep -Seconds 3
    }
    Verdict $seeding 'S passe en SEEDING' "infohash=$ih"
    if (-not $seeding) { throw 'jamais SEEDING' }

    $ipOk = $false; $ipWait = (Get-Date).AddMinutes(6)
    while ((Get-Date) -lt $ipWait) {
        $nIp = @(Circuits $P_S.Api $kS 'IP_SEEDER').Count
        if ($nIp -ge 1) { $ipOk = $true; break }
        Start-Sleep -Seconds 5
    }
    Verdict $ipOk 'S : circuit(s) IP_SEEDER READY' "n=$nIp"

    # Propagation de l'annonce DHT (sonde = A, le reseau reel fait le reste).
    Log 'attente propagation de l annonce DHT (sonde A)...'
    $dhtOk = $false; $dhtWait = (Get-Date).AddMinutes(15)
    if ($dhtWait -gt $deadline) { $dhtWait = $deadline }
    while ((Get-Date) -lt $dhtWait) {
        try {
            $vA = Api 'GET' $P_A.Api "/ipv8/dht/values/$lookup" $kA $null 30
            $vS = Api 'GET' $P_S.Api "/ipv8/dht/values/$lookup" $kS $null 30
            Log ("DHT {0} : S={1} A={2} | A req={3} val={4} t={5:N1}s" -f
                $lookup.Substring(0, 12), @($vS.values).Count, @($vA.values).Count,
                $vA.debug.requests, $vA.debug.responses_with_values, $vA.debug.time)
            if (@($vA.values).Count -gt 0 -or @($vS.values).Count -gt 0) { $dhtOk = $true; break }
        } catch { Log "DHT lookup : $($_.Exception.Message)" }
        Start-Sleep -Seconds 20
    }
    Verdict $dhtOk 'DHT : annonce du point d introduction propagee'
    $nIp = @(Circuits $P_S.Api $kS 'IP_SEEDER').Count
    Log "points d'introduction de S : $nIp circuits IP_SEEDER"
    # Gate deterministe : pas de phase Tribler tant que l'annonce n'est
    # pas relue par un autre noeud (une annonce non propagee rendait le
    # verdict final ambigu — 'aucun lookup' vs 'pas d annonce').
    if (-not $dhtOk) { throw 'annonce DHT non propagee - Tribler non lance' }

    if ($DhtOnly) {
        # Run de collecte : le verdict minimal etait « store confirme
        # sur un noeud Rust + valeur relue par lookup » — la sonde
        # ci-dessus fait exactement ca (find_values depuis A1 != S).
        Log '=== -DhtOnly : collecte DHT terminee, phase Tribler ignoree ==='
        return
    }

    # ---------- Phase 2 : Tribler.exe reel ----------
    Log "=== Phase 2 : Tribler.exe -s (etat vierge : $tstate, ipv8 :$tIpv8, api :$tApiPort) ==="
    # Canal d'amorce : le bootstrapper de TriblerTunnelCommunity est le
    # dispersy public EN DUR (non configurable) — inatteignable depuis le
    # socket binde loopback. MAIS restore_exitnodes_from_disk() envoie un
    # introduction-request UDP direct a chaque adresse de
    # `exitnode_cache.dat` (format Network.snapshot() : 0x01 + IPv4 + port
    # BE par entree). On y ecrit A1 -> handshake TunnelCommunity ->
    # discovery de tout le maillage par introduction-response.
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

    $tApiUp = $false; $tWait = (Get-Date).AddSeconds(180)
    while ((Get-Date) -lt $tWait -and -not $tApiUp) {
        try { TApi 'GET' '/ipv8/overlays' 5 | Out-Null; $tApiUp = $true } catch { Start-Sleep -Seconds 2 }
    }
    Verdict $tApiUp 'Tribler : API REST en ligne'
    if (-not $tApiUp) { throw 'API Tribler jamais en ligne - voir tribler_stderr.log' }

    # Maillage controle : Tribler ne connait qu'A1 au depart puis
    # decouvre A2/A3/S par introduction-response. Il faut >= 3 pairs
    # tunnel (A1 exit + 2 relais pour les circuits 2-hop). Gate :
    # >= 3 pairs tunnel community dont >= 1 exit, ou 6 min max.
    $tPeers = $false; $nP = 0; $nTunnel = 0; $exits = 0; $apiErr = $null; $pWait = (Get-Date).AddMinutes(6)
    while ((Get-Date) -lt $pWait -and -not $tPeers) {
        try {
            $ov = (TApi 'GET' '/ipv8/overlays' 10).overlays
            $nP = ($ov | ForEach-Object { @($_.peers).Count } | Measure-Object -Sum).Sum
            $nTunnel = @($ov | Where-Object { $_.overlay_name -match 'Tunnel' } | ForEach-Object { @($_.peers).Count } | Measure-Object -Sum).Sum
            # flags pyipv8 : RELAY=1 EXIT_BT=2 EXIT_IPV8=4 SPEED_TEST=8
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
    if (-not $tPeers -and $nP -gt 0) {
        $tPeers = $true
        Log "ATTENTION : pool tunnel mince (tunnel_peers=$nTunnel exits=$exits) -- tentative quand meme"
    }
    Verdict ($nP -gt 0) 'Tribler : bootstrap maillage controle' "peers=$nP tunnel=$nTunnel exits=$exits"

    # ---------- Phase 3 : download anonyme chez Tribler ----------
    Log '=== Phase 3 : PUT torrent (binaire) chez Tribler ==='
    $dest = [System.Uri]::EscapeDataString($dlDir)
    $putUri = "http://127.0.0.1:$tApiPort/api/downloads?anon_hops=$Hops&safe_seeding=true&destination=$dest"
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
    Verdict $addOk 'Tribler : download anonyme accepte' "started=$($tAdd.started) infohash=$($tAdd.infohash)"
    if (-not $addOk) { throw 'ajout download refuse' }

    # ---------- Phase 4 : attente + collecte de preuves ----------
    $sLog = Join-Path $P_S.Dir 'logs\onionbit.log'
    $seen = @{ e2e = $false; linked = $false; utp = $false; rp = $false; bytes = $false; dhtT = $false }
    $ds0 = Download-State $P_S.Api $kS $ih
    $ulBefore = if ($ds0) { [int64]$ds0.all_time_upload } else { 0 }
    Log "S : upload baseline=$ulBefore octets"

    $done = $false
    while ((Get-Date) -lt $deadline -and -not $done) {
        Start-Sleep -Seconds 10
        # -- Tribler : download + circuits
        $dt = TDownload-State $ih
        $tc = TCircuits
        $nRp = @($tc | Where-Object { $_.type -eq 'RP_DOWNLOADER' }).Count
        $nData = @($tc | Where-Object { $_.type -eq 'DATA' }).Count
        if ($dt) {
            $dlb = [int64]$dt.all_time_download
            Log ("T : progress={0:P1} dl={1}B sp={2}B/s peers={3} st={4} | circuits RPDL={5} DATA={6} total={7}" -f
                $dt.progress, $dlb, $dt.speed_down, $dt.num_peers, $dt.status, $nRp, $nData, @($tc).Count)
            if ($dlb -gt 0 -and -not $seen.bytes) {
                $seen.bytes = $true
                Verdict $true 'Tribler recoit des octets via le hidden service' "dl=$dlb peers=$($dt.num_peers)"
            }
            if ($dt.progress -ge 1.0) { $done = $true }
        }
        if ($nRp -gt 0 -and -not $seen.rp) {
            $seen.rp = $true
            Verdict $true 'Tribler : circuit(s) RP_DOWNLOADER' "n=$nRp"
        }
        # -- Tribler : sonde DHT (stockage local de la valeur recherchee)
        if (-not $seen.dhtT) {
            try {
                $vt = TApi 'GET' "/ipv8/dht/values/$lookup" 15
                if (@($vt.values).Count -gt 0) {
                    $seen.dhtT = $true
                    Verdict $true 'Tribler : lookup DHT a trouve le point d introduction' "n=$(@($vt.values).Count)"
                }
            } catch {}
        }
        # -- S : e2e + uTP
        if (-not $seen.e2e) {
            $m = @(LogGrep $sLog 'create-e2e')
            if ($m.Count -gt 0) { $seen.e2e = $true; Verdict $true 'S : create-e2e recu (Tribler->Rust)' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
        if (-not $seen.linked) {
            $m = @(LogGrep $sLog 'linked-e2e')
            if ($m.Count -gt 0) { $seen.linked = $true; Verdict $true 'S : linked-e2e (e2e etabli)' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
        if (-not $seen.utp) {
            $m = @(LogGrep $sLog 'utp_stream\{|utp_listen_custom')
            if ($m.Count -gt 0) { $seen.utp = $true; Verdict $true 'S : stream uTP accepte sur lane e2e' $m[0].Trim().Substring(0, [Math]::Min(140, $m[0].Trim().Length)) }
        }
    }

    # ---------- Phase 5 : integrite ----------
    $dt = TDownload-State $ih
    $dsE = Download-State $P_S.Api $kS $ih
    $sUp = if ($dsE) { [int64]$dsE.all_time_upload } else { 0 }
    $ulDelta = $sUp - $ulBefore
    Verdict ($ulDelta -gt 0) 'S upload (delta all_time_upload)' "delta=$ulDelta (avant=$ulBefore apres=$sUp)"
    if ($dt) {
        Verdict ($dt.progress -ge 1.0) 'Tribler : telechargement termine (pieces verifiees)' ("progress={0:P1} peers={1} dl={2}" -f $dt.progress, $dt.num_peers, $dt.all_time_download)
    }
    if ($done) {
        # Le downloader peut garder le fichier ouvert quelques secondes
        # apres progress=1.0 (flush/close asynchrone) : retry borne.
        $f = $null; $h = $null
        $hashDeadline = (Get-Date).AddSeconds(30)
        while ((Get-Date) -lt $hashDeadline) {
            if (-not $f) {
                $f = Get-ChildItem -Recurse -Filter 'donnee.bin' $dlDir -ErrorAction SilentlyContinue | Select-Object -First 1
            }
            if ($f) {
                try {
                    $h = (Get-FileHash -Algorithm SHA256 $f.FullName).Hash.ToLower()
                    break
                } catch { Start-Sleep -Seconds 1 }
            } else { Start-Sleep -Seconds 1 }
        }
        if ($h) {
            Verdict ($h -eq $srcHash) 'integrite : SHA256 du fichier recu' "got=$h want=$srcHash"
        } elseif ($f) {
            Verdict $false 'integrite : SHA256 du fichier recu' "donnee.bin encore verrouille apres 30 s"
        } else { Verdict $false 'integrite : SHA256 du fichier recu' 'donnee.bin introuvable dans la destination' }
    }
}
finally {
    foreach ($p in (@($P_S, $P_A) + $relays)) {
        try {
            $k = ApiKey $p.Dir
            if ($k) {
                foreach ($ep in @('/downloads','/ipv8/tunnel/circuits','/ipv8/tunnel/relays','/ipv8/tunnel/exits','/ipv8/tunnel/swarms','/ipv8/tunnel/peers','/ipv8/tunnel/guards')) {
                    $name = ($p.Name + ($ep -replace '/','_') + '.json')
                    try { Api 'GET' $p.Api $ep $k $null 5 | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $rep $name) -Encoding UTF8 } catch {}
                }
                if ($Guards) { Assert-Guards $p $k }
            }
        } catch {}
        if ($procs.ContainsKey($p.Name) -and -not $procs[$p.Name].HasExited) {
            try { $procs[$p.Name].Kill(); $procs[$p.Name].WaitForExit() } catch {}
        }
        $log = Join-Path $p.Dir 'logs\onionbit.log'
        if (Test-Path $log) { Copy-Item $log (Join-Path $rep ("tribler_{0}.log" -f $p.Name)) -Force }
    }
    # Instantanes Tribler avant extinction.
    try { TCircuits | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $rep 'T_circuits.json') -Encoding UTF8 } catch {}
    try { TApi 'GET' "/downloads?infohash=$ih" 10 | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $rep 'T_download_detail.json') -Encoding UTF8 } catch {}
    try { TApi 'GET' '/ipv8/overlays' 10 | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $rep 'T_overlays.json') -Encoding UTF8 } catch {}
    if ($procs.ContainsKey('T') -and -not $procs['T'].HasExited) {
        try { $procs['T'].Kill(); $procs['T'].WaitForExit() } catch {}
    }
    # Logs bruts de Tribler (core + stderr/stdout).
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
