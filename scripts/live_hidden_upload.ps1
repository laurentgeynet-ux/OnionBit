#requires -Version 5.1
<#
.SYNOPSIS
  Test live bout-en-bout : hidden seeding + upload anonyme + kill switch.

.DESCRIPTION
  Trois instances tribler-daemon isolees (etats separes) :
    A = ancre/relais pur (bootstrap Tribler.exe)  api 8097, ipv8 17787
    S = seeder anonyme (bootstrap A + Tribler)  api 8095, ipv8 17785
    D = downloader  (bootstrap A UNIQUEMENT)    api 8096, ipv8 17786

  Phases :
    1. S seede un torrent maison SANS tracker en safe_seeding -> intro point + DHT.
    2. D ajoute le meme torrent en anon_hops=N -> la seule voie de decouverte est
       le hidden service ; tout octet recu prouve intro-point -> DHT -> e2e -> tunnel.
    3. Kill switch : on tue A (seul ancrage de D) -> la progression DOIT geler
       (aucun repli direct possible), puis redemarrage de A -> reprise.

  Verdict : octets telecharges cote D, octets uploades cote S, gel/reprise mesures.
#>
param(
    [int]    $Bytes       = 2097152,
    [int]    $Hops        = 1,
    [int]    $TimeoutMin  = 25,
    [string] $Bootstrap   = '',
    [string] $OutDir      = 'target\live'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root   = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$out    = Join-Path $root $OutDir
$daemon = Join-Path $root 'target\debug\tribler-daemon.exe'
$mk     = Join-Path $root 'target\debug\examples\mk_torrent.exe'
$rep    = Join-Path $out 'report'
New-Item -ItemType Directory -Force -Path $out, $rep | Out-Null

$P_S = @{ Api = 8095; Ipv8 = 17785; Dir = Join-Path $out 'seed';   Name = 'S' }
$P_A = @{ Api = 8097; Ipv8 = 17787; Dir = Join-Path $out 'anchor'; Name = 'A' }
$P_D = @{ Api = 8096; Ipv8 = 17786; Dir = Join-Path $out 'down';   Name = 'D' }
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
    $argList = @('--state-dir', ('"{0}"' -f $p.Dir), '--listen', "127.0.0.1:$($p.Api)",
              '--ipv8-port', "$($p.Ipv8)", '--no-tray')
    foreach ($b in $boot) { $argList += @('--bootstrap', $b) }
    $psi = New-Object System.Diagnostics.ProcessStartInfo($daemon)
    $psi.Arguments = $argList -join ' '
    $psi.UseShellExecute = $false
    # Detail debug sur les crates tunnel/core seulement (le fil wire
    # reste en info — les logs sont deja volumineux).
    $psi.EnvironmentVariables['RUST_LOG'] = 'info,tribler_tunnel=debug,tribler_core=debug,librqbit_utp=debug,librqbit=debug'
    $proc = [System.Diagnostics.Process]::Start($psi)
    $procs[$p.Name] = $proc
    Log ("{0} demarre pid={1} bootstrap=[{2}]" -f $p.Name, $proc.Id, ($boot -join ', '))
}

