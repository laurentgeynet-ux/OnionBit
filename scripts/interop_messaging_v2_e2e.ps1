# This file is part of OnionBit.
#
# Copyright (C) 2026 Laurent Geynet
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Banc MG-13 (ADR-0019, etape 69) : cycle applicatif messagerie v2 reel
# entre TROIS demons OnionBit independants (processus, state dirs,
# sockets distincts) interconnectes par un mesh loopback ancre/relai/
# exit (meme convention que interop_messaging_e2e.ps1).
#
# Sequence assertee :
#   stats -> liaisons de contacts A-B, A-C, B-C (invite exige un
#   contact actif) -> A cree un groupe {B,C} -> B/C voient la conv
#   'invited' -> accept -> fan-out A->B,C et B->A,C -> piece jointe
#   reelle (fichier -> attach -> accept -> download anonyme -> SHA-256
#   identique) -> oracle du sel (meme fichier, deux ih) -> C leave ->
#   roster 'left' chez A -> cleanup.
#
# Usage :
#   pwsh -File scripts/interop_messaging_v2_e2e.ps1
#   pwsh -File scripts/interop_messaging_v2_e2e.ps1 -MessagingHops 2
#
# Preconditions : `cargo build -p onionbit-daemon` a jour.
# Artefacts : $OutDir/{manifest.json, console.log, <node>/...}.

