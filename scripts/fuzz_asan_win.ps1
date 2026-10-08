# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# fuzz_asan_win.ps1 - campagne ASan sous Windows/MSVC.
#
# rustup ne livre pas librustc-nightly_rt.asan.a sur
# x86_64-pc-windows-msvc (MCP compiler-team#702) : on lie le runtime
# clang externe. UBSan n'existe pas sur cette cible ; la campagne
# complete ASan+UBSan reste Linux/CI (scripts/fuzz_asan.sh, job
# fuzz-san de ci.yml).
#
# Recette validee 2026-10-05 (smoke messaging_window 60 s : 291 420
# execs, RSS pic 402 Mo, 0 crash) :
#   -Zexternal-clangrt + link-args sur l'import lib et le thunk
#   clang_rt.asan_* — JAMAIS /WHOLEARCHIVE sur le thunk : son objet
#   sanitizer_coverage_win_sections redefinit __start___sancov_*
#   (deja fourni par sancov_shim.lib de libfuzzer-sys). Sans
#   wholearchive, lld ne tire que les objets references.
#   Le DLL clang_rt.asan_dynamic-x86_64.dll doit etre sur PATH.
#
# Usage (shell non-admin OK) :
#   .\scripts\fuzz_asan_win.ps1                        # toutes cibles, 10 min
#   .\scripts\fuzz_asan_win.ps1 -Target messaging_window -Sec 60
#
# NOTE encodage : fichier volontairement ASCII.

param(
    [string]$Target = "-",   # "-" = toutes les cibles
    [int]$Sec = 600
)

$ErrorActionPreference = "Stop"
$Repo = Split-Path $PSScriptRoot
Set-Location $Repo

$llvmBin  = "C:\Program Files\LLVM\bin"
$clangRt  = Get-ChildItem "C:\Program Files\LLVM\lib\clang\*\lib\windows" -ErrorAction SilentlyContinue |
    Sort-Object FullName | Select-Object -Last 1
if (-not (Test-Path "$llvmBin\clang-cl.exe") -or -not $clangRt) {
    throw "LLVM/clang-cl ou le repertoire clang_rt introuvable sous C:\Program Files\LLVM"
}
$env:CC  = "$llvmBin\clang-cl.exe"
$env:CXX = "$llvmBin\clang++.exe"
$env:PATH = "$llvmBin;$($clangRt.FullName);$env:PATH"   # DLL asan a l'execution
$fuzzerLib = Join-Path $clangRt.FullName "clang_rt.fuzzer-x86_64.lib"
if (-not (Test-Path $fuzzerLib)) { throw "clang_rt.fuzzer-x86_64.lib absent : $fuzzerLib" }
$env:CUSTOM_LIBFUZZER_PATH    = $fuzzerLib
$env:CUSTOM_LIBFUZZER_STD_CXX = "libcpmt"

$msvcLib = Get-ChildItem "C:\Program Files\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\lib\x64" -ErrorAction SilentlyContinue |
    Sort-Object FullName | Select-Object -Last 1
$sdkRoot = "C:\Program Files (x86)\Windows Kits\10\Lib"
$sdkVer  = Get-ChildItem $sdkRoot -ErrorAction SilentlyContinue | Sort-Object Name | Select-Object -Last 1
$env:LIB = "$($msvcLib.FullName);$sdkRoot\$($sdkVer.Name)\ucrt\x64;$sdkRoot\$($sdkVer.Name)\um\x64;$($clangRt.FullName)"

# Runtime ASan externe : import lib + thunk (lien selectif, cf. en-tete).
$env:RUSTFLAGS = "-Zexternal-clangrt " +
    "-Clink-arg=clang_rt.asan_dynamic-x86_64.lib " +
    "-Clink-arg=clang_rt.asan_dynamic_runtime_thunk-x86_64.lib"

$targets = @("raw_datagram", "tunnel_cell", "tunnel_payloads",
    "ipv8_packet", "unsigned_dispatch", "utp_datagram",
    "messaging_frame", "messaging_window", "obd_file")
if ($Target -ne "-") { $targets = @($Target) }

$journal = Join-Path $Repo "fuzz\artifacts\fuzz_journal_san_win.csv"
New-Item -ItemType Directory -Force -Path (Join-Path $Repo "fuzz\artifacts") | Out-Null
if (-not (Test-Path $journal)) {
    "date,commit,target,san,duree_s,execs,crashes,exit_code" | Out-File $journal -Encoding ascii
}
$commit = (git rev-parse --short HEAD)
$fails = 0

foreach ($t in $targets) {
    Write-Host "`n=== fuzz $t  san=address  ${Sec}s  commit $commit ==="
    $log = Join-Path $Repo "fuzz\artifacts\asan-last-run-$t-address.log"
    $ErrorActionPreference = "Continue"
    cargo +nightly fuzz run -O -s address $t -- "-max_total_time=$Sec" "-print_final_stats=1" 2>&1 |
        Tee-Object $log | Select-Object -Last 12
    $code = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    $execs = (Select-String -Path $log -Pattern '#(\d+)\s+(DONE|pulse|REDUCE|NEW)' -AllMatches |
        Select-Object -Last 1).Matches.Groups[1].Value
    $crashes = @(Get-ChildItem "fuzz\artifacts\$t" -File -ErrorAction SilentlyContinue).Count
    # Garde « sanitizer arme » : le binaire doit importer la DLL asan.
    $bin = Get-ChildItem "fuzz\target\x86_64-pc-windows-msvc\release\$t.exe" -ErrorAction SilentlyContinue
    $armed = $bin -and ((Select-String -Path $bin.FullName -Pattern 'clang_rt.asan' -SimpleMatch -ErrorAction SilentlyContinue) -or
             (& "$llvmBin\llvm-nm.exe" $bin.FullName 2>$null | Select-String '__asan_' -Quiet))
    if ($code -eq 0 -and -not $armed) {
        Write-Host "!! $t : clang_rt.asan non reference - sanitizer non arme"
        $code = 42
    }
    "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss'),$commit,$t,address,$Sec,$execs,$crashes,$code" |
        Out-File $journal -Append -Encoding ascii
    Write-Host "journal -> $t : execs=$execs crashes=$crashes code=$code armed=$armed"
    if ($code -ne 0) { $fails++ }
}
Write-Host "`nCampagne ASan Windows terminee ($fails echec(s)). Journal : $journal"
exit $fails