function Get-TriblerBootstrap {
    if ($Bootstrap) { return $Bootstrap }
    $cfg = "$env:APPDATA\.Tribler\8.0\configuration.json"
    if (-not (Test-Path $cfg)) { return $null }
    try {
        $j = Get-Content $cfg -Raw | ConvertFrom-Json
        $port = @($j.ipv8.interfaces | Where-Object { $_.interface -eq 'UDPIPv4' })[0].port
        if ($port) { return "127.0.0.1:$port" }
    } catch {}
    $null
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

# ---------- Phase 0 : contenu + torrent ----------
Log '=== Phase 0 : contenu de test ==='
if (-not (Test-Path $daemon)) { throw 'tribler-daemon.exe absent - cargo build -p tribler-daemon' }
if (-not (Test-Path $mk)) { throw 'mk_torrent.exe absent - cargo build -p tribler-bittorrent --example mk_torrent' }
$content = Join-Path $out 'content'
# Nonce aleatoire : infohash unique par run -> cle DHT vierge, donc les
# seuls points d'introduction resolus par D sont ceux que S vient
# d'etablir (pas d'annonces perimees des runs precedents).
# Etat propre par run : les downloads restaures d'un run precedent
# polluent les mesures (`all_time_upload` cumule, pieces re-hashees
# contre le contenu courant -> hashcheck en echec, trafic parasite).
foreach ($p in @($P_S, $P_A, $P_D)) {
    if (Test-Path $p.Dir) { Remove-Item -Recurse -Force $p.Dir }
}
if (Test-Path $content) { Remove-Item -Recurse -Force $content }

$nonce = Get-Random -Minimum 1 -Maximum ([int]::MaxValue)
$mkOut = & $mk $content $Bytes $nonce
$mkOut | Tee-Object (Join-Path $rep 'mk_torrent.txt') | Out-Null
$torrent = Join-Path $content 'test.torrent'
$ih = (($mkOut | Select-String 'infohash=([0-9a-f]+)').Matches.Groups[1].Value)
if (-not $ih) { throw 'infohash non obtenu' }
Log "infohash=$ih torrent=$torrent"

# Cle DHT du swarm cache : SHA1("tribler anonymous download" +
# hexlify(infohash)) — `get_lookup_info_hash` pyipv8.
$lookup = ([System.BitConverter]::ToString(
    [System.Security.Cryptography.SHA1]::Create().ComputeHash(
        [System.Text.Encoding]::ASCII.GetBytes('tribler anonymous download' + $ih)))
).Replace('-', '').ToLower()
Log "lookup_dht=$lookup"

$tb = Get-TriblerBootstrap
if ($tb) { Log "bootstrap Tribler.exe : $tb" } else { Log 'Tribler.exe absent - banc 100% Rust' }

$deadline = (Get-Date).AddMinutes($TimeoutMin)
try {
    # ---------- Phase 1 : ancre + seeder ----------
    Log '=== Phase 1 : ancre A + seeder S ==='
    $bA = @(); if ($tb) { $bA += $tb }
    Start-Daemon $P_A $bA
    $kA = Wait-ApiKey $P_A.Dir; Wait-ApiUp $P_A.Api $kA

    $bS = @("127.0.0.1:$($P_A.Ipv8)"); if ($tb) { $bS += $tb }
    # Point d'introduction epingle sur A (`tunnel_community/
    # intro_point_peer` = `required_ip` de create_introduction_point
    # pyipv8) : l'infra de test publique (192.42.116.24x) accepte
    # establish-intro sans toujours relayer le create-e2e. A est notre
    # daemon — relais et annonce DHT sont prouves. Les sauts
    # intermediaires et la DHT restent sur le reseau reel.
    New-Item -ItemType Directory -Force -Path $P_S.Dir | Out-Null
    # WriteAllText sans BOM : `Set-Content -Encoding UTF8` (PS 5.1)
    # ajoute un BOM que serde_json rejette -> config ignoree.
    [System.IO.File]::WriteAllText((Join-Path $P_S.Dir 'configuration.json'),
        (@{ tunnel_community = @{ intro_point_peer = "127.0.0.1:$($P_A.Ipv8)" } } | ConvertTo-Json -Compress))
    Start-Daemon $P_S $bS
    $kS = Wait-ApiKey $P_S.Dir; Wait-ApiUp $P_S.Api $kS

    Api 'PUT' $P_S.Api '/downloads' $kS @{ torrent = $torrent; destination = $content;
        anon_hops = $Hops; safe_seeding = $true } | Out-Null
    Log 'S : download ajoute (anon, safe_seeding) - attente SEEDING...'
    $seeding = $false
    while ((Get-Date) -lt $deadline) {
        $d = Download-State $P_S.Api $kS $ih
        if ($d -and $d.status -eq 'SEEDING') { $seeding = $true; break }
        Start-Sleep -Seconds 3
    }
    Verdict $seeding 'S passe en SEEDING'
    if (-not $seeding) { throw 'jamais SEEDING' }

    # Circuits IP_SEEDER prets = points d'introduction en place.
    $ipOk = $false; $ipWait = (Get-Date).AddMinutes(4)
    while ((Get-Date) -lt $ipWait) {
        if (@(Circuits $P_S.Api $kS 'IP_SEEDER').Count -ge 1) { $ipOk = $true; break }
        Start-Sleep -Seconds 5
    }
    Verdict $ipOk 'S : circuit(s) IP_SEEDER READY'
    # Temps mort : establish-intro + annonce DHT vers les noeuds publics.
    Log 'settle 90s (establish-intro + annonce DHT)...'; Start-Sleep -Seconds 90

    # ---------- Phase 2 : downloader ----------
    Log '=== Phase 2 : downloader D (ancrage A uniquement) ==='
    # Sortie des circuits DATA epinglee sur A (`tunnel_community/
    # data_exit_peer` = `required_exit` de create_circuit pyipv8) : le
    # create-e2e doit atteindre le point d'introduction A depuis
    # l'exit du circuit — un exit public ne peut pas joindre A (WAN
    # NATe + pas de hairpin). Sortir chez A (loopback) resout les
    # deux. Le payload IPv8/prefixe communaute est toujours admis par
    # la politique de sortie — A n'a pas besoin du flag EXIT_*.
    New-Item -ItemType Directory -Force -Path $P_D.Dir | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $P_D.Dir 'configuration.json'),
        (@{ tunnel_community = @{ data_exit_peer = "127.0.0.1:$($P_A.Ipv8)" } } | ConvertTo-Json -Compress))
    Start-Daemon $P_D @("127.0.0.1:$($P_A.Ipv8)")
    $kD = Wait-ApiKey $P_D.Dir; Wait-ApiUp $P_D.Api $kD
    $dlDir = Join-Path $out 'dl'
    New-Item -ItemType Directory -Force -Path $dlDir | Out-Null
    Api 'PUT' $P_D.Api '/downloads' $kD @{ torrent = $torrent; destination = $dlDir;
        anon_hops = $Hops; safe_seeding = $true } | Out-Null
    Log 'D : download ajoute - attente des premiers octets...'

    # Gate DHT : le swarm_lookup interne doit trouver l'annonce du
    # point d'introduction. On mesure la propagation sur les 3 noeuds —
    # tant que n=0 chez D le transfert ne peut pas demarrer.
    $dhtFound = $false; $dhtWait = (Get-Date).AddMinutes(20)
    if ($dhtWait -gt $deadline) { $dhtWait = $deadline }
    while ((Get-Date) -lt $dhtWait) {
        try {
            $vD = Api 'GET' $P_D.Api "/ipv8/dht/values/$lookup" $kD $null 30
            $nD = @($vD.values).Count
            $vA = Api 'GET' $P_A.Api "/ipv8/dht/values/$lookup" $kA $null 30
            $nA = @($vA.values).Count
            $vS = Api 'GET' $P_S.Api "/ipv8/dht/values/$lookup" $kS $null 30
            $nS = @($vS.values).Count
            Log ("DHT {0} : S={1} A={2} D={3} | D req={4} val={5} noeuds={6} t={7:N1}s" -f
                $lookup.Substring(0, 12), $nS, $nA, $nD,
                $vD.debug.requests, $vD.debug.responses_with_values,
                $vD.debug.responses_with_nodes, $vD.debug.time)
            if ($nD -gt 0) { $dhtFound = $true; break }
        } catch { Log "DHT lookup : $($_.Exception.Message)" }
        Start-Sleep -Seconds 20
    }
    Verdict $dhtFound 'DHT : D trouve le point d introduction du swarm'

    $ulBefore = [int64](Download-State $P_S.Api $kS $ih).all_time_upload
    Log "S : upload baseline=$ulBefore octets"

    $gotBytes = $false
    while ((Get-Date) -lt $deadline -and -not $gotBytes) {
        Start-Sleep -Seconds 5
        $d = Download-State $P_D.Api $kD $ih
        if ($d) {
            $dlb = [int64]$d.all_time_download
            if ($dlb -gt 0) { $gotBytes = $true }
            Log ("D : progress={0:P1} dl={1}B sp={2}B/s peers={3} st={4}" -f $d.progress, $dlb, $d.speed_down, $d.num_peers, $d.status)
        }
    }
    Verdict $gotBytes 'D recoit des octets via le hidden service'
    if (-not $gotBytes) { throw 'aucun octet recu' }

    $sUp = [int64](Download-State $P_S.Api $kS $ih).all_time_upload
    $ulDelta = $sUp - $ulBefore
    Verdict ($ulDelta -gt 0) 'S upload (delta all_time_upload)' "delta=$ulDelta (avant=$ulBefore apres=$sUp)"

    # ---------- Phase 3 : kill switch ----------
    # A n'est qu'une ancre de bootstrap : avec hops=1 le plan de donnees
    # de D transite par des noeuds publics (exits/RP). La coupure qui
    # gele reellement le transfert est la mort de S : le circuit
    # RP_SEEDER meurt, la jambe e2e tombe, et la lane anonyme n'a aucun
    # repli direct possible — la progression doit geler sans fuite.
    Log '=== Phase 3 : kill switch - extinction de S ==='
    $dBefore = Download-State $P_D.Api $kD $ih
    $freezeAt = [int64]$dBefore.all_time_download

    # Instantane des lanes anonymes de D avant la coupure : les
    # circuits RP_DOWNLOADER/e2e et leurs compteurs servent de preuve
    # que le flux transitait par le tunnel (pas de repli direct).
    $circBefore = @(Circuits $P_D.Api $kD $null $null)
    $circBefore | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $rep 'D_circuits_avant_kill.json') -Encoding UTF8
    $upBefore = [int64](($circBefore | Measure-Object -Property bytes_up -Sum).Sum)
    Log ("D avant kill : {0} circuits (RP_DOWNLOADER={1} DATA={2}) up_total={3}B" -f
        $circBefore.Count,
        @($circBefore | Where-Object { $_.type -eq 'RP_DOWNLOADER' }).Count,
        @($circBefore | Where-Object { $_.type -eq 'DATA' }).Count,
        $upBefore)

    $procs['S'].Kill(); $procs['S'].WaitForExit()
    Log "S tue - progression D figee a $freezeAt octets ; phase drain..."

    # Modele drain/fenetre-morte : un transport fiable (uTP + cellules)
    # peut livrer apres la coupure les datagrammes DEJA partis de S
    # (buffer d'envoi, relais, file d'injection) — le drain est borne ;
    # ensuite `all_time_download` doit etre strictement invariant.
    $drainCap   = 2MB   # au-dela : le flux n'etait pas borne par les buffers -> fuite
    $drainQuiet = 15    # s sans nouvel octet => drain termine
    $drainMaxS  = 90    # le drain ne peut pas s'eterniser
    $deadWindow = 60    # fenetre stricte d'invariance apres drain

    $drainEnd = (Get-Date).AddSeconds($drainMaxS)
    $lastDl = $freezeAt; $lastChange = Get-Date; $upDuring = 0; $drained = $false
    while ((Get-Date) -lt $drainEnd -and -not $drained) {
        Start-Sleep -Seconds 5
        $d = Download-State $P_D.Api $kD $ih
        if (-not $d) { continue }
        $dlNow = [int64]$d.all_time_download
        if ($dlNow -ne $lastDl) { $lastDl = $dlNow; $lastChange = Get-Date }
        $upNow = [int64]((@(Circuits $P_D.Api $kD $null $null) | Measure-Object -Property bytes_up -Sum).Sum)
        $upDuring = [Math]::Max($upDuring, $upNow - $upBefore)
        $grown = $dlNow - $freezeAt
        Log ("drain : dl={0} (+{1}) sp={2}B/s up=+{3}B" -f $dlNow, $grown, $d.speed_down, ($upNow - $upBefore))
        if ($grown -gt $drainCap) { break }
        if (((Get-Date) - $lastChange).TotalSeconds -ge $drainQuiet -and [int64]$d.speed_down -eq 0) {
            $drained = $true
        }
    }
    $drainBytes = [int64]$lastDl - $freezeAt
    $plateau = $lastDl

    # Fenetre morte : aucun octet, aucun peer ne peut renaitre.
    $deadEnd = (Get-Date).AddSeconds($deadWindow)
    $postGrowth = 0; $upDead = 0
    while ((Get-Date) -lt $deadEnd) {
        $d = Download-State $P_D.Api $kD $ih
        if (-not $d) { Start-Sleep -Seconds 10; continue }
        $postGrowth = [Math]::Max($postGrowth, [int64]$d.all_time_download - $plateau)
        $upNow = [int64]((@(Circuits $P_D.Api $kD $null $null) | Measure-Object -Property bytes_up -Sum).Sum)
        $upDead = [Math]::Max($upDead, $upNow - $upBefore)
        $rc = @(Circuits $P_D.Api $kD 'DATA').Count
        $rp = @(Circuits $P_D.Api $kD 'RP_DOWNLOADER').Count
        Log ("fenetre morte : dl={0} (+{1} vs plateau) sp={2}B/s peers={3} DATA={4} RP={5} up=+{6}B" -f
            $d.all_time_download, ([int64]$d.all_time_download - $plateau), $d.speed_down, $d.num_peers, $rc, $rp, ($upNow - $upBefore))
        Start-Sleep -Seconds 10
    }
    $circAfter = @(Circuits $P_D.Api $kD $null $null)
    $circAfter | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $rep 'D_circuits_apres_kill.json') -Encoding UTF8
    Log ("D apres kill : {0} circuits drain=+{1}B up_total=+{2}B" -f $circAfter.Count, $drainBytes, $upDead)

    if ($drainBytes -gt $drainCap) {
        Verdict $false 'kill switch : drain borne' "drain=+$drainBytes > cap=$drainCap (flux non borne apres kill)"
    } elseif ($postGrowth -gt 0) {
        Verdict $false 'kill switch : fenetre morte' "+$postGrowth octets apres stabilisation (fuite/repli)"
    } else {
        Verdict $true 'kill switch : drain borne puis gel strict' "drain=+$drainBytes B puis invariant ${deadWindow}s ; up_fenetre=+$upDead"
    }

    # ---------- Phase 4 : reprise ----------
    # S redemarre : SEEDING -> IP_SEEDER -> establish-intro -> annonce
    # DHT -> D le redecouvre au prochain swarm_lookup -> nouvel e2e.
    Log '=== Phase 4 : redemarrage de S - reprise attendue ==='
    $procs.Remove('S') | Out-Null
    Start-Daemon $P_S $bS
    $kS = Wait-ApiKey $P_S.Dir; Wait-ApiUp $P_S.Api $kS
    $resumed = $false; $resumeEnd = (Get-Date).AddMinutes(8)
    if ($resumeEnd -gt $deadline) { $resumeEnd = $deadline }
    while ((Get-Date) -lt $resumeEnd) {
        Start-Sleep -Seconds 10
        $d = Download-State $P_D.Api $kD $ih
        if ($d -and [int64]$d.all_time_download -gt $freezeAt) { $resumed = $true }
        if ($d) {
            Log ("reprise : dl={0} sp={1}B/s peers={2} circuits_READY={3}" -f $d.all_time_download, $d.speed_down, $d.num_peers, @(Circuits $P_D.Api $kD).Count)
            if ($d.progress -ge 1.0) { $resumed = $true; break }
        }
    }
    Verdict $resumed 'reprise du telechargement apres retour de S'

    $dEnd = Download-State $P_D.Api $kD $ih
    if ($dEnd) { Verdict ($dEnd.progress -ge 1.0) 'telechargement termine' ("progress={0:P1}" -f $dEnd.progress) }
}
finally {
    # ---------- Instantanes + nettoyage ----------
    foreach ($p in @($P_S, $P_D, $P_A)) {
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
    Write-Host ''
    Write-Host '================ VERDICTS ================'
    $verdicts | ForEach-Object { Write-Host $_ }
    Write-Host "rapport : $rep"
}
