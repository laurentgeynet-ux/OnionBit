// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! « Démarrer avec Windows » — valeur `TriblerRustDaemon` de la clé
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (autostart par
//! utilisateur, sans droits admin). L'état coché du menu tray reflète
//! directement la présence de la valeur : le registre est la source
//! de vérité (survit à une réinstallation / réinitialisation de
//! `configuration.json`).

use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};
use winreg::RegKey;

/// Sous-clé Run standard de l'utilisateur courant.
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Nom de la valeur pour ce daemon.
const VALUE_NAME: &str = "TriblerRustDaemon";

/// `true` si la valeur Run existe (peu importe son contenu).
pub fn is_enabled() -> bool {
    open(KEY_QUERY_VALUE)
        .ok()
        .and_then(|key| key.get_value::<String, _>(VALUE_NAME).ok())
        .is_some()
}

/// Écrit la ligne de commande complète (`"exe" --state-dir "…"`).
pub fn set(command_line: &str) -> std::io::Result<()> {
    open(KEY_SET_VALUE)?.set_value(VALUE_NAME, &command_line)
}

/// Supprime la valeur ; `NotFound` n'est pas une erreur.
pub fn clear() -> std::io::Result<()> {
    match open(KEY_SET_VALUE)?.delete_value(VALUE_NAME) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        res => res,
    }
}

/// Inverse l'état courant ; retourne le nouvel état (`true` = activé).
pub fn toggle(command_line: &str) -> std::io::Result<bool> {
    if is_enabled() {
        clear()?;
        Ok(false)
    } else {
        set(command_line)?;
        Ok(true)
    }
}

fn open(access: u32) -> std::io::Result<RegKey> {
    RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_SUBKEY, access)
}
