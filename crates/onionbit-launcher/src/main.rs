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
use std::path::Path;
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

/// Mise a jour signee `OBUP` (ADR-0032, etape 144) — seconde porte
/// de confiance. Le launcher **re-verifie tout** avant le premier
/// renommage : signature + `key_id` du blob stage, puis `sha256`/
/// `size` de chaque fichier de `state/update/payload/` (jamais
/// `incoming/` — il peut avoir ete vide). Le state dir est
/// modifiable entre l'import et ce boot : aucune confiance perimee.
///
/// Swap en deux phases — retrait puis installation :
///
/// 1. `windows/` → `windows.old/` (purge d'un `windows.old`
///    precedent au prealable — une cible non vide fait echouer le
///    rename Windows) ; tout autre fichier racine present dans le
///    payload est retire en `<nom>.update-old` (Windows permet de
///    renommer un exe en cours — le lanceur peut ainsi mettre a jour
///    ses propres copies) ;
/// 2. chaque entree de `payload/` est deplacee a la racine ;
/// 3. purge du staging (`staged.obup` + `payload/`).
///
/// Un daemon actif est detecte **avant** tout renommage (sonde du
/// mutex d'instance unique sous Windows — les images ouvertes
/// verrouilleraient `windows/` au rename).
///
/// Crash mid-swap : `windows/` absent + `windows.old/` present au
/// boot suivant → restauration (voir [`recover_interrupted_swap`]).
fn apply_staged_update(dir: &Path) -> Result<bool, String> {
    let state_dir = dir.join("state");
    let upd = state_dir.join("update");
    let staged = upd.join("staged.obup");
    let payload = upd.join("payload");
    if !staged.is_file() {
        return Ok(false);
    }
    let blob = std::fs::read(&staged).map_err(|e| format!("staged.obup : {e}"))?;
    let keys = onionbit_crypto::release::accepted_keys(&state_dir);
    let manifest = onionbit_format::obupd::verify(&blob, &keys)
        .map_err(|e| format!("mise a jour rejetee : {e}"))?;
    if !payload.is_dir() {
        return Err("staging incomplet : payload/ absent".to_string());
    }
    // Re-verification integrale des fichiers *stages*.
    use sha2::Digest;
    for f in &manifest.files {
        let p = payload.join(&f.path);
        let data = std::fs::read(&p).map_err(|_| format!("fichier stage absent : {}", f.path))?;
        let sha: [u8; 32] = sha2::Sha256::digest(&data).into();
        if data.len() as u64 != f.size || sha != f.sha256 {
            return Err(format!("payload altere : {}", f.path));
        }
    }

    // Un daemon actif verrouille `windows/` (images ouvertes) — sonde
    // avant tout renommage, sinon le swap s'arreterait a moitie fait.
    #[cfg(windows)]
    if daemon_running(&state_dir) {
        return Err(
            "une instance OnionBit tourne — fermez-la puis relancez pour appliquer la mise a jour"
                .to_string(),
        );
    }

    let win = dir.join(PAYLOAD_DIR);
    let old = dir.join("windows.old");

    // Phase 1 — retrait : purge des reliquats, puis renommages.
    if old.exists() {
        std::fs::remove_dir_all(&old).map_err(|e| {
            format!("purge de windows.old impossible ({e}) — staging conserve, reessayez")
        })?;
    }
    let entries: Vec<_> = std::fs::read_dir(&payload)
        .map_err(|e| format!("payload illisible : {e}"))?
        .flatten()
        .collect();
    let mut retired = Vec::new();
    for e in &entries {
        let dst = dir.join(e.file_name());
        if e.file_name() == PAYLOAD_DIR {
            if win.exists() {
                std::fs::rename(&win, &old).map_err(|e| {
                    format!(
                        "impossible de renommer windows/ — une instance est peut-etre active ({e})"
                    )
                })?;
                retired.push(dst);
            }
        } else if dst.exists() {
            let backup = dir.join(format!("{}.update-old", e.file_name().to_string_lossy()));
            if backup.exists() {
                let _ = std::fs::remove_dir_all(&backup).or_else(|_| std::fs::remove_file(&backup));
            }
            let name = e.file_name().to_string_lossy().to_string();
            std::fs::rename(&dst, &backup)
                .map_err(|err| format!("retrait de {name} impossible ({err})"))?;
            retired.push(dst);
        }
    }

    // Phase 2 — installation. En cas d'echec : restauration
    // best-effort de ce qui a ete retire, staging conserve.
    for e in &entries {
        let dst = dir.join(e.file_name());
        if let Err(err) = std::fs::rename(e.path(), &dst) {
            for r in &retired {
                let b = if r.file_name().and_then(OsStr::to_str) == Some(PAYLOAD_DIR) {
                    old.clone()
                } else {
                    dir.join(format!(
                        "{}.update-old",
                        r.file_name().unwrap_or_default().to_string_lossy()
                    ))
                };
                let _ = std::fs::rename(&b, r);
            }
            return Err(format!(
                "installation de {} impossible ({err}) — installation precedente restauree",
                e.file_name().to_string_lossy()
            ));
        }
    }

    // Phase 3 — purge du staging : le blob n'est retire qu'apres
    // l'installation complete.
    let _ = std::fs::remove_file(&staged);
    let _ = std::fs::remove_dir_all(&payload);
    Ok(true)
}