[CmdletBinding()]
param(
    [string]$Daemon = 'target\debug\onionbit-daemon.exe',
    [string]$OutDir = ("target\mg13-{0}" -f (Get-Date -Format 'yyyyMMdd-HHmmss')),
    [string]$BaseAddr = '127.0.0.1',
    [int]$MessagingHops = 1
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

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$consoleLog = Join-Path $OutDir 'console.log'

function Log([string]$msg) {
    $line = '[{0}] {1}' -f (Get-Date -Format 'HH:mm:ss.fff'), $msg
    Write-Host $line
    Add-Content -Path $consoleLog -Value $line
}

$results = [System.Collections.ArrayList]::new()
function Step([string]$name, [string]$detail, [bool]$ok = $true) {
    $results.Add(@{ step = $name; ok = $ok; detail = $detail; ts = (Get-Date).ToString('o') }) | Out-Null
    if ($ok) { Log ("OK   {0} - {1}" -f $name, $detail) } else { throw "FAIL {0} - {1}" -f $name, $detail }
}

function Wait-For([string]$what, [scriptblock]$probe, [int]$timeoutSec = 60) {
    $deadline = (Get-Date).AddSeconds($timeoutSec)
    while ((Get-Date) -lt $deadline) {
        $v = & $probe
        if ($v) { return $v }
        Start-Sleep -Milliseconds 400
    }
    $last = try { & $probe | ConvertTo-Json -Depth 4 -Compress } catch { 'n/a' }
    throw "timeout ${timeoutSec}s : $what (dernier=$last)"
}

function ApiGet($node, [string]$path) {
    Invoke-RestMethod -Uri "http://$($node.Api)/api$path" -Headers @{ 'X-Api-Key' = $node.Key } -TimeoutSec 15
}
function ApiGetStatus($node, [string]$path) {
    try {
        Invoke-RestMethod -Uri "http://$($node.Api)/api$path" -Headers @{ 'X-Api-Key' = $node.Key } -TimeoutSec 5 | Out-Null
        return 200
    } catch {
        if ($_.Exception.Response) { return [int]$_.Exception.Response.StatusCode }
        return -1
    }
}
function ApiPost($node, [string]$path, $body) {
    $json = $body | ConvertTo-Json -Compress
    Invoke-RestMethod -Uri "http://$($node.Api)/api$path" -Method Post `
        -Headers @{ 'X-Api-Key' = $node.Key; 'Content-Type' = 'application/json' } `
        -Body $json -TimeoutSec 60
}
function ApiDelete($node, [string]$path, $body) {
    $json = $body | ConvertTo-Json -Compress
    Invoke-RestMethod -Uri "http://$($node.Api)/api$path" -Method Delete `
        -Headers @{ 'X-Api-Key' = $node.Key; 'Content-Type' = 'application/json' } `
        -Body $json -TimeoutSec 30
}

function Stop-OnionBit($p) {
    if ($p -and -not $p.HasExited) {
        try { Stop-Process -Id $p.Id -Force -ErrorAction Stop; $p.WaitForExit() | Out-Null } catch { }
    }
}

function Start-OnionBit($node) {
    $p = Start-Process -FilePath (Resolve-Path $Daemon).Path -PassThru -NoNewWindow `
        -ArgumentList ('--state-dir "{0}" --listen {1} --no-tray' -f $node.Dir, $node.Api) `
        -RedirectStandardOutput (Join-Path $OutDir "$($node.Name).out.log") `
        -RedirectStandardError  (Join-Path $OutDir "$($node.Name).err.log")
    $node.Proc = $p
    Log ("{0} demarre pid={1} api={2} ipv8={3}" -f $node.Name, $p.Id, $node.Api, $node.Ipv8)
    return $p
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
        Start-Sleep -Milliseconds 400
    }
    throw "cle API absente dans $dir"
}

# Noeud mesh : config `configuration.json` du state dir (le demon y
# fusionne ses cles generees -- api.key, identite ipv8).
function New-Node([string]$name, [int]$ipv8, [int]$api, [bool]$exit, [string[]]$boot) {
    $dir = Join-Path (Resolve-Path $OutDir) $name
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $ipv8addr = "$BaseAddr`:$ipv8"
    $cfg = @{
        tunnel_community = @{
            enabled           = $true
            exitnode_enabled  = $exit
            messaging_enabled = $true
            messaging_hops    = $MessagingHops
            min_circuits      = 2
            # Banc controle : exit et points d'introduction epingles
            # sur R (required_exit / required_ip de pyipv8).
            data_exit_peer    = "$BaseAddr`:29500"
            intro_point_peer  = "$BaseAddr`:29500"
        }
        ipv8 = @{
            bootstrap     = @{ override = @($boot) }
            interfaces    = @( @{ interface = 'UDPIPv4'; ip = $BaseAddr; port = $ipv8 } )
            estimated_wan = $ipv8addr
        }
        dht_discovery = @{ enabled = $true }
        libtorrent    = @{ port = 0; download_defaults = @{ anonymity_enabled = $true; number_hops = 1 } }
    }
    [System.IO.File]::WriteAllText((Join-Path $dir 'configuration.json'),
        ($cfg | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))
    return [pscustomobject]@{
        Name = $name; Ipv8 = $ipv8addr; Api = "$BaseAddr`:$api"
        Dir = $dir; Proc = $null; Key = $null
    }
}

function ContactState($node, [string]$pk) {
    $c = (@((ApiGet $node '/messaging/contacts').contacts | Where-Object { $_.public_key -eq $pk }) | Select-Object -First 1)
    if ($c) { $c.state } else { $null }
}
function IsBound($node, [string]$pk) {
    @((ApiGet $node '/messaging/contacts').contacts | Where-Object { $_.public_key -eq $pk -and $null -ne $_.circuit_id }).Count -gt 0
}
function ConvList($node) { (ApiGet $node '/messaging/conversations').conversations }
function ConvById($node, [string]$conv) {
    (@(ConvList $node) | Where-Object { $_.conv_id -eq $conv }) | Select-Object -First 1
}
function ConvMessages($node, [string]$conv) {
    (ApiGet $node "/messaging/conversations/$conv/messages?limit=200").messages
}
function ConvAttachs($node, [string]$conv) {
    (ApiGet $node "/messaging/conversations/$conv/attachments").attachments
}

# Etablit la liaison + le consentement initiator -> peer. Le
# connect peut etre long (annonce de presence DHT + points
# d'introduction epingles — 1 a ~250 s observes selon l'ordre).
function Link-Pair($from, $to, [string]$pkTo, [string]$pkFrom) {
    Wait-For "connect $($from.Name) -> $($to.Name)" {
        try { ApiPost $from '/messaging/contacts/connect' @{ public_key = $pkTo } } catch { $null }
    } 360 | Out-Null
    # Le premier send porte le hello — la demande pending apparait
    # chez le destinataire.
    ApiPost $from "/messaging/contacts/$pkTo/messages" @{ body = "mg13-handshake-$($from.Name)-$($to.Name)" } | Out-Null
    Wait-For "pending $($to.Name) <- $($from.Name)" {
        if ((ContactState $to $pkFrom) -eq 'pending') { $true } else { $null }
    } 120 | Out-Null
    ApiPost $to "/messaging/contacts/$pkFrom/accept" @{} | Out-Null
    Wait-For "active $($to.Name) <- $($from.Name)" {
        if ((ContactState $to $pkFrom) -eq 'active') { $true } else { $null }
    } 60 | Out-Null
}

# Logs demons : debug volontaire (diagnostic du chemin tunnel/e2e).
$env:RUST_LOG = 'onionbit_tunnel=debug,onionbit_core=debug,onionbit_ipv8=debug,onionbit_messaging=debug,onionbit_api=debug,info'

$R = $M = $A = $B = $C = $null
$pkA = $pkB = $pkC = $null
try {
    if (-not (Test-Path $Daemon)) { throw "binaire absent : $Daemon (cargo build -p onionbit-daemon d'abord)" }

    # --------------------------------------------------------------
    # Topologie : R = ancre + seul exit + point d'introduction epingle,
    # M = relais pur, A B C = terminaux (groupe de 3).
    # --------------------------------------------------------------
    $R = New-Node 'R' 29500 29600 $true  @()
    $M = New-Node 'M' 29501 29601 $false @($R.Ipv8)
    $A = New-Node 'A' 29510 29610 $false @($R.Ipv8, $M.Ipv8)
    $B = New-Node 'B' 29520 29620 $false @($R.Ipv8, $M.Ipv8)
    $C = New-Node 'C' 29530 29630 $false @($R.Ipv8, $M.Ipv8)

    foreach ($n in @($R, $M, $A, $B, $C)) { Start-OnionBit $n | Out-Null }
    foreach ($n in @($R, $M, $A, $B, $C)) { $n.Key = Wait-ApiKey $n.Dir }
    Step 'demarrage' ("R(exit) M(relais) A B C ; messaging_hops={0}" -f $MessagingHops)

    foreach ($n in @($A, $B, $C)) {
        Wait-For "API $($n.Name)" { (ApiGetStatus $n '/statistics/tribler') -eq 200 } 90 | Out-Null
    }
    Step 'apis' 'REST disponible sur A, B et C'

    # Convergence : chaque terminal doit verifier les 4 autres pairs.
    foreach ($n in @($A, $B, $C)) {
        Wait-For "mesh convergent ($($n.Name) >= 4 pairs)" {
            $o = ApiGet $n '/ipv8/overlays'
            $d = @($o.overlays | Where-Object { $_.overlay_name -match 'Discovery' }) | Select-Object -First 1
            if ($d -and @($d.peers).Count -ge 4) { $d } else { $null }
        } 240 | Out-Null
    }
    Step 'mesh' 'A, B et C ont verifie R, M et les deux autres terminaux'

    $pkA = (ApiGet $A '/messaging/stats').public_key
    $pkB = (ApiGet $B '/messaging/stats').public_key
    $pkC = (ApiGet $C '/messaging/stats').public_key
    if (-not $pkA -or -not $pkB -or -not $pkC) { throw 'public_key absent des stats' }
    if (@($pkA, $pkB, $pkC | Sort-Object -Unique).Count -ne 3) { throw 'cles d identite non distinctes' }
    Step 'identites' ("pkA={0}.. pkB={1}.. pkC={2}.." -f `
        $pkA.Substring(0,12), $pkB.Substring(0,12), $pkC.Substring(0,12))

    # --------------------------------------------------------------
    # Maillage de contacts : A-B, A-C, B-C (le fan-out groupe exige
    # une liaison e2e par paire de membres ; l'invite exige un
    # contact actif cote invitant).
    # --------------------------------------------------------------
    Link-Pair $A $B $pkB $pkA
    Step 'lien-A-B' 'A <-> B : liaison e2e + consentement actif'
    Link-Pair $A $C $pkC $pkA
    Step 'lien-A-C' 'A <-> C : liaison e2e + consentement actif'
    Link-Pair $B $C $pkC $pkB
    Step 'lien-B-C' 'B <-> C : liaison e2e + consentement actif'
    Wait-For 'liaisons bound A<-B A<-C' { (IsBound $A $pkB) -and (IsBound $A $pkC) } 60 | Out-Null

    # --------------------------------------------------------------
    # Groupe : A cree {B, C} — invite admise (contacts actifs).
    # --------------------------------------------------------------
    $g = ApiPost $A '/messaging/groups' @{ name = 'mg13-groupe'; members = @($pkB, $pkC) }
    $conv = $g.conv_id
    if (-not $conv -or $conv.Length -ne 32) { throw "conv_id inattendu : $conv" }
    Step 'groupe-cree' ("conv={0} cree par A avec B,C invites" -f $conv.Substring(0,12))

    foreach ($n in @($B, $C)) {
        Wait-For "conv invited chez $($n.Name)" {
            $c = ConvById $n $conv
            if ($c -and $c.state -eq 'invited' -and $c.name -eq 'mg13-groupe') { $c } else { $null }
        } 120 | Out-Null
    }
    Step 'invites-recus' 'B et C voient la conv invited (gctl invite + roster)'

    ApiPost $B "/messaging/groups/$conv/accept" @{} | Out-Null
    ApiPost $C "/messaging/groups/$conv/accept" @{} | Out-Null
    Wait-For 'roster A : B,C membres' {
        $m = (ApiGet $A "/messaging/groups/$conv/members").members
        $act = @($m | Where-Object { $_.state -in @('member','active') -and $_.member_pk -ne $pkA })
        if ($act.Count -ge 2) { $m } else { $null }
    } 120 | Out-Null
    Step 'groupe-actif' 'roster de A : B et C membres actifs'

    # --------------------------------------------------------------
    # Fan-out : A -> groupe (B et C recoivent), puis B -> groupe
    # (A et C recoivent). Dedup (conv, author, mid) couverte par les
    # tests unitaires — ici on verifie la livraison reelle.
    # --------------------------------------------------------------
    $bodyG1 = 'mg13-A-' + [guid]::NewGuid().ToString('N')
    ApiPost $A "/messaging/conversations/$conv/messages" @{ body = $bodyG1 } | Out-Null
    foreach ($n in @($B, $C)) {
        Wait-For "$($n.Name) recoit msg A" {
            $m = @(ConvMessages $n $conv | Where-Object { $_.body -eq $bodyG1 })
            if ($m.Count -ge 1) { $m } else { $null }
        } 90 | Out-Null
    }
    Step 'fanout-A' 'message de groupe de A livre a B et C'

    $bodyG2 = 'mg13-B-' + [guid]::NewGuid().ToString('N')
    ApiPost $B "/messaging/conversations/$conv/messages" @{ body = $bodyG2 } | Out-Null
    foreach ($n in @($A, $C)) {
        Wait-For "$($n.Name) recoit msg B" {
            $m = @(ConvMessages $n $conv | Where-Object { $_.body -eq $bodyG2 -and $_.author_pk -eq $pkB })
            if ($m.Count -ge 1) { $m } else { $null }
        } 90 | Out-Null
    }
    Step 'fanout-B' 'message de groupe de B livre a A et C (author_pk=B)'

    # --------------------------------------------------------------
    # Piece jointe reelle : fichier aleatoire -> attach -> accept ->
    # download anonyme -> SHA-256 identique.
    # --------------------------------------------------------------
    $payload = Join-Path (Resolve-Path $OutDir) 'mg13-payload.bin'
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $bytes = New-Object byte[] (384 * 1024); $rng.GetBytes($bytes)
    [System.IO.File]::WriteAllBytes($payload, $bytes)
    $shaSrc = (Get-FileHash -Algorithm SHA256 $payload).Hash.ToLower()

    $att = ApiPost $A "/messaging/conversations/$conv/attachments" @{ path = $payload }
    if (-not $att.attach_id -or -not $att.infohash) { throw "offre attach inattendue : $($att | ConvertTo-Json -Compress)" }
    if ($att.sent -lt 2) { throw "fan-out attach incomplet : sent=$($att.sent)" }
    Step 'attach-offre' ("attach_id={0} ih={1} sent={2}" -f `
        $att.attach_id.Substring(0,12), $att.infohash.Substring(0,12), $att.sent)

    # L'`attach_id` local d'un destinataire est l'`id` de LA trame
    # `attach` qu'il a recue (une par membre) — la correlation
    # entre pairs passe par l'infohash sale du descripteur.
    $offerAt = @{}
    foreach ($n in @($B, $C)) {
        $o = Wait-For "offre attach chez $($n.Name)" {
            $a = @(ConvAttachs $n $conv | Where-Object { $_.infohash -eq $att.infohash -and $_.state -eq 'offered' -and $_.role -eq 'recv' })
            if ($a.Count -ge 1) { $a[0] } else { $null }
        } 120
        $offerAt[$n.Name] = $o
    }
    Step 'attach-recus' 'B et C voient l offre (descripteur : ih sale, nom, taille)'

    foreach ($n in @($B, $C)) {
        $acc = ApiPost $n "/messaging/attachments/$($offerAt[$n.Name].attach_id)/accept" @{ area = 'public' }
        if ($acc.infohash -ne $att.infohash) { throw "ih accepte != ih offert chez $($n.Name)" }
    }
    Step 'attach-accept' 'B et C acceptent : download anonyme du ih sale lance'

    # Oracle de completion : l'etat `done` suit `stats.finished`
    # (pieces verifiees) — la taille ne suffit pas, le fichier de
    # sortie est pre-alloue par le moteur des le premier octet.
    foreach ($n in @($B, $C)) {
        Wait-For "etat done chez $($n.Name)" {
            $a = @(ConvAttachs $n $conv | Where-Object { $_.infohash -eq $att.infohash -and $_.state -eq 'done' })
            if ($a.Count -ge 1) { $a } else { $null }
        } 240 | Out-Null
    }
    Step 'attach-done' 'lignes attachment en etat done chez B et C (finished -> done)'

    # SHA-256 du contenu livre identique des deux cotes. Le
    # download seede encore → le handle bloque la lecture locale :
    # on le retire d'abord (`remove_data=false`, le fichier reste).
    foreach ($n in @($B, $C)) {
        ApiDelete $n "/downloads/$($att.infohash)" @{ remove_data = $false } | Out-Null
        $dest = Join-Path $n.Dir 'data\public\messaging\mg13-payload.bin'
        $shaDst = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
        if ($shaDst -ne $shaSrc) { throw "SHA-256 divergent chez $($n.Name)" }
    }
    Step 'attach-sha256' 'contenu identique aller-retour chez B et C (384 Kio via essaim sale)'

    # Oracle du sel : meme fichier, seconde offre -> ih different.
    $att2 = ApiPost $A "/messaging/conversations/$conv/attachments" @{ path = $payload }
    if ($att2.infohash -eq $att.infohash) { throw 'salage absent : meme ih pour deux envois identiques' }
    Step 'attach-sel' ("deux envois du meme fichier -> ih distincts ({0} vs {1})" -f `
        $att.infohash.Substring(0,12), $att2.infohash.Substring(0,12))

    # --------------------------------------------------------------
    # Leave : C quitte — roster de A le voit 'left', C voit sa conv
    # 'left' (un depart n'est acte que signe par le membre lui-meme).
    # --------------------------------------------------------------
    ApiPost $C "/messaging/groups/$conv/leave" @{} | Out-Null
    Wait-For 'roster A : C left' {
        $m = (ApiGet $A "/messaging/groups/$conv/members").members
        $cRow = @($m | Where-Object { $_.member_pk -eq $pkC -and $_.state -eq 'left' })
        if ($cRow.Count -ge 1) { $m } else { $null }
    } 120 | Out-Null
    $cLeft = ConvById $C $conv
    if ($cLeft -and $cLeft.state -ne 'left') { throw "conv C pas en left : $($cLeft.state)" }
    Step 'leave' 'C a quitte — roster A le voit left, sa conv est left'

    Step 'mg13' 'cycle groupe 3 demons + piece jointe reelle valide'
}
finally {
    foreach ($n in @($C, $B, $A, $M, $R)) { if ($n -and $n.Proc) { Stop-OnionBit $n.Proc } }
    $manifest = @{
        bench = 'MG-13'
        hops  = $MessagingHops
        ok    = ($results.Count -gt 0 -and -not ($results | Where-Object { -not $_.ok }))
        steps = $results
        ts    = (Get-Date).ToString('o')
    }
    $manifest | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OutDir 'manifest.json') -Encoding UTF8
    Log ("manifeste -> {0}" -f (Join-Path $OutDir 'manifest.json'))
}
