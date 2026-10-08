# This file is part of OnionBit.
#
# Copyright (C) 2026 Laurent Geynet
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Banc MS-13 : cycle applicatif messagerie reel entre DEUX demons OnionBit
# independants (processus, state dirs, sockets distincts) interconnectes
# par un mesh loopback avec ancre/relai/exit reels (meme convention que
# fingerprint_mesh.ps1).
#
# Sequence assertee :
#   stats -> connect (resolve DHT + create-e2e) -> premier send (hello)
#   -> B pending, msg ecarte (pending_drop) -> A jamais 'acked'
#   -> accept B -> Active des deux cotes -> msg livre exactement une fois
#   -> 'acked' chez A -> reemission = nouvelle ligne -> restart A
#   (identite, etat, historique, seq restaures) -> kill B -> pas de faux
#   ack -> mort du circuit -> send = 404 + 'failed' -> restart B : aucune
#   retransmission automatique -> reconnect explicite -> livraison ->
#   cleanup DELETE (contact + historique).
#
# Usage :
#   pwsh -File scripts/interop_messaging_e2e.ps1
#   pwsh -File scripts/interop_messaging_e2e.ps1 -MessagingHops 2
#
# Preconditions : `cargo build -p onionbit-daemon` a jour.
# Artefacts : $OutDir/{manifest.json, console.log, <node>/...}.

[CmdletBinding()]
param(
    [string]$Daemon = 'target\debug\onionbit-daemon.exe',
    [string]$OutDir = ("target\ms13-{0}" -f (Get-Date -Format 'yyyyMMdd-HHmmss')),
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

function Sha256([string]$s) {
    # PS 5.1 / .NET Framework : pas de SHA256::HashData statique.
    $sha = [System.Security.Cryptography.SHA256]::Create()
    ([System.BitConverter]::ToString($sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($s)))).Replace('-', '').ToLower()
}

function ApiGet($node, [string]$path) {
    Invoke-RestMethod -Uri "http://$($node.Api)/api$path" -Headers @{ 'X-Api-Key' = $node.Key } -TimeoutSec 10
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
function ApiPostExpect($node, [string]$path, $body, [int]$want) {
    $json = $body | ConvertTo-Json -Compress
    try {
        Invoke-WebRequest -Uri "http://$($node.Api)/api$path" -Method Post `
            -Headers @{ 'X-Api-Key' = $node.Key; 'Content-Type' = 'application/json' } `
            -Body $json -TimeoutSec 60 | Out-Null
        if ($want -ne 200) { throw "POST $path -> 200, attendu $want" }
        return $null
    } catch {
        if ($_.Exception.Response -and [int]$_.Exception.Response.StatusCode -eq $want) { return $null }
        throw
    }
}
function ApiDelete($node, [string]$path) {
    Invoke-RestMethod -Uri "http://$($node.Api)/api$path" -Method Delete `
        -Headers @{ 'X-Api-Key' = $node.Key } -TimeoutSec 15
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
    $node.StartedAt = (Get-Date).ToString('o')
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
            data_exit_peer    = "$BaseAddr`:28500"
            intro_point_peer  = "$BaseAddr`:28500"
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
        Dir = $dir; Proc = $null; Key = $null; StartedAt = $null
    }
}

# Historique d'un contact ({pk} hex) : {messages: [{id,direction,seq,ts,body,status}]}.
function MsgHistory($node, [string]$pk) { (ApiGet $node "/messaging/contacts/$pk/messages?limit=200").messages }
# Contacts lies (bound uniquement) : {contacts: [{public_key,state,circuit_id}]}.
function BoundContacts($node) { (ApiGet $node '/messaging/contacts').contacts }
function IsBound($node, [string]$pk) {
    # /contacts liste TOUS les contacts ; `circuit_id` null = pas lie.
    @((BoundContacts $node) | Where-Object { $_.public_key -eq $pk -and $null -ne $_.circuit_id }).Count -gt 0
}
function ContactState($node, [string]$pk) {
    $c = (@((BoundContacts $node) | Where-Object { $_.public_key -eq $pk }) | Select-Object -First 1)
    if ($c) { $c.state } else { $null }
}

# Logs demons : herites par les processus enfants (le banc diagnostique
# le chemin tunnel/e2e -- debug volontaire).
$env:RUST_LOG = 'onionbit_tunnel=debug,onionbit_core=debug,onionbit_ipv8=debug,onionbit_messaging=debug,onionbit_api=debug,info'

