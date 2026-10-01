# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# interop_tunnel.ps1 - echange reproductible Rust <-> TunnelCommunity pyipv8.
#
# Jalon du roadmap : le noeud Rust cree un circuit vers le vrai
# TunnelCommunity Python (relais+exit), envoie un datagramme "uTP"
# vers l'echo UDP cote Python a travers la sortie, et recoit la
# reponse par le circuit. Prouve create/created + crypto par couches
# + sortie bidirectionnelle contre le code de reference.
#
# Usage : powershell -NoProfile -ExecutionPolicy RemoteSigned -File scripts\interop_tunnel.ps1
# Prerequis : venv interop (voir scripts/interop_ipv8.ps1).

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$pyipv8 = if ($env:TRIBLER_PYIPV8) { $env:TRIBLER_PYIPV8 } else { "D:\Projet\Tribler_sources\tribler\pyipv8" }
$venvPy = if ($env:TRIBLER_INTEROP_PY) { $env:TRIBLER_INTEROP_PY } else { "D:\Projet\Tribler_sources\.venv-interop\Scripts\python.exe" }
$outDir = Join-Path $root "target\interop-tunnel"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$pyPort = 12090      # endpoint tunnel pyipv8
$echoPort = 12091    # echo UDP (destination de sortie)
$rsPort = 12092      # endpoint tunnel Rust
$pyLog = Join-Path $outDir "py_tunnel_packets.log"
$rsLog = Join-Path $outDir "rust_tunnel_packets.log"
$keyFile = Join-Path $outDir "py_key.txt"

Remove-Item -Force -ErrorAction SilentlyContinue $keyFile

Write-Host "== build tunnel_interop_node (rust) =="
cargo build -p onionbit-tunnel --example tunnel_interop_node
if ($LASTEXITCODE -ne 0) { throw "build echoue" }

$env:PYTHONPATH = $pyipv8
Write-Host "== noeud Python tunnel sur 127.0.0.1:$pyPort (echo :$echoPort) =="
$pyProc = Start-Process -FilePath $venvPy -PassThru -NoNewWindow `
    -ArgumentList "`"$PSScriptRoot\interop\py_tunnel_node.py`" --port $pyPort --echo-port $echoPort --keyfile `"$keyFile`" --log `"$pyLog`" --duration 15" `
    -RedirectStandardError (Join-Path $outDir "py_tunnel_stderr.log")

# Attendre que le noeud Python ait ecrit sa cle.
$deadline = (Get-Date).AddSeconds(10)
while (-not (Test-Path $keyFile) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 100
}
if (-not (Test-Path $keyFile)) {
    Stop-Process -Id $pyProc.Id -Force -ErrorAction SilentlyContinue
    throw "keyfile jamais ecrit - voir target\interop-tunnel\py_tunnel_stderr.log"
}

Write-Host "== noeud Rust tunnel sur 127.0.0.1:$rsPort =="
# Invocation synchrone via cmd : $LASTEXITCODE est fiable (ExitCode
# de Start-Process est vide quand stderr est redirige, et `2>` en PS5
# transforme le stderr en NativeCommandError avec Stop).
$rsErr = Join-Path $outDir "rust_tunnel_stderr.log"
cmd /c "`".\target\debug\examples\tunnel_interop_node.exe`" --port $rsPort --keyfile `"$keyFile`" --echo 127.0.0.1:$echoPort --duration 12 --log `"$rsLog`" 2> `"$rsErr`""
$rsOk = $LASTEXITCODE -eq 0

# Pas de verify_packets.py ici : les cellules de tunnel ne sont pas
# des paquets IPv8 signes (msg_id 0) — l'echange chiffre est sa propre
# preuve (le decrypt_str pyipv8 a accepte nos cellules).

$pyProc.WaitForExit()
# Succes Python : un exit socket a ete cree (le log stderr le dit).
$pyStderr = Get-Content (Join-Path $outDir "py_tunnel_stderr.log") -Raw -ErrorAction SilentlyContinue
$pyOk = $pyStderr -match "exit_sockets=[1-9]"

Write-Host "rust:$rsOk py:$pyOk"
if ($rsOk -and $pyOk) {
    Write-Host "INTEROP TUNNEL OK"
    exit 0
} else {
    Write-Host "INTEROP TUNNEL ECHEC - voir target\interop-tunnel\*.log"
    exit 1
}
