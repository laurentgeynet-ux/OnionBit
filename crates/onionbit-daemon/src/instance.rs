// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mutex d'instance unique : un seul `onionbit-daemon` par répertoire
//! d'état (deux lancements d'un même `state_dir` = un seul systray et
//! un seul bind API). Le nom du mutex incorpore un hash du chemin
//! absolu pour ne pas bloquer des daemons de `state_dir` distincts
//! (tests e2e en `tempdir`, instances parallèles).

#[cfg(any(windows, test))]
use std::collections::hash_map::DefaultHasher;
#[cfg(any(windows, test))]
use std::hash::{Hash, Hasher};
use std::path::Path;

/// Handle Win32 du mutex nommé (Windows) ou fichier `.onionbit.lock`
/// verrouillé (POSIX) — libéré à la fin du processus.
pub struct InstanceGuard {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(not(windows))]
    _file: Option<std::fs::File>,
}

/// `None` = une instance est déjà en cours pour ce `state_dir`.
#[cfg(windows)]
pub fn acquire(state_dir: &Path) -> Option<InstanceGuard> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let mut name: Vec<u16> = format!("Local\\TriblerRustDaemon-{}\0", state_hash(state_dir))
        .encode_utf16()
        .collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_mut_ptr());
        if handle.is_null() {
            // Le mutex n'a pas pu être créé : ne pas bloquer le daemon.
            return Some(InstanceGuard { handle });
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return None;
        }
        Some(InstanceGuard { handle })
    }
}

/// POSIX : verrou exclusif `flock` sur `state_dir/.onionbit.lock`
/// (fs2 — libéré automatiquement à la fermeture du descripteur, y
/// compris en cas de crash). `try_lock_exclusive` échoue quand une
/// autre instance tient déjà le verrou. Fichier inouvrable =
/// contournement avec avertissement (fail-open comme Windows).
#[cfg(not(windows))]
pub fn acquire(state_dir: &Path) -> Option<InstanceGuard> {
    use fs2::FileExt;
    let path = state_dir.join(".onionbit.lock");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
    else {
        tracing::warn!(?path, "fichier de verrou d'instance inouvrable");
        return Some(InstanceGuard { _file: None });
    };
    match file.try_lock_exclusive() {
        Ok(()) => Some(InstanceGuard { _file: Some(file) }),
        Err(_) => None, // verrou tenu par une instance active
    }
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
        }
    }
}

/// Hash stable du chemin absolu de `state_dir` (minuscules — Windows
/// est insensible à la casse sur les chemins).
#[cfg(any(windows, test))]
fn state_hash(state_dir: &Path) -> u64 {
    let abs = std::path::absolute(state_dir).unwrap_or_else(|_| state_dir.to_path_buf());
    let mut h = DefaultHasher::new();
    abs.to_string_lossy().to_lowercase().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_hash_est_stable_et_distingue_deux_chemins() {
        let dir = std::path::PathBuf::from("state-a");
        let other = std::path::PathBuf::from("state-b");
        assert_eq!(state_hash(&dir), state_hash(&dir));
        assert_ne!(state_hash(&dir), state_hash(&other));
    }

    // Mutex Win32 sous Windows, verrou `flock` fs2 ailleurs — dans
    // les deux cas le second `acquire` sur le meme state_dir refuse.
    #[test]
    fn le_second_lock_sur_le_meme_state_dir_echoue() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path());
        assert!(first.is_some(), "premier lock attendu");
        assert!(
            acquire(dir.path()).is_none(),
            "le même state_dir doit refuser une seconde instance"
        );
        drop(first);
        assert!(
            acquire(dir.path()).is_some(),
            "le mutex doit être libéré avec le guard"
        );
    }
}
