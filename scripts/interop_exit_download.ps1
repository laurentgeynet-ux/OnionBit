# interop_exit_download.ps1 - jalon de fermeture de l'etape 12 :
# telechargement BitTorrent reel a travers un circuit dont la sortie
# est le vrai `TunnelCommunity` pyipv8 (PEER_FLAG_EXIT_BT).
#
#   rqbit downloader -> [relais Rust] -> sortie pyipv8 -> rqbit seeder
#
# Le noeud Python relaie les datagrammes uTP du downloader vers la
# socket d'ecoute du seeder (destination reelle des cellules `data`)
# et achemine les reponses en retour — le contenu telecharge est
# verifie octet a octet.
#
# Usage : powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\interop_exit_download.ps1 [-Hops 1|2|3] [-Dht]
# Prerequis : venv interop (voir scripts/interop_ipv8.ps1).
# -Dht : le pair du seeder est decouvert par la DHT routee dans le
# tunnel (get_peers a travers la sortie pyipv8) au lieu de
# l'injection `initial_peers`.

param(
    # 1 = circuit direct vers la sortie Python ; 2/3 = un/deux
    # relais Rust avant la sortie Python (defaut 2).
    [int] $Hops = 2,
    # Decouverte du seeder par la DHT via le tunnel.
    [switch] $Dht,
    # Taille du fichier telecharge en octets (defaut 200000).
    [int] $Payload = 200000
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$pyipv8 = if ($env:TRIBLER_PYIPV8) { $env:TRIBLER_PYIPV8 } else { "D:\Projet\Tribler_sources\tribler\pyipv8" }
$venvPy = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
$outDir = Join-Path $root "target\interop-exit-download"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$pyPort = 12100        # endpoint tunnel pyipv8 (sortie)
$pyLog = Join-Path $outDir "py_exit_packets.log"
$rsLog = Join-Path $outDir "rust_exit_download.log"
$keyFile = Join-Path $outDir "py_key.txt"

Remove-Item -Force -ErrorAction SilentlyContinue $keyFile

Write-Host "== build exit_download_interop (rust) =="
cargo build -p tribler-bittorrent --example exit_download_interop
if ($LASTEXITCODE -ne 0) { throw "build echoue" }

$env:PYTHONPATH = $pyipv8
Write-Host "== noeud Python tunnel (relais+exit EXIT_BT) sur 127.0.0.1:$pyPort =="
$pyProc = Start-Process -FilePath $venvPy -PassThru -NoNewWindow `
    -ArgumentList "`"$PSScriptRoot\interop\py_tunnel_node.py`" --port $pyPort --keyfile `"$keyFile`" --log `"$pyLog`" --duration 300" `
    -RedirectStandardError (Join-Path $outDir "py_exit_stderr.log")

# Attendre que le noeud Python ait ecrit sa cle.
$deadline = (Get-Date).AddSeconds(10)
while (-not (Test-Path $keyFile) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 100
}
if (-not (Test-Path $keyFile)) {
    Stop-Process -Id $pyProc.Id -Force -ErrorAction SilentlyContinue
    throw "keyfile jamais ecrit - voir target\interop-exit-download\py_exit_stderr.log"
}

$dhtFlag = if ($Dht) { "--dht" } else { "" }
# La taille demandee figure dans la ligne de lancement : une execution
# sous-dimensionnee (ex. mauvais nom de parametre avale en silencieux)
# reste visible dans la sortie.
Write-Host "== telechargement rqbit via circuit ($Hops saut(s), sortie pyipv8$(if ($Dht) { ', DHT' }), payload demande = $Payload octets) =="
$rsErr = Join-Path $outDir "rust_exit_stderr.log"
cmd /c "`".\target\debug\examples\exit_download_interop.exe`" --keyfile `"$keyFile`" --hops $Hops --payload $Payload $dhtFlag > `"$rsLog`" 2> `"$rsErr`""
$rsOk = $LASTEXITCODE -eq 0

Stop-Process -Id $pyProc.Id -Force -ErrorAction SilentlyContinue

$rsStderr = Get-Content $rsErr -Raw -ErrorAction SilentlyContinue
Write-Host "---- rust stderr (extrait) ----"
($rsStderr -split "`n" | Select-String "READY|INTEROP|ECHEC|timeout" | Select-Object -Last 8) | ForEach-Object { $_.Line }

# La ligne OK rapporte la taille reellement verifiee : elle doit
# correspondre a la taille demandee.
$verified = [regex]::Match($rsStderr, "verifie=(\d+) octets")
if ($verified.Success -and [int64]$verified.Groups[1].Value -ne [int64]$Payload) {
    Write-Host "INTEROP EXIT DOWNLOAD ECHEC - taille verifiee $($verified.Groups[1].Value) <> demandee $Payload"
    exit 1
}

if ($rsOk) {
    Write-Host "INTEROP EXIT DOWNLOAD OK"
    exit 0
} else {
    Write-Host "INTEROP EXIT DOWNLOAD ECHEC - voir target\interop-exit-download\*.log"
    exit 1
}
