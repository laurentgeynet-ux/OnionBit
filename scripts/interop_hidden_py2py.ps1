#requires -Version 5.1
# Controle interop hidden-service : Tribler seeder anonyme -> Tribler
# downloader anonyme, tous deux reels (8.4.3), etats isoles.
# But : discriminer "reseau public ne repond pas aux peers-request"
# de "notre annonce Rust est defectueuse".
[CmdletBinding()]
param(
    [int]$Hops = 1,
    [int]$Bytes = 6291456,
    [int]$TimeoutMin = 40,
    [string]$Out = (Join-Path $PSScriptRoot '..\target\interop-hidden-py2py')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$out = (Resolve-Path (New-Item -ItemType Directory -Force $Out)).Path
$rep = Join-Path $out 'report'
New-Item -ItemType Directory -Force $rep | Out-Null
$verdicts = [System.Collections.Generic.List[string]]::new()
function Log([string]$m) { $l = '[{0:HH:mm:ss}] {1}' -f (Get-Date), $m; Write-Host $l }
function Verdict([bool]$ok, [string]$label, [string]$detail = '') {
    $tag = if ($ok) { 'OK  ' } else { 'FAIL' }
    $line = ('{0} {1} {2}' -f $tag, $label, $detail).TrimEnd()
    $script:verdicts.Add($line); Log $line
}
$triblerExe = 'C:\Program Files (x86)\Tribler\Tribler.exe'
$mk = Join-Path $PSScriptRoot '..\target\debug\examples\mk_torrent.exe'
function Get-FreeTcpPort { $l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0); $l.Start(); $p = $l.LocalEndpoint.Port; $l.Stop(); return $p }
function Get-FreeUdpPort { $u = [System.Net.Sockets.UdpClient]::new([System.Net.IPEndPoint]::new([System.Net.IPAddress]::Any, 0)); $p = ([System.Net.IPEndPoint]$u.Client.LocalEndPoint).Port; $u.Close(); return $p }

