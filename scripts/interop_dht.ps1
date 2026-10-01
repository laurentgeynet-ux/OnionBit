# interop_dht.ps1 - echange reproductible Rust <-> DHTCommunity pyipv8.
#
# Jalon de l'etape 10 : find_values/store_value dans les DEUX sens
# contre le vrai DHTCommunity Python, acceptation et refus des tokens
# anti-spoofing, y compris apres rotation des secrets cote Python.
#
# Preuves attendues (marqueurs) :
#   Rust   : RUST_FIND_OK, RUST_STORE_OK, RUST_STALE_REJECTED,
#            RUST_REFRESHED_STORE_OK
#   Python : PY_FIND_OK, PY_STORE_OK, PY_VERIFY_OK (relecture chez
#            Rust), PY_BADTOKEN_REJECTED, PY_READS_RUST_OK,
#            TOKENS_ROTATED
#
# Usage : powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\interop_dht.ps1
# Prerequis : venv interop (voir scripts/interop_ipv8.ps1).

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$pyipv8 = if ($env:TRIBLER_PYIPV8) { $env:TRIBLER_PYIPV8 } else { "D:\Projet\Tribler_sources\tribler\pyipv8" }
$venvPy = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
$outDir = Join-Path $root "target\interop-dht"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$pyPort = 12100      # endpoint DHT pyipv8
$rsPort = 12101      # endpoint DHT Rust
$pyLog = Join-Path $outDir "py_dht_stdout.log"
$pyErr = Join-Path $outDir "py_dht_stderr.log"
$rsLog = Join-Path $outDir "rust_dht_stderr.log"
$rsKeyFile = Join-Path $outDir "rust_key.txt"
$pyKeyFile = Join-Path $outDir "py_key.txt"

Remove-Item -Force -ErrorAction SilentlyContinue $rsKeyFile, $pyKeyFile, $pyLog, $pyErr, $rsLog

Write-Host "== build dht_interop_node (rust) =="
cargo build -p onionbit-ipv8 --example dht_interop_node
if ($LASTEXITCODE -ne 0) { throw "build echoue" }

# Le noeud Rust demarre en premier : il ecrit sa cle puis attend le
# ping du Python (le Python a besoin de rust-key pour
# on_node_discovered + le store bidon).
Write-Host "== noeud Rust DHT sur 127.0.0.1:$rsPort =="
$rsProc = Start-Process -FilePath ".\target\debug\examples\dht_interop_node.exe" -PassThru -NoNewWindow `
    -ArgumentList "--port $rsPort --py-addr 127.0.0.1:$pyPort --py-key-file `"$pyKeyFile`" --key-file `"$rsKeyFile`" --duration 28" `
    -RedirectStandardError $rsLog

$deadline = (Get-Date).AddSeconds(10)
while (-not (Test-Path $rsKeyFile) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 100
}
if (-not (Test-Path $rsKeyFile)) {
    Stop-Process -Id $rsProc.Id -Force -ErrorAction SilentlyContinue
    throw "rust keyfile jamais ecrit"
}
$rustKey = (Get-Content $rsKeyFile -Raw).Trim()

$env:PYTHONPATH = $pyipv8
Write-Host "== noeud Python DHT sur 127.0.0.1:$pyPort =="
$pyProc = Start-Process -FilePath $venvPy -PassThru -NoNewWindow `
    -ArgumentList "`"$PSScriptRoot\interop\py_dht_node.py`" --port $pyPort --rust-addr 127.0.0.1:$rsPort --rust-key $rustKey --key-file `"$pyKeyFile`" --duration 26" `
    -RedirectStandardOutput $pyLog -RedirectStandardError $pyErr

$pyProc.WaitForExit()
$rsProc.WaitForExit()

$py = Get-Content $pyLog -Raw -ErrorAction SilentlyContinue
$rs = Get-Content $rsLog -Raw -ErrorAction SilentlyContinue

$expected = @{
    "rust:RUST_FIND_OK"              = $rs -match "RUST_FIND_OK"
    "rust:RUST_STORE_OK"             = $rs -match "RUST_STORE_OK"
    "rust:RUST_STALE_REJECTED"       = $rs -match "RUST_STALE_REJECTED"
    "rust:RUST_REFRESHED_STORE_OK"   = $rs -match "RUST_REFRESHED_STORE_OK"
    "py:PY_FIND_OK"                  = $py -match "PY_FIND_OK"
    "py:PY_STORE_OK"                 = $py -match "PY_STORE_OK"
    "py:PY_VERIFY_OK"                = $py -match "PY_VERIFY_OK"
    "py:PY_BADTOKEN_REJECTED"        = $py -match "PY_BADTOKEN_REJECTED"
    "py:PY_READS_RUST_OK"            = $py -match "PY_READS_RUST_OK"
    "py:TOKENS_ROTATED"              = $py -match "TOKENS_ROTATED"
}
$allOk = $true
foreach ($k in $expected.Keys | Sort-Object) {
    $ok = $expected[$k]
    if (-not $ok) { $allOk = $false }
    Write-Host ("{0} : {1}" -f $k, $(if ($ok) { "OK" } else { "ABSENT" }))
}

if ($allOk) {
    Write-Host "INTEROP DHT OK"
    exit 0
} else {
    Write-Host "INTEROP DHT ECHEC - voir target\interop-dht\*.log"
    exit 1
}