/// Reprise d'un swap interrompu (crash entre les renommages) :
/// `windows/` absent + `windows.old/` present → restauration ;
/// `<x>.update-old` orphelin → meme traitement. Le staging est
/// conserve : le swap sera retente au boot suivant.
fn recover_interrupted_swap(dir: &Path) -> Result<(), String> {
    let win = dir.join(PAYLOAD_DIR);
    let old = dir.join("windows.old");
    if !win.exists() && old.is_dir() {
        std::fs::rename(&old, &win).map_err(|e| format!("reprise windows.old : {e}"))?;
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(base) = name.strip_suffix(".update-old") {
                let orig = dir.join(base);
                if !orig.exists() {
                    let _ = std::fs::rename(e.path(), orig);
                }
            }
        }
    }
    Ok(())
}

/// Sonde best-effort du mutex d'instance unique du daemon — miroir
/// de `onionbit-daemon::instance` (le launcher reste un crate
/// minimal sans dep au daemon). `true` = une instance tient le
/// verrou pour ce `state_dir`.
#[cfg(windows)]
fn daemon_running(state_dir: &Path) -> bool {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let abs = std::path::absolute(state_dir).unwrap_or_else(|_| state_dir.to_path_buf());
    let mut h = DefaultHasher::new();
    abs.to_string_lossy().to_lowercase().hash(&mut h);
    let mut name: Vec<u16> = format!("Local\\TriblerRustDaemon-{}\0", h.finish())
        .encode_utf16()
        .collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_mut_ptr());
        if handle.is_null() {
            return false; // sonde impossible — le rename decidera
        }
        let running = GetLastError() == ERROR_ALREADY_EXISTS;
        CloseHandle(handle);
        running
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
    // Reprise d'un swap interrompu AVANT toute chose (pattern
    // `.plain.bak` de la migration DB : l'etat detectable est
    // toujours reparable).
    recover_interrupted_swap(dir)?;
    // Mise a jour stagee : reverification complete + swap. Un blob
    // invalide refuse le lancement — `state/` tripatouille n'est
    // pas une raison de demarrer comme si de rien n'etait.
    let applied = apply_staged_update(dir)?;
    // Fenetre de rollback manuel = premier run : `windows.old` est
    // conserve le temps d'un demarrage du nouveau, purge best-effort
    // ensuite (fichiers verrouilles → conserve, purge retentee).
    if !applied {
        let old = dir.join("windows.old");
        if old.exists() {
            let _ = std::fs::remove_dir_all(&old);
        }
    }
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

    // -- Swap `OBUP` (ADR-0032, etape 144) -------------------------
    // La placeholder `RELEASE_PUBKEY` est fail-closed : les tests
    // inscrivent leur clef dans `state/release_key.next`.

    fn keypair() -> (ed25519_dalek::SigningKey, [u8; 32]) {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[0x5Au8; 32]);
        let pk = sk.verifying_key().to_bytes();
        (sk, pk)
    }

    fn make_obup(sk: &ed25519_dalek::SigningKey, files: &[(String, Vec<u8>)]) -> Vec<u8> {
        use onionbit_format::obupd as obup;
        use sha2::Digest;
        let pk = sk.verifying_key().to_bytes();
        let m = obup::UpdateManifest {
            version: "9.9.9".into(),
            target: "windows-x64".into(),
            ts: 1_700_000_000,
            files: files
                .iter()
                .map(|(p, d)| obup::ObupFile {
                    path: p.clone(),
                    sha256: sha2::Sha256::digest(d).into(),
                    size: d.len() as u64,
                })
                .collect(),
            min_supported: None,
            rollback: false,
            next_key: None,
            notes: None,
            key_id: onionbit_crypto::release::release_key_id(&pk),
        };
        let enc = obup::encode_manifest(&m);
        let sig = onionbit_crypto::release::sign_update_manifest(sk, &enc);
        obup::build(&m, m.key_id, &sig)
    }

    /// Bundle factice : `windows/` ancien + staging signe complet.
    fn fake_bundle(files: &[(String, Vec<u8>)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let (sk, pk) = keypair();
        let state = dir.path().join("state");
        std::fs::create_dir_all(state.join("update/payload")).unwrap();
        std::fs::write(state.join("release_key.next"), pk).unwrap();
        std::fs::create_dir_all(dir.path().join("windows")).unwrap();
        std::fs::write(dir.path().join("windows/ancien.exe"), b"OLD").unwrap();
        for (p, d) in files {
            let dst = state.join("update/payload").join(p);
            std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
            std::fs::write(&dst, d).unwrap();
        }
        std::fs::write(state.join("update/staged.obup"), make_obup(&sk, files)).unwrap();
        dir
    }

    #[test]
    fn swap_complet_remplace_windows() {
        let files = vec![("windows/neuf.exe".to_string(), b"NEW".to_vec())];
        let dir = fake_bundle(&files);
        assert!(apply_staged_update(dir.path()).unwrap());
        assert_eq!(
            std::fs::read(dir.path().join("windows/neuf.exe")).unwrap(),
            b"NEW"
        );
        assert!(
            !dir.path().join("windows/ancien.exe").exists(),
            "l'ancien payload est retire"
        );
        assert_eq!(
            std::fs::read(dir.path().join("windows.old/ancien.exe")).unwrap(),
            b"OLD"
        );
        assert!(
            !dir.path().join("state/update/staged.obup").exists(),
            "staging purge apres swap"
        );
    }

    #[test]
    fn payload_altere_refuse_et_preserve() {
        let files = vec![("windows/neuf.exe".to_string(), b"NEW".to_vec())];
        let dir = fake_bundle(&files);
        // Alteration post-import : le fichier stage ne correspond
        // plus au manifest signe.
        std::fs::write(
            dir.path().join("state/update/payload/windows/neuf.exe"),
            b"ALT",
        )
        .unwrap();
        let err = apply_staged_update(dir.path()).unwrap_err();
        assert!(err.contains("windows/neuf.exe"), "{err}");
        // Rien n'a bouge : windows/ intact, staging conserve.
        assert_eq!(
            std::fs::read(dir.path().join("windows/ancien.exe")).unwrap(),
            b"OLD"
        );
        assert!(dir.path().join("state/update/staged.obup").exists());
        assert!(!dir.path().join("windows.old").exists());
    }

    #[test]
    fn signature_invalide_refuse_le_swap() {
        let files = vec![("windows/neuf.exe".to_string(), b"NEW".to_vec())];
        let dir = fake_bundle(&files);
        let mut blob = std::fs::read(dir.path().join("state/update/staged.obup")).unwrap();
        blob[20] ^= 0xff;
        std::fs::write(dir.path().join("state/update/staged.obup"), blob).unwrap();
        assert!(apply_staged_update(dir.path()).is_err());
        assert!(dir.path().join("windows/ancien.exe").is_file());
    }

    #[test]
    fn reprise_swap_interrompu_restaure_windows() {
        // Crash mid-swap : windows/ absent + windows.old/ present.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("windows.old")).unwrap();
        std::fs::write(dir.path().join("windows.old/ancien.exe"), b"OLD").unwrap();
        recover_interrupted_swap(dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("windows/ancien.exe")).unwrap(),
            b"OLD"
        );
        assert!(!dir.path().join("windows.old").exists());
    }

    #[test]
    fn collision_fichier_racine_update_old() {
        // Le payload met a jour une copie du lanceur a la racine :
        // l'existant est retire en `.update-old` (renommable meme en
        // cours d'execution sous Windows), le nouveau installe.
        let files = vec![
            ("windows/neuf.exe".to_string(), b"NEW".to_vec()),
            ("OnionBit.exe".to_string(), b"LAUNCHER-NEW".to_vec()),
        ];
        let dir = fake_bundle(&files);
        std::fs::write(dir.path().join("OnionBit.exe"), b"LAUNCHER-OLD").unwrap();
        assert!(apply_staged_update(dir.path()).unwrap());
        assert_eq!(
            std::fs::read(dir.path().join("OnionBit.exe")).unwrap(),
            b"LAUNCHER-NEW"
        );
        assert_eq!(
            std::fs::read(dir.path().join("OnionBit.exe.update-old")).unwrap(),
            b"LAUNCHER-OLD"
        );
    }

    #[test]
    fn windows_old_preexistant_purge_avant_swap() {
        // Un reliquat de la mise a jour precedente ne doit pas faire
        // echouer le rename `windows/` -> `windows.old/`.
        let files = vec![("windows/neuf.exe".to_string(), b"NEW".to_vec())];
        let dir = fake_bundle(&files);
        std::fs::create_dir_all(dir.path().join("windows.old")).unwrap();
        std::fs::write(dir.path().join("windows.old/reliquat"), b"X").unwrap();
        assert!(apply_staged_update(dir.path()).unwrap());
        assert!(dir.path().join("windows/neuf.exe").is_file());
        assert_eq!(
            std::fs::read(dir.path().join("windows.old/ancien.exe")).unwrap(),
            b"OLD"
        );
        assert!(!dir.path().join("windows.old/reliquat").exists());
    }
}