$R = $M = $A = $B = $null
$pkA = $pkB = $sA0 = $sB0 = $body1 = $null
try {
    if (-not (Test-Path $Daemon)) { throw "binaire absent : $Daemon (cargo build -p onionbit-daemon d'abord)" }

    # --------------------------------------------------------------
    # Topologie : R = ancre + seul exit + point d'introduction epingle,
    # M = relais pur (utile a messaging_hops=2), A et B = terminaux.
    # --------------------------------------------------------------
    $R = New-Node 'R' 28500 28600 $true  @()
    $M = New-Node 'M' 28501 28601 $false @($R.Ipv8)
    $A = New-Node 'A' 28510 28610 $false @($R.Ipv8, $M.Ipv8)
    $B = New-Node 'B' 28520 28620 $false @($R.Ipv8, $M.Ipv8)

    Start-OnionBit $R | Out-Null
    Start-OnionBit $M | Out-Null
    Start-OnionBit $A | Out-Null
    Start-OnionBit $B | Out-Null
    foreach ($n in @($R, $M, $A, $B)) { $n.Key = Wait-ApiKey $n.Dir }
    Step 'demarrage' ("R(exit) M(relais) A B ; pids={0},{1},{2},{3} ; messaging_hops={4}" -f `
        $R.Proc.Id, $M.Proc.Id, $A.Proc.Id, $B.Proc.Id, $MessagingHops)

    foreach ($n in @($A, $B)) {
        Wait-For "API $($n.Name)" { (ApiGetStatus $n '/statistics/tribler') -eq 200 } 90 | Out-Null
    }
    Step 'apis' 'REST disponible sur A et B (cles distinctes auto-generees)'

    # Convergence : chaque terminal doit verifier >= 3 pairs (R, M, pair
    # oppose) sur l'overlay de decouverte.
    foreach ($n in @($A, $B)) {
        Wait-For "mesh convergent ($($n.Name) >= 3 pairs)" {
            $o = ApiGet $n '/ipv8/overlays'
            $d = @($o.overlays | Where-Object { $_.overlay_name -match 'Discovery' }) | Select-Object -First 1
            if ($d -and @($d.peers).Count -ge 3) { $d } else { $null }
        } 180 | Out-Null
    }
    Step 'mesh' 'A et B ont verifie R, M et le pair oppose'

    # Stats initiaux + identites distinctes.
    $sA0 = ApiGet $A '/messaging/stats'
    $sB0 = ApiGet $B '/messaging/stats'
    if (-not $sA0.public_key -or -not $sB0.public_key) { throw 'public_key absent des stats' }
    if ($sA0.public_key -eq $sB0.public_key) { throw 'A et B partagent la meme cle - demons non independants' }
    $pkA = $sA0.public_key; $pkB = $sB0.public_key
    Step 'identites' ("pkA={0}.. mhA={1}.. pkB={2}.. mhB={3}.." -f `
        $pkA.Substring(0,16), $sA0.messaging_hash.Substring(0,16), $pkB.Substring(0,16), $sB0.messaging_hash.Substring(0,16))

    # Presence : attendre que les points d'introduction de B soient
    # annonces puis resolvables par A (DHT via exit R).
    $lastErr = ''
    $conn = Wait-For 'connect A -> B (resolve DHT + create-e2e)' {
        try { ApiPost $A '/messaging/contacts/connect' @{ public_key = $pkB } }
        catch {
            $m = $_.Exception.Message
            if ($m -ne $lastErr) { $lastErr = $m; Log "connect retry : $m" }
            $null
        }
    } 240
    if (-not $conn.circuit_id) { throw 'connect n a pas retourne circuit_id' }
    Step 'connect' ("circuit e2e lie cid={0}" -f $conn.circuit_id)

    # --------------------------------------------------------------
    # Consentement : le premier send porte le hello ; le msg est
    # ecarte chez B (pending_drop) et jamais acquitte chez A.
    # --------------------------------------------------------------
    $body0 = 'ms13-pre-consent-' + [guid]::NewGuid().ToString('N')
    $send0 = ApiPost $A "/messaging/contacts/$pkB/messages" @{ body = $body0 }
    if (-not $send0.id) { throw 'POST messages ne retourne pas d id' }
    Step 'hello-emis' ("premier send accepte (id={0}) - porte le hello" -f $send0.id)

    Wait-For 'demande pending chez B' {
        $c = ApiGet $B '/messaging/contacts/pending'
        if (@($c.contacts | Where-Object { $_.public_key -eq $pkA }).Count -eq 1) { $c } else { $null }
    } 120 | Out-Null
    Step 'pending' 'B voit la demande de contact de A (Consent)'

    # Non-livraison avant consentement.
    $hB0 = MsgHistory $B $pkA
    if (@($hB0 | Where-Object { $_.direction -eq 'in' }).Count -ne 0) {
        throw 'B a livre un message avant consentement'
    }
    Start-Sleep -Seconds 5
    $m0 = (@((MsgHistory $A $pkB) | Where-Object { $_.id -eq $send0.id }) | Select-Object -First 1)
    if (-not $m0) { throw 'message pre-consent absent de l historique A' }
    if ($m0.status -eq 'acked') { throw 'message acquitte sans consentement !' }
    Step 'pre-consent-gating' ("B : rien livre ; A : statut '{0}' (pas de faux ack)" -f $m0.status)

    ApiPost $B "/messaging/contacts/$pkA/accept" @{} | Out-Null
    Wait-For 'B -> active' { (ContactState $B $pkA) -eq 'active' } 60 | Out-Null
    Wait-For 'A -> active (bound conserve)' { (IsBound $A $pkB) } 60 | Out-Null
    Step 'consentement' 'Active des deux cotes (accept -> trame accept -> liaison conservee)'

    # --------------------------------------------------------------
    # Envoi A -> B : livraison exactly-once puis acked chez A.
    # --------------------------------------------------------------
    $body1 = 'ms13-A-to-B-' + [guid]::NewGuid().ToString('N')
    $send1 = ApiPost $A "/messaging/contacts/$pkB/messages" @{ body = $body1 }
    $id1 = $send1.id
    $mB1 = Wait-For 'B recoit le message' {
        $m = (@((MsgHistory $B $pkA) | Where-Object { $_.id -eq $id1 }) | Select-Object -First 1)
        if ($m) { $m } else { $null }
    } 60
    if ($mB1.body -ne $body1) { throw "corps recu != envoye" }
    if ($mB1.direction -ne 'in') { throw "direction inattendue : $($mB1.direction)" }
    if ($null -eq $mB1.seq -or $null -eq $mB1.ts) { throw 'seq/ts absents' }
    if ($mB1.status -ne 'received') { throw "statut B inattendu : $($mB1.status)" }
    Step 'delivery' ("B a recu exactement le bon corps (id={0} seq={1})" -f $id1, $mB1.seq)

    Wait-For 'A voit acked' {
        $m = (@((MsgHistory $A $pkB) | Where-Object { $_.id -eq $id1 }) | Select-Object -First 1)
        if ($m -and $m.status -eq 'acked') { $m } else { $null }
    } 60 | Out-Null
    Step 'ack' 'A voit le message acked (pas juste sent)'

    # Reemission applicative : nouvelle trame = nouvelle ligne chez B
    # (la dedup filaire id est couverte par les tests unitaires).
    $send2 = ApiPost $A "/messaging/contacts/$pkB/messages" @{ body = $body1 }
    if ($send2.id -eq $id1) { throw 'meme id retourne pour une reemission' }
    $hB2 = Wait-For 'B recoit la 2e copie' {
        $h = MsgHistory $B $pkA
        if (@($h | Where-Object { $_.direction -eq 'in' -and $_.body -eq $body1 }).Count -eq 2) { $h } else { $null }
    } 60
    if (@($hB2 | Where-Object { $_.direction -eq 'in' }).Count -ne 2) { throw 'exact-once casse chez B' }
    Step 'reemission' 'reemission = nouvelle ligne chez B ; pas de dedup applicative parasite'

    # --------------------------------------------------------------
    # Restart A : identite, contact, historique, statuts restaures ;
    # liaison e2e perdue -> send = 404 + failed ; reconnect explicite.
    # --------------------------------------------------------------
    Stop-OnionBit $A.Proc
    Step 'stop-A' ("A arrete (pid {0})" -f $A.Proc.Id)
    $A.Proc = $null
    Start-OnionBit $A | Out-Null
    $A.Key = Wait-ApiKey $A.Dir
    Wait-For 'API A (restart)' { (ApiGetStatus $A '/statistics/tribler') -eq 200 } 90 | Out-Null

    $sA1 = ApiGet $A '/messaging/stats'
    if ($sA1.public_key -ne $pkA) { throw 'cle d identite changee au restart' }
    $hA1 = MsgHistory $A $pkB
    $mA1 = @($hA1 | Where-Object { $_.id -eq $id1 }) | Select-Object -First 1
    if (-not $mA1) { throw 'historique perdu au restart' }
    if ($mA1.status -ne 'acked') { throw 'statut acked perdu au restart' }
    if (IsBound $A $pkB) { throw 'liaison e2e encore presente apres restart ?' }
    Step 'restart' 'identite + contact + historique + acked restaures ; liaison e2e perdue'

    # Contact Active mais non lie : send = 404 offline + ligne failed.
    $off = ApiPostExpect $A "/messaging/contacts/$pkB/messages" @{ body = 'ms13-unbound' } 404
    $mOff = Wait-For 'ligne failed visible' {
        $m = (@((MsgHistory $A $pkB) | Where-Object { $_.body -eq 'ms13-unbound' }) | Select-Object -First 1)
        if ($m -and $m.status -eq 'failed') { $m } else { $null }
    } 30
    Step 'unbound-offline' 'contact actif non lie -> 404 + failed, pas de file'

    # Reconnexion explicite -> re-liaison -> envoi livre a nouveau.
    $conn2 = Wait-For 'reconnect A -> B' {
        try { ApiPost $A '/messaging/contacts/connect' @{ public_key = $pkB } } catch { $null }
    } 120
    $body3 = 'ms13-A2-to-B-' + [guid]::NewGuid().ToString('N')
    $send3 = ApiPost $A "/messaging/contacts/$pkB/messages" @{ body = $body3 }
    Wait-For 'B recoit apres reconnexion' {
        $m = (@((MsgHistory $B $pkA) | Where-Object { $_.id -eq $send3.id }) | Select-Object -First 1)
        if ($m -and $m.body -eq $body3) { $m } else { $null }
    } 60 | Out-Null
    Step 'reconnect' 'liaison retablie explicitement ; livraison + ack reprennent'

    # --------------------------------------------------------------
    # Offline reel : kill B.
    #  a) envoi immediat : send_data reussit (UDP) mais jamais acquitte
    #     -> le statut doit rester 'sent' (pas de faux ack).
    #  b) mort du circuit cote A -> envoi = 404 + 'failed'.
    #  c) restart B : aucune retransmission automatique ; reconnect
    #     explicite -> reprise.
    # --------------------------------------------------------------
    Stop-OnionBit $B.Proc
    $pidB = $B.Proc.Id; $B.Proc = $null
    Step 'stop-B' ("B arrete (pid {0})" -f $pidB)

    $bodyDead = 'ms13-dead-' + [guid]::NewGuid().ToString('N')
    $sendDead = ApiPost $A "/messaging/contacts/$pkB/messages" @{ body = $bodyDead }
    if (-not $sendDead.id) { throw 'POST sur pair mort devrait etre accepte par REST (send_data UDP)' }
    Start-Sleep -Seconds 10
    $mDead = (@((MsgHistory $A $pkB) | Where-Object { $_.id -eq $sendDead.id }) | Select-Object -First 1)
    if ($mDead.status -eq 'acked') { throw 'faux acquittement : message acked alors que B est mort' }
    Step 'pas-de-faux-ack' ("statut '{0}' - pas d ack fantome" -f $mDead.status)

    # La liaison meurt a l'expiration du circuit (inactivite du pair,
    # ~60 s) : attendre que /contacts ne liste plus B.
    Wait-For 'liaison e2e morte cote A' { -not (IsBound $A $pkB) } 200 | Out-Null
    $off2 = ApiPostExpect $A "/messaging/contacts/$pkB/messages" @{ body = 'ms13-offline' } 404
    Wait-For 'failed persiste' {
        $m = (@((MsgHistory $A $pkB) | Where-Object { $_.body -eq 'ms13-offline' }) | Select-Object -First 1)
        if ($m -and $m.status -eq 'failed') { $m } else { $null }
    } 30 | Out-Null
    Step 'offline' 'circuit mort -> POST 404 + failed persistant (online-only, pas de file)'

    # Restart B : la messagerie v1 ne retransmet rien automatiquement.
    Start-OnionBit $B | Out-Null
    $B.Key = Wait-ApiKey $B.Dir
    Wait-For 'API B (restart)' { (ApiGetStatus $B '/statistics/tribler') -eq 200 } 90 | Out-Null
    Start-Sleep -Seconds 15
    $hB4 = MsgHistory $B $pkA
    if (@($hB4 | Where-Object { $_.body -in @('ms13-offline', $bodyDead) }).Count -ne 0) {
        throw 'message hors ligne livre fantome apres restart de B'
    }
    $hA4 = MsgHistory $A $pkB
    if ((@($hA4 | Where-Object { $_.id -eq $sendDead.id }) | Select-Object -First 1).status -eq 'acked') {
        throw 'message mort retroactivement acquitte'
    }
    Step 'pas-de-retransmission' 'restart B : rien de relivre ; statuts figes'

    # Reprise explicite : A reconnecte -> livraison reprend.
    $conn3 = Wait-For 'reconnect final A -> B' {
        try { ApiPost $A '/messaging/contacts/connect' @{ public_key = $pkB } } catch { $null }
    } 120
    $body4 = 'ms13-final-' + [guid]::NewGuid().ToString('N')
    $send4 = ApiPost $A "/messaging/contacts/$pkB/messages" @{ body = $body4 }
    Wait-For 'B recoit le message final' {
        $m = (@((MsgHistory $B $pkA) | Where-Object { $_.id -eq $send4.id }) | Select-Object -First 1)
        if ($m -and $m.body -eq $body4) { $m } else { $null }
    } 60 | Out-Null
    Step 'reprise' 'connect explicite apres coupure -> livraison retablie'

    # --------------------------------------------------------------
    # Cleanup : DELETE contact = suppression reelle (contact+messages).
    # --------------------------------------------------------------
    ApiDelete $A "/messaging/contacts/$pkB" | Out-Null
    Start-Sleep -Milliseconds 500
    if (@((MsgHistory $A $pkB)).Count -ne 0) { throw 'historique non purge' }
    if (IsBound $A $pkB) { throw 'liaison encore presente apres DELETE' }
    Step 'cleanup' 'contact + historique supprimes cote A'

    Step 'MS-13' 'cycle e2e reel complet : connect, consent-gating, livraison exactly-once, acked, restart, offline, reprise, cleanup'
}
catch {
    $results.Add(@{ step = 'FATAL'; ok = $false; detail = $_.Exception.Message; ts = (Get-Date).ToString('o') }) | Out-Null
    Log ("FAIL {0}" -f $_.Exception.Message)
}
finally {
    foreach ($n in @($R, $M, $A, $B)) { if ($n) { Stop-OnionBit $n.Proc } }
    $gitCommit = try { git rev-parse --short HEAD 2>$null } catch { 'inconnu' }
    $gitBranch = try { git rev-parse --abbrev-ref HEAD 2>$null } catch { 'inconnue' }
    $manifest = [ordered]@{
        bench          = 'MS-13 interop messagerie e2e'
        git_commit     = $gitCommit
        git_branch     = $gitBranch
        date           = (Get-Date).ToString('o')
        messaging_hops = $MessagingHops
        nodes          = @{}
        steps          = $results
        ok             = -not (@($results | Where-Object { -not $_.ok }).Count -gt 0)
    }
    foreach ($n in @($R, $M, $A, $B)) {
        if ($n) {
            $manifest.nodes[$n.Name] = @{
                ipv8         = $n.Ipv8
                api          = $n.Api
                pid          = if ($n.Proc) { $n.Proc.Id } else { $null }
                api_key_sha256 = if ($n.Key) { Sha256 $n.Key } else { $null }
                state_dir    = $n.Dir
                started_at   = $n.StartedAt
            }
        }
    }
    if ($pkA) { $manifest.nodes['A'].public_key = $pkA; $manifest.nodes['A'].messaging_hash = $sA0.messaging_hash }
    if ($pkB) { $manifest.nodes['B'].public_key = $pkB; $manifest.nodes['B'].messaging_hash = $sB0.messaging_hash }
    if ($body1) { $manifest.payload_sha256 = Sha256 $body1 }
    try {
        $manifest | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OutDir 'manifest.json')
    } catch {
        Add-Content (Join-Path $OutDir 'manifest.json') ($manifest | Out-String)
        Log "manifest degrade (serialisation : $($_.Exception.Message))"
    }
    Log ("manifest -> {0} ; ok={1}" -f (Join-Path $OutDir 'manifest.json'), $manifest.ok)
}
