// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `--console` : rattachement de la console Windows au binaire
//! sous-système GUI (`#![windows_subsystem = "windows"]`).

/// Rattache le processus à la console parente ; à défaut en alloue une
/// nouvelle (`AttachConsole` échoue quand le lanceur n'a pas de
/// console — ex. double-clic depuis Explorer). Les handles std déjà
/// valides (redirection vers un fichier/pipe par le parent) sont
/// préservés : seuls les handles absents sont branchés sur `CONOUT$`.
///
/// Retourne `true` si une console est disponible pour les logs stdout.
#[cfg(windows)]
pub fn attach() -> bool {
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS,
        STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 && AllocConsole() == 0 {
            return false;
        }
        let conout: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        for std_handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let current: HANDLE = GetStdHandle(std_handle);
            // Le handle existe déjà (ex. `-RedirectStandardOutput` du
            // lanceur) : ne pas le détourner vers la console.
            if !current.is_null() && current != INVALID_HANDLE_VALUE {
                continue;
            }
            let h: HANDLE = CreateFileW(
                conout.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_EXISTING,
                0,
                null_mut(),
            );
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                SetStdHandle(std_handle, h);
            }
        }
        true
    }
}

/// `true` si écrire sur stdout a un sens (console attachée ou handle
/// redirigé vers un fichier/pipe par le lanceur — ex.
/// `-RedirectStandardOutput`). `false` en sous-système GUI sans
/// console : la couche stdout de `tracing` est alors inutile.
#[cfg(windows)]
pub fn stdout_available() -> bool {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        !h.is_null() && h != INVALID_HANDLE_VALUE
    }
}

/// Hors Windows le binaire reste en sous-système console : toujours
/// « attaché » / stdout toujours utilisable.
#[cfg(not(windows))]
pub fn attach() -> bool {
    true
}

#[cfg(not(windows))]
pub fn stdout_available() -> bool {
    true
}
