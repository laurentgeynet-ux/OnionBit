/* This file is part of OnionBit.
 * Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
 * SPDX-License-Identifier: GPL-3.0-or-later */

/* Bornes de section pour l'instrumentation sancov sous COFF/MSVC.
 *
 * rustc emet des references `__start___sancov_*` / `__stop___sancov_*`
 * (convention ELF, synthetisees automatiquement par le linker ELF).
 * Sous COFF, rustc place les compteurs dans les groupes `.SCOV$*` et
 * `.SCOVP$*`, mais lld-link ne synthetise pas les symboles de bornes.
 * On les definit directement : le suffixe `$A`/$`Z` se trie avant/apres
 * `$CM`/`$M` dans le groupe fusionne, donc les variables nommees
 * `__start_*`/`__stop_*` encadrent exactement les donnees sancov.
 * Inutile sur Linux/ELF : le linker les synthetise nativement. */

#pragma section(".SCOV$A", read, write)
#pragma section(".SCOV$Z", read, write)
#pragma section(".SCOVP$A", read, write)
#pragma section(".SCOVP$Z", read, write)

__declspec(allocate(".SCOV$A"))
unsigned char __start___sancov_cntrs[1] = {0};
__declspec(allocate(".SCOV$Z"))
unsigned char __stop___sancov_cntrs[1] = {0};
__declspec(allocate(".SCOVP$A"))
unsigned long long __start___sancov_pcs[1] = {0};
__declspec(allocate(".SCOVP$Z"))
unsigned long long __stop___sancov_pcs[1] = {0};
