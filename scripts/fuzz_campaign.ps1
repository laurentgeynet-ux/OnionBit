# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# OnionBit -- campagne libFuzzer (journal : docs/security/fuzz_journal.md)
#
# Prerequis :
#   rustup toolchain install nightly --profile minimal
#   rustup component add --toolchain nightly rust-src
#   cargo install cargo-fuzz
#   Windows : LLVM (clang_rt.fuzzer-*.lib) + Visual Studio (libs MSVC/SDK)
#   Linux   : clang suffit ; ASan est disponible (sanitizer par defaut)
#
# Usage :
#   .\scripts\fuzz_campaign.ps1                 # campagne complete (~5 h)
#   .\scripts\fuzz_campaign.ps1 -Smoke          # 60 s par cible (validation)
#   .\scripts\fuzz_campaign.ps1 -Target raw_datagram -Minutes 60
#
# Le corpus grossit dans fuzz/corpus/<target>/ -- a conserver (versionner
# les minima, pas les millions). Les crashes vont dans
# fuzz/artifacts/<target>/.
#
# NOTE encodage : fichier volontairement ASCII -- un caractere multi-octets
# (ex. tiret cadratin) lu en CP1252 par Windows PowerShell 5.1 peut
# generer un guillemet fermeant et casser le parsing.
param(
    [string]$Target = "",
    [int]$Minutes = 0,
    [switch]$Smoke
)

$ErrorActionPreference = "Stop"
$Repo = Split-Path $PSScriptRoot
Set-Location $Repo

# clang-cl pour libfuzzer-sys (build C). cc cherche cl.exe en priorite sous MSVC.
$llvmBin = "C:\Program Files\LLVM\bin"
if ((Test-Path "$llvmBin\clang-cl.exe") -and -not (Get-Command clang-cl -ErrorAction SilentlyContinue)) {
    $env:CC = "$llvmBin\clang-cl.exe"
    $env:CXX = "$llvmBin\clang++.exe"
    $env:PATH = "$llvmBin;$env:PATH"
    Write-Host "CC -> clang-cl ($llvmBin)"
}

# --- Environnement Windows/MSVC pour le fuzzing coverage-guide ----------
# rustc windows-msvc ne livre aucun runtime sanitizer ; sous COFF,
# l'instrumentation sancov emet des bornes `__start_/__stop_` que lld-link
# ne synthetise pas. Solution validee sur cette machine :
#   - `fuzz/sancov_shim.c` (compile par fuzz/build.rs) definit les bornes
#     `.SCOV$A`/`$Z` et `.SCOVP$A`/`$Z` ;
#   - CUSTOM_LIBFUZZER_PATH pointe vers le runtime fuzzer precompile de
#     LLVM (clang_rt.fuzzer-*.lib) : evite la recompilation de libFuzzer ;
#   - LIB fournit msvcrt/ucrt/STL statiques (Visual Studio requis).
$SanitizerArg = @()
if ($IsWindows -or $env:OS -eq "Windows_NT") {
    $fuzzerLib = Get-ChildItem "C:\Program Files\LLVM\lib\clang\*\lib\windows\clang_rt.fuzzer-x86_64.lib" -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1
    if (-not $fuzzerLib) {
        Write-Host "!! clang_rt.fuzzer-x86_64.lib introuvable sous LLVM -- installez LLVM ou utilisez WSL/Linux"
        exit 2
    }
    $env:CUSTOM_LIBFUZZER_PATH = $fuzzerLib.FullName
    $env:CUSTOM_LIBFUZZER_STD_CXX = "libcpmt"

    $msvcLib = Get-ChildItem "C:\Program Files\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\lib\x64" -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1
    $sdkRoot = "C:\Program Files (x86)\Windows Kits\10\Lib"
    $sdkVer = Get-ChildItem $sdkRoot -ErrorAction SilentlyContinue | Sort-Object Name | Select-Object -Last 1
    if (-not $msvcLib -or -not $sdkVer) {
        Write-Host "!! Libs MSVC/SDK introuvables -- installez Visual Studio Build Tools (workload C++)"
        exit 2
    }
    $env:LIB = "$($msvcLib.FullName);$sdkRoot\$($sdkVer.Name)\ucrt\x64;$sdkRoot\$($sdkVer.Name)\um\x64"
    # Pas de runtime ASan sous windows-msvc : coverage-guiding seul.
    $SanitizerArg = @("-s", "none")
    Write-Host "msvc-lib -> $($msvcLib.FullName)"
    Write-Host "fuzzer   -> $($fuzzerLib.FullName)"
}

if ($Smoke) {
    $plan = @(
        @{name="raw_datagram";     sec=60},
        @{name="tunnel_cell";      sec=60},
        @{name="tunnel_payloads";  sec=60},
        @{name="ipv8_packet";      sec=60},
        @{name="unsigned_dispatch";sec=60},
        @{name="utp_datagram";     sec=60}
    )
} elseif ($Target -ne "") {
    $sec = if ($Minutes -gt 0) { $Minutes * 60 } else { 3600 }
    $plan = @(@{name=$Target; sec=$sec})
} else {
    # Campagne de reference (cf. docs/security/fuzz_journal.md)
    $plan = @(
        @{name="raw_datagram";      sec=3600},
        @{name="tunnel_cell";       sec=3600},
        @{name="unsigned_dispatch"; sec=3600},
        @{name="tunnel_payloads";   sec=3600},
        @{name="ipv8_packet";       sec=1800},
        @{name="utp_datagram";      sec=1800}
    )
}

$commit = (git rev-parse --short HEAD)
$date = (Get-Date -Format "yyyy-MM-dd HH:mm")
$journal = "$Repo\docs\security\fuzz_journal.csv"
if (-not (Test-Path $journal)) {
    "date,commit,target,duree_s,execs,crashes,exit_code" | Out-File $journal -Encoding utf8
}

foreach ($t in $plan) {
    $name = $t.name; $sec = $t.sec
    Write-Host ""
    Write-Host "=== fuzz $name  (${sec}s)  commit $commit ==="
    $log = "$Repo\fuzz\artifacts\last-run-$name.log"
    New-Item -ItemType Directory -Force "$Repo\fuzz\artifacts" | Out-Null

    # EAP=Stop + redirection stderr d'un natif = NativeCommandError sous
    # PS 5.1 : on retombe sur Continue pendant l'appel, $LASTEXITCODE
    # conserve le vrai statut.
    $ErrorActionPreference = "Continue"
    cargo +nightly fuzz run -O @SanitizerArg $name -- "-max_total_time=$sec" "-print_final_stats=1" 2>&1 |
        Tee-Object -FilePath $log | Select-Object -Last 12
    $ErrorActionPreference = "Stop"

    $code = $LASTEXITCODE
    $execs = ""
    $m = Select-String -Path $log -Pattern '#(\d+)\s+(DONE|pulse|REDUCE|NEW)' |
         Select-Object -Last 1
    if ($m) { $execs = $m.Matches[0].Groups[1].Value }
    $crashes = (Get-ChildItem "$Repo\fuzz\artifacts\$name" -ErrorAction SilentlyContinue | Measure-Object).Count

    "$date,$commit,$name,$sec,$execs,$crashes,$code" | Out-File $journal -Append -Encoding utf8
    Write-Host "journal -> $name : execs=$execs crashes=$crashes code=$code"
    if ($code -ne 0) {
        Write-Host "!! CRASH sur $name -- artefacts dans fuzz\artifacts\$name"
        Write-Host "    cycle : minimize -> proptest regression -> fix -> relancer"
    }
}
Write-Host ""
Write-Host "Campagne terminee. Journal : $journal"
