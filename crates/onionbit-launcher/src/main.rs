// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-launcher` — lanceur portable du bundle Windows (ADR-0018).
//!
//! Le script `build_dist.ps1` copie ce ~30 Ko trois fois a la racine du
//! bundle sous des noms parlants ; chaque copie choisit sa cible d'apres
//! **son propre nom de fichier** et resout `windows\<cible>`
//! relativement a sa position :
//!
//! - `OnionBit.exe`        → `windows\OnionBit.exe` (UI desktop)
//! - `OnionBit Daemon.exe` → `windows\onionbit-daemon.exe`
//! - `OnionBit Web.exe`    → `windows\onionbit-daemon.exe --open-webui`
//!
//! Pourquoi pas des `.lnk` ? `WScript.Shell.CreateShortcut` grave le
//! **chemin absolu de la machine de build** dans `TargetPath` ; Windows
//! le reutilise tant qu'il existe — une copie du bundle sur Desktop ou
//! une cle USB lancait alors l'ancienne installation (`dist\`), avec son
//! propre `state\`. Le tracking NTFS ne sait pas non plus suivre le
//! raccourci lui-meme. Un exe resolvant relativement (`current_exe`)
//! est la convention PortableApps : aucun chemin fige, aucune console
//! parasite (sous-systeme GUI), icone embarquee.
//!
//! Les arguments de la ligne de commande sont relayes apres les
//! arguments fixes. Nom de fichier inconnu (exe renomme) → repli sur
//! l'UI desktop. Hors Windows : stub informatif, le crate reste dans le
//! workspace pour `cargo check` croise.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::ffi::OsStr;
use std::process::ExitCode;

/// Sous-repertoire du bundle contenant le payload de l'OS (ADR-0018 :
/// `<bundle>\windows\` ; le marqueur `OnionBit.portable` est a cote).
const PAYLOAD_DIR: &str = "windows";

/// Cible resolue : exe sous `windows\` + arguments fixes du role.
struct Target {
    exe: &'static str,
    args: &'static [&'static str],
}

/// Choix de la cible d'apres le nom de fichier du lanceur (copie de
/// `build_dist.ps1`). Repli `_` → UI desktop : un exe renomme par
/// l'utilisateur doit rester lancable plutot que de faire no-op.
fn target_for(stem: &str) -> Target {
    match stem {
        "OnionBit Daemon" => Target {
            exe: "onionbit-daemon.exe",
            args: &[],
        },
        "OnionBit Web" => Target {
            exe: "onionbit-daemon.exe",
            args: &["--open-webui"],
        },
        _ => Target {
            exe: "OnionBit.exe",
            args: &[],
        },
    }
}

fn run() -> Result<(), String> {
    let self_exe =
        std::env::current_exe().map_err(|e| format!("chemin du lanceur illisible : {e}"))?;
    let dir = self_exe
        .parent()
        .ok_or_else(|| "lanceur sans dossier parent".to_string())?;
    let stem = self_exe
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    let target = target_for(stem);
    let payload = dir.join(PAYLOAD_DIR);
    let exe = payload.join(target.exe);
    if !exe.is_file() {
        return Err(format!("cible introuvable : {}", exe.display()));
    }
    std::process::Command::new(&exe)
        .args(target.args)
        .args(std::env::args_os().skip(1))
        .current_dir(&payload)
        .spawn()
        .map_err(|e| format!("lancement de {} : {e}", exe.display()))?;
    Ok(())
}

#[cfg(windows)]
fn report_error(msg: &str) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| -> Vec<u16> { OsStr::new(s).encode_wide().chain(Some(0)).collect() };
    let text = wide(&format!(
        "{msg}\n\nRe-copiez le bundle OnionBit complet (le dossier \
         « windows » doit rester a cote de ce lanceur)."
    ));
    let caption = wide("OnionBit");
    // Lanceur GUI sans console : l'echec de lancement serait sinon
    // invisible — boite modale native, pas de fenetre parasite.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONERROR | MB_OK,
        );
    }
}

#[cfg(not(windows))]
fn report_error(msg: &str) {
    eprintln!("onionbit-launcher : {msg}");
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            report_error(&msg);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn nom_ui_pointe_sur_onionbit_exe() {
        let t = target_for("OnionBit");
        assert_eq!(t.exe, "OnionBit.exe");
        assert!(t.args.is_empty());
    }

    #[test]
    fn nom_daemon_sans_arguments() {
        let t = target_for("OnionBit Daemon");
        assert_eq!(t.exe, "onionbit-daemon.exe");
        assert!(t.args.is_empty());
    }

    #[test]
    fn nom_web_ouvre_lui_web() {
        let t = target_for("OnionBit Web");
        assert_eq!(t.exe, "onionbit-daemon.exe");
        assert_eq!(t.args, &["--open-webui"]);
    }

    #[test]
    fn nom_inconnu_replie_sur_ui() {
        for stem in ["onionbit-launcher", "Mon Shortcut", "", "OnionBit (2)"] {
            let t = target_for(stem);
            assert_eq!(t.exe, "OnionBit.exe", "stem {stem:?}");
        }
    }

    #[test]
    fn cible_sous_le_payload_os() {
        // Le chemin resolu reste relatif au lanceur — jamais absolu
        // fige au build (c'est le point de l'exe vs .lnk).
        let dir = Path::new("E:\\cle-usb\\OnionBit");
        let t = target_for("OnionBit Web");
        let exe = dir.join(PAYLOAD_DIR).join(t.exe);
        assert!(exe.starts_with(dir));
        assert_eq!(exe.file_name().and_then(OsStr::to_str), Some(t.exe));
    }
}
