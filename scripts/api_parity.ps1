# api_parity.ps1 - banc de parite REST Tribler Python <-> daemon Rust.
#
# Envoie la meme batterie de requetes (GET sans effet de bord) aux deux
# daemons et compare : code HTTP, ensemble des cles JSON de premier
# niveau et type de chaque cle. Les divergences residuelles connues
# sont documentees dans docs/architecture/decisions/0006-*.
#
# Les deux daemons doivent deja tourner :
#   - Tribler Python : `Tribler.exe -s` (cle API dans
#     `<state>\8.0\configuration.json`, entree `api/key`).
#   - Daemon Rust : `tribler-daemon` (cle dans `<state>\configuration.json`,
#     entree `api.key`).
#
# Usage :
#   powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\api_parity.ps1 `
#       -TriblerUrl http://127.0.0.1:20100 -TriblerKey <hex> `
#       -RustUrl http://127.0.0.1:8085 -RustKey <hex> [-FailOnDiff]

[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $TriblerUrl,
    [Parameter(Mandatory)] [string] $TriblerKey,
    [Parameter(Mandatory)] [string] $RustUrl,
    [Parameter(Mandatory)] [string] $RustKey,
    # Code de sortie 1 si une divergence est detectee (defaut : rapport seul).
    [switch] $FailOnDiff,
    [int] $TimeoutSec = 15
)

$ErrorActionPreference = "Stop"

# Batterie de requetes : GET idempotents des endpoints de controle.
# Les endpoints SSE (`/api/events`, speed-test) et les mutants
# (PUT/POST/DELETE) sont exclus - la parite des mutations est couverte
# par les tests d'integration.
$Battery = @(
    "/api/downloads",
    "/api/settings",
    "/api/statistics/tribler",
    "/api/statistics/ipv8",
    "/api/ipv8/overlays",
    "/api/ipv8/overlays/statistics",
    "/api/ipv8/network",
    "/api/ipv8/tunnel/settings",
    "/api/ipv8/tunnel/circuits",
    "/api/ipv8/tunnel/relays",
    "/api/ipv8/tunnel/exits",
    "/api/ipv8/tunnel/swarms",
    "/api/ipv8/tunnel/peers",
    "/api/ipv8/tunnel/peers/dht",
    "/api/ipv8/tunnel/peers/pex",
    "/api/ipv8/dht/statistics",
    "/api/ipv8/dht/values",
    "/api/ipv8/dht/buckets",
    "/api/ipv8/asyncio/drift",
    "/api/ipv8/asyncio/tasks",
    "/api/ipv8/asyncio/debug",
    "/api/rss",
    "/api/versioning/versions",
    "/api/versioning/versions/current",
    "/api/files/browse?path=.",
    "/api/events/info",
    "/api/logging"
)

function Invoke-Api {
    param([string] $Base, [string] $Key, [string] $Path)
    $uri = "$Base$Path"
    try {
        $resp = Invoke-WebRequest -Uri $uri -Method GET -TimeoutSec $TimeoutSec `
            -Headers @{ "X-Api-Key" = $Key } -UseBasicParsing
        $json = $null
        try { $json = $resp.Content | ConvertFrom-Json } catch { }
        return @{ Status = [int] $resp.StatusCode; Json = $json }
    } catch {
        $status = if ($_.Exception.Response) { [int] $_.Exception.Response.StatusCode } else { -1 }
        $json = $null
        if ($_.ErrorDetails -and $_.ErrorDetails.Message) {
            try { $json = $_.ErrorDetails.Message | ConvertFrom-Json } catch { }
        }
        return @{ Status = $status; Json = $json }
    }
}

function Get-Shape {
    # Signature = ensemble de "cle:type" de premier niveau (objets) ou
    # type du tableau (listes) - comparaison structurelle, pas de valeurs.
    param($Json)
    if ($null -eq $Json) { return "(non-json)" }
    if ($Json -is [System.Management.Automation.PSCustomObject]) {
        $props = $Json.PSObject.Properties | ForEach-Object {
            "$($_.Name):$($_.Value.GetType().Name)"
        } | Sort-Object
        return ($props -join "`n")
    }
    return "scalar:$($Json.GetType().Name)"
}

$diffs = @()
foreach ($path in $Battery) {
    $py = Invoke-Api -Base $TriblerUrl -Key $TriblerKey -Path $path
    $rs = Invoke-Api -Base $RustUrl -Key $RustKey -Path $path
    $statusDiff = $py.Status -ne $rs.Status
    $shapeDiff = (Get-Shape $py.Json) -ne (Get-Shape $rs.Json)
    $mark = if ($statusDiff -or $shapeDiff) { "DIFF" } else { "ok  " }
    Write-Host "$mark $path  (py=$($py.Status) rs=$($rs.Status))"
    if ($statusDiff -or $shapeDiff) {
        $diffs += [pscustomobject]@{
            Path      = $path
            PyStatus  = $py.Status
            RsStatus  = $rs.Status
            ShapeDiff = $shapeDiff
        }
    }
}

Write-Host ""
if ($diffs.Count -eq 0) {
    Write-Host "PARITE OK - $($Battery.Count) requetes identiques." -ForegroundColor Green
    exit 0
}
Write-Host "$($diffs.Count) divergence(s) :" -ForegroundColor Yellow
$diffs | Format-Table -AutoSize
if ($FailOnDiff) { exit 1 }
exit 0