# Generation de la config Tribler isolee (identique au banc principal).
function New-Tribler([string]$tag, [string]$stateDir, [string]$destDir) {
    $apiPort = Get-FreeTcpPort
    $ipv8 = Get-FreeUdpPort
    while ($ipv8 -eq $apiPort) { $ipv8 = Get-FreeUdpPort }
    $key = "interop$tag$(Get-Random)"
    $confDir = Join-Path $stateDir '8.0'
    New-Item -ItemType Directory -Force $confDir | Out-Null
    $conf = @{
        api = @{ http_enabled = $true; http_port = $apiPort; http_host = '127.0.0.1'
                 https_enabled = $false; key = $key }
        ipv8 = @{
            logger = @{ level = 'INFO' }
            interfaces = @( @{ interface = 'UDPIPv4'; ip = '0.0.0.0'; port = $ipv8 } )
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
                            ip_addresses = @(
                                @('130.161.119.206', 6421), @('130.161.119.206', 6422),
                                @('131.180.27.155', 6423), @('131.180.27.156', 6424),
                                @('131.180.27.161', 6427), @('131.180.27.161', 6521),
                                @('131.180.27.161', 6522), @('131.180.27.162', 6523),
                                @('131.180.27.162', 6524), @('130.161.119.215', 6525),
                                @('130.161.119.215', 6526), @('130.161.119.201', 6527),
                                @('130.161.119.201', 6528)
                            )
                            dns_addresses = @(
                                @('dispersy1.tribler.org', 6421), @('dispersy1.st.tudelft.nl', 6421),
                                @('dispersy2.tribler.org', 6422), @('dispersy2.st.tudelft.nl', 6422),
                                @('dispersy3.tribler.org', 6423), @('dispersy3.st.tudelft.nl', 6423),
                                @('dispersy4.tribler.org', 6424)
                            )
                            bootstrap_timeout = 30.0
                        } }
                    )
                    initialize = @{}
                    on_start = @()
                }
            )
        }
        libtorrent = @{ port = 0; utp = $true; dht = $false; upnp = $false; natpmp = $false
                        lsd = $false; download_defaults = @{ saveas = $destDir } }
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
    $envMap = @{ TSTATEDIR = $stateDir; CORE_API_PORT = "$apiPort"; CORE_API_KEY = $key }
    return [pscustomobject]@{ Tag = $tag; State = $stateDir; Api = $apiPort; Key = $key
                             Dest = $destDir; Env = $envMap; Proc = $null }
}
function TCall($t, [string]$method, [string]$path, [int]$sec = 15) {
    Invoke-RestMethod -Method $method -Uri "http://127.0.0.1:$($t.Api)/api$path" `
        -Headers @{ 'X-Api-Key' = $t.Key } -TimeoutSec $sec
}
function Start-Tribler($t) {
    foreach ($kv in $t.Env.GetEnumerator()) { Set-Item "Env:$($kv.Key)" $kv.Value }
    # Start-Process n'herite pas l'env modifie -> on passe par cmd /c avec env.
    $envFile = Join-Path $t.State 'env.cmd'
    $lines = $t.Env.GetEnumerator() | ForEach-Object { "set $($_.Key)=$($_.Value)" }
    $lines += "`"$triblerExe`" -s --log-level INFO"
    Set-Content $envFile $lines -Encoding ASCII
    $proc = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', "`"$envFile`"" -PassThru `
        -WindowStyle Hidden -RedirectStandardOutput (Join-Path $out "$($t.Tag)_stdout.log") `
        -RedirectStandardError (Join-Path $out "$($t.Tag)_stderr.log")
    foreach ($kv in $t.Env.GetEnumerator()) { Remove-Item "Env:$($kv.Key)" -ErrorAction SilentlyContinue }
    $t.Proc = $proc
    Log "$($t.Tag) demarre pid=$($proc.Id) api=$($t.Api)"
}

# ---------- Phase 0 : contenu ----------
Log '=== Phase 0 : contenu + torrent ==='
$content = Join-Path $out 'content'
foreach ($d in @($content, (Join-Path $out 'seed-dl'), (Join-Path $out 'dl-dl'),
                 (Join-Path $out 'ts-state'), (Join-Path $out 'td-state'))) {
    if (Test-Path $d) { Remove-Item -Recurse -Force $d }
}
$nonce = Get-Random -Minimum 1 -Maximum ([int]::MaxValue)
$mkOut = & $mk $content $Bytes $nonce
$mkOut | Tee-Object (Join-Path $rep 'mk_torrent.txt') | Out-Null
$torrent = Join-Path $content 'test.torrent'
$ih = ($mkOut | Select-String 'infohash=([0-9a-f]+)').Matches.Groups[1].Value
if (-not $ih) { throw 'infohash non obtenu' }
Log "infohash=$ih"
$lookup = ([System.BitConverter]::ToString(
    [System.Security.Cryptography.SHA1]::Create().ComputeHash(
        [System.Text.Encoding]::ASCII.GetBytes('tribler anonymous download' + $ih)))
).Replace('-', '').ToLower()
Log "lookup_dht=$lookup"

# ---------- Phase 1 : deux Tribler ----------
$Ts = New-Tribler 'Ts' (Join-Path $out 'ts-state') (Join-Path $out 'seed-dl')
$Td = New-Tribler 'Td' (Join-Path $out 'td-state') (Join-Path $out 'dl-dl')
# Le seeder a besoin du fichier complet dans sa destination AVANT le PUT.
New-Item -ItemType Directory -Force $Ts.Dest | Out-Null
Copy-Item (Join-Path $content 'donnee.bin') $Ts.Dest -Force
$deadline = (Get-Date).AddMinutes($TimeoutMin)

try {
    Start-Tribler $Ts
    Start-Tribler $Td
    foreach ($t in @($Ts, $Td)) {
        $up = $false; $w = (Get-Date).AddSeconds(240)
        while ((Get-Date) -lt $w -and -not $up) {
            try { TCall $t 'GET' '/ipv8/overlays' 5 | Out-Null; $up = $true } catch { Start-Sleep -Seconds 2 }
        }
        Verdict $up "$($t.Tag) : API en ligne"
        if (-not $up) { throw "$($t.Tag) API muette" }
    }
    # Bootstrap : attendre quelques pairs tunnel sur les deux.
    foreach ($t in @($Ts, $Td)) {
        $ok = $false; $w = (Get-Date).AddMinutes(6)
        while ((Get-Date) -lt $w -and -not $ok) {
            try {
                $ov = (TCall $t 'GET' '/ipv8/overlays' 10).overlays
                $nP = ($ov | ForEach-Object { @($_.peers).Count } | Measure-Object -Sum).Sum
                $nT = @($ov | Where-Object { $_.overlay_name -match 'Tunnel' } | ForEach-Object { @($_.peers).Count } | Measure-Object -Sum).Sum
                Log "$($t.Tag) peers=$nP tunnel=$nT"
                if ($nT -ge 4) { $ok = $true }
            } catch { Start-Sleep -Seconds 3 }
            if (-not $ok) { Start-Sleep -Seconds 5 }
        }
        Verdict $ok "$($t.Tag) : bootstrap" "tunnel_peers=$nT"
    }

    # ---------- Phase 2 : seed anonyme chez Ts ----------
    Log '=== Phase 2 : seed anonyme chez Ts ==='
    $dest = [System.Uri]::EscapeDataString($Ts.Dest)
    $sAdd = $null; $w = (Get-Date).AddMinutes(3)
    while (-not $sAdd -and (Get-Date) -lt $w) {
        try {
            $sAdd = Invoke-RestMethod -Method PUT `
                -Uri "http://127.0.0.1:$($Ts.Api)/api/downloads?anon_hops=$Hops&safe_seeding=true&destination=$dest" `
                -Headers @{ 'X-Api-Key' = $Ts.Key } -ContentType 'applications/x-bittorrent' `
                -InFile $torrent -TimeoutSec 30
            if ($sAdd.PSObject.Properties['error'] -and $sAdd.error) {
                Log "PUT seed : $($sAdd.error | ConvertTo-Json -Compress -Depth 4)"; $sAdd = $null; Start-Sleep -Seconds 10
            }
        } catch { Log "PUT seed : $($_.Exception.Message)"; Start-Sleep -Seconds 10 }
    }
    Verdict ($sAdd -and $sAdd.started) 'Ts : seed anonyme accepte' "started=$($sAdd.started)"
    # attendre SEEDING + IP_SEEDER chez Ts
    $seedOk = $false; $w = (Get-Date).AddMinutes(5)
    while ((Get-Date) -lt $w -and -not $seedOk) {
        try {
            $ds = @((TCall $Ts 'GET' '/downloads' 10).downloads) | Where-Object { $_.infohash -eq $ih }
            if ($ds -and $ds[0].status -match 'SEED') { $seedOk = $true }
            $ip = @((TCall $Ts 'GET' '/ipv8/tunnel/circuits' 10).circuits | Where-Object { $_.type -eq 'IP_SEEDER' })
            Log ("Ts : st={0} prog={1:P0} IP_SEEDER={2}" -f $ds[0].status, $ds[0].progress, $ip.Count)
            if ($seedOk -and $ip.Count -ge 1) { break }
        } catch { Start-Sleep -Seconds 3 }
        Start-Sleep -Seconds 5
    }
    Verdict $seedOk 'Ts : SEEDING' "status=$($ds[0].status)"

    # ---------- Phase 3 : download anonyme chez Td ----------
    Log '=== Phase 3 : download anonyme chez Td ==='
    $ddest = [System.Uri]::EscapeDataString($Td.Dest)
    New-Item -ItemType Directory -Force $Td.Dest | Out-Null
    $dAdd = $null; $w = (Get-Date).AddMinutes(3)
    while (-not $dAdd -and (Get-Date) -lt $w) {
        try {
            $dAdd = Invoke-RestMethod -Method PUT `
                -Uri "http://127.0.0.1:$($Td.Api)/api/downloads?anon_hops=$Hops&safe_seeding=true&destination=$ddest" `
                -Headers @{ 'X-Api-Key' = $Td.Key } -ContentType 'applications/x-bittorrent' `
                -InFile $torrent -TimeoutSec 30
            if ($dAdd.PSObject.Properties['error'] -and $dAdd.error) {
                Log "PUT dl : $($dAdd.error | ConvertTo-Json -Compress -Depth 4)"; $dAdd = $null; Start-Sleep -Seconds 10
            }
        } catch { Log "PUT dl : $($_.Exception.Message)"; Start-Sleep -Seconds 10 }
    }
    Verdict ($dAdd -and $dAdd.started) 'Td : download anonyme accepte' "started=$($dAdd.started)"

    $done = $false
    while ((Get-Date) -lt $deadline -and -not $done) {
        Start-Sleep -Seconds 15
        try {
            $dd = @((TCall $Td 'GET' '/downloads' 10).downloads) | Where-Object { $_.infohash -eq $ih }
            $sw = (TCall $Td 'GET' '/ipv8/tunnel/swarms' 10).swarms
            $swm = @($sw | Where-Object { $_.info_hash -eq $lookup })
            $cc = (TCall $Td 'GET' '/ipv8/tunnel/circuits' 10).circuits
            $nRp = @($cc | Where-Object { $_.type -eq 'RP_DOWNLOADER' }).Count
            if ($dd) {
                Log ("Td : progress={0:P1} dl={1}B peers={2} st={3} | swarm_ips_dht={4} RPDL={5}" -f
                    $dd[0].progress, $dd[0].all_time_download, $dd[0].num_peers, $dd[0].status,
                    $(if ($swm) { $swm[0].num_ips_from_dht } else { '?' }), $nRp)
                if ($dd[0].progress -ge 1.0) { $done = $true }
            }
        } catch { Log "sonde Td : $($_.Exception.Message)" }
    }
    $dd = @((TCall $Td 'GET' '/downloads' 10).downloads) | Where-Object { $_.infohash -eq $ih }
    if ($dd) {
        Verdict ($dd[0].all_time_download -gt 0) 'Td a recu des octets (hidden service)' "dl=$($dd[0].all_time_download)"
        Verdict ($dd[0].progress -ge 1.0) 'Td : telechargement termine' ("progress={0:P1}" -f $dd[0].progress)
    }
    if ($done) {
        Start-Sleep -Seconds 3
        $f = Get-ChildItem -Recurse -Filter 'donnee.bin' $Td.Dest | Select-Object -First 1
        $h = (Get-FileHash -Algorithm SHA256 $f.FullName).Hash.ToLower()
        $src = (Get-FileHash -Algorithm SHA256 (Join-Path $content 'donnee.bin')).Hash.ToLower()
        Verdict ($h -eq $src) 'integrite SHA256' "got=$h want=$src"
    }
}
finally {
    foreach ($t in @($Ts, $Td)) {
        foreach ($ep in @('/downloads','/ipv8/tunnel/circuits','/ipv8/tunnel/swarms','/ipv8/overlays')) {
            try { TCall $t 'GET' $ep 10 | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $rep ("{0}{1}.json" -f $t.Tag, ($ep -replace '/','_'))) -Encoding UTF8 } catch {}
        }
        if ($t.Proc -and -not $t.Proc.HasExited) {
            try {
                # tuer l'arbre Tribler (cmd->Tribler)
                Get-CimInstance Win32_Process | Where-Object { $_.ParentProcessId -eq $t.Proc.Id -or $_.ProcessId -eq $t.Proc.Id } |
                    ForEach-Object { try { Stop-Process -Id $_.ProcessId -Force -ErrorAction Stop } catch {} }
            } catch {}
        }
        foreach ($f in @("$($t.Tag)_stdout.log","$($t.Tag)_stderr.log")) {
            $src = Join-Path $out $f
            if (Test-Path $src) { Copy-Item $src (Join-Path $rep $f) -Force }
        }
    }
    Write-Host ''
    Write-Host '================ VERDICTS ================'
    $verdicts | ForEach-Object { Write-Host $_ }
    Write-Host "rapport : $rep"
}
