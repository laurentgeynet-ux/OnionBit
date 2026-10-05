# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# fuzz_corpus_replay.ps1 - integration post-campagne d'un corpus
# libFuzzer : minimisation par merge, copie vers le corpus versionne
# du harnais stable, puis replay CI.
#
# A executer UNE FOIS la campagne terminee (le corpus de
# fuzz/corpus/<target>/ ne doit plus etre ecrit par le fuzzer).
#
# Chaine (meme workflow que le commit 6b22263 sur messaging_frame) :
#   cargo +nightly fuzz run -s none <target> <corpus> -- -merge=1 <min>
#   copy <min> -> crates/<crate>/tests/<corpus_dir>/
#   cargo test -p <crate> --test <test_cible>
#
# Usage (shell admin non requis) :
#   .\scripts\fuzz_corpus_replay.ps1 -Target messaging_window
#   .\scripts\fuzz_corpus_replay.ps1 -Target messaging_window -SkipMerge -MinDir <dir>
#
# NOTE encodage : fichier volontairement ASCII.
param(
    [Parameter(Mandatory = $true)]
    [string]$Target,
    # Saute le merge : copie depuis -MinDir (corpus deja minimise).
    [switch]$SkipMerge,
    [string]$MinDir = "",
    # Override explicite du repertoire de destination versionne.
    [string]$DestDir = "",
    # Saute le replay final (copie seule).
    [switch]$SkipTest
)

$ErrorActionPreference = "Stop"
$Repo = Split-Path $PSScriptRoot
Set-Location $Repo

# Table cible -> (crate, sous-repertoire corpus versionne). Les cibles
# sans entree ci-dessous exigent -DestDir explicite.
$map = @{
    messaging_frame  = @{ crate = 'onionbit-messaging'; corpus = 'fuzz_corpus' }
    messaging_window = @{ crate = 'onionbit-messaging'; corpus = 'fuzz_corpus_window' }
}
$info = $map[$Target]
if ($DestDir -eq "") {
    if (-not $info) { throw "pas de mapping pour '$Target' - fournir -DestDir" }
    $crate = $info.crate
    $DestDir = Join-Path $Repo "crates\$crate\tests\$($info.corpus)"
} else {
    $crate = if ($info) { $info.crate } else { '' }
}
$srcCorpus = Join-Path $Repo "fuzz\corpus\$Target"

# --- Environnement Windows/MSVC (cf. fuzz_campaign.ps1) --------------
$llvmBin = "C:\Program Files\LLVM\bin"
if ((Test-Path "$llvmBin\clang-cl.exe") -and -not (Get-Command clang-cl -ErrorAction SilentlyContinue)) {
    $env:CC = "$llvmBin\clang-cl.exe"
    $env:CXX = "$llvmBin\clang++.exe"
    $env:PATH = "$llvmBin;$env:PATH"
}
if ($IsWindows -or $env:OS -eq "Windows_NT") {
    $fuzzerLib = Get-ChildItem "C:\Program Files\LLVM\lib\clang\*\lib\windows\clang_rt.fuzzer-x86_64.lib" -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1
    if (-not $fuzzerLib) { throw "clang_rt.fuzzer-x86_64.lib introuvable sous LLVM" }
    $env:CUSTOM_LIBFUZZER_PATH = $fuzzerLib.FullName
    $env:CUSTOM_LIBFUZZER_STD_CXX = "libcpmt"
    $msvcLib = Get-ChildItem "C:\Program Files\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\lib\x64" -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1
    $sdkRoot = "C:\Program Files (x86)\Windows Kits\10\Lib"
    $sdkVer = Get-ChildItem $sdkRoot -ErrorAction SilentlyContinue | Sort-Object Name | Select-Object -Last 1
    $env:LIB = "$($msvcLib.FullName);$sdkRoot\$($sdkVer.Name)\ucrt\x64;$sdkRoot\$($sdkVer.Name)\um\x64"
}

$commit = (git rev-parse --short HEAD)
Write-Host "== replay corpus $Target (commit $commit) =="

if (-not $SkipMerge) {
    if (-not (Test-Path $srcCorpus)) { throw "corpus source absent : $srcCorpus" }
    $nSrc = @(Get-ChildItem $srcCorpus -File).Count
    if ($MinDir -eq "") {
        $MinDir = Join-Path $Repo "fuzz\corpus\.min-$Target"
        Remove-Item -Recurse -Force $MinDir -ErrorAction SilentlyContinue
    }
    New-Item -ItemType Directory -Force -Path $MinDir | Out-Null
    Write-Host "merge libFuzzer : $nSrc inputs -> $MinDir"
    $ErrorActionPreference = "Continue"
    cargo +nightly fuzz run -O -s none $Target $srcCorpus -- "-merge=1" $MinDir 2>&1 |
        Tee-Object (Join-Path $Repo "fuzz\artifacts\merge-$Target.log") | Select-Object -Last 10
    $ErrorActionPreference = "Stop"
    if ($LASTEXITCODE -ne 0) { throw "merge libFuzzer echoue (voir fuzz\artifacts\merge-$Target.log)" }
}
$nMin = @(Get-ChildItem $MinDir -File -ErrorAction SilentlyContinue).Count
if ($nMin -eq 0) { throw "corpus minimise vide : $MinDir" }
Write-Host "corpus minimise : $nMin inputs"

# Copie vers le corpus versionne du harnais stable.
New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
Copy-Item "$MinDir\*" $DestDir -Force
Write-Host "copie -> $DestDir ($nMin fichiers)"

if (-not $SkipTest -and $crate -ne "") {
    Write-Host "replay stable : cargo test -p $crate --test fuzz_regression"
    cargo test -p $crate --test fuzz_regression -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "replay corpus echoue - un input fait diverger/paniquer" }
}
Write-Host "== replay $Target OK : $nMin inputs versionnes sous $DestDir =="
