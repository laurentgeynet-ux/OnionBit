# This file is part of OnionBit - a Rust port of the Tribler daemon.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later

param([string]$Path)
$errs = $null
[void][System.Management.Automation.PSParser]::Tokenize((Get-Content $Path -Raw), [ref]$errs)
if ($errs -and $errs.Count -gt 0) { $errs | ForEach-Object { $_.Message }; exit 1 }
'syntaxe OK'
