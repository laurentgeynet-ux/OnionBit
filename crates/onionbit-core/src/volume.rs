// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Détection « média amovible / sans ACL persistantes » pour le
//! bandeau `identity.at_rest` (ADR-0018 étape 63, décision de revue :
//! proposition non bloquante — jamais un verrou).
//!
//! `media_removable` retourne `true` quand la racine vit sur un volume
//! probablement amovible (clé USB, carte SD, disque externe) ou sur un
//! système de fichiers sans ACL (FAT32/exFAT/NTFS-3G, sdcardfs) : la
//! graine `identity_seed.bin` y est lisible par tout hôte qui monte le
//! média. La détection est best-effort et conservatrice — un doute
//! retourne `false` plutôt qu'un faux positif systématique.

use std::path::Path;

/// `true` si `path` réside sur un volume amovible ou sans ACL
/// persistantes. `path` n'a pas besoin d'exister : la détection
/// retombe sur le premier ancêtre existant.
#[cfg(windows)]
pub fn media_removable(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetVolumeInformationW, GetVolumePathNameW,
    };
    use windows_sys::Win32::System::SystemServices::FILE_PERSISTENT_ACLS;
    use windows_sys::Win32::System::WindowsProgramming::{DRIVE_CDROM, DRIVE_REMOVABLE};

    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let wide: Vec<u16> = canon
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // Racine du volume (« E:\ ») — `GetVolumePathNameW` gère les
    // junctions/points de montage mieux que le préfixe du chemin.
    let mut root = [0u16; 1024];
    unsafe {
        if GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) == 0 {
            return false;
        }
        match GetDriveTypeW(root.as_ptr()) {
            DRIVE_REMOVABLE | DRIVE_CDROM => return true,
            _ => {}
        }
        // `FILE_PERSISTENT_ACLS` absent → FAT32/exFAT : aucune ACL sur
        // les fichiers, la graine en clair est lisible partout.
        let mut flags: u32 = 0;
        let ok = GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            std::ptr::null_mut(),
            0,
        );
        ok != 0 && (flags & FILE_PERSISTENT_ACLS) == 0
    }
}

/// Linux/Android : le fstype du point de montage le plus englobant
/// dans `/proc/self/mounts` tranche — les fs sans ACL persistants
/// (FAT/exFAT/NTFS-3G/sdcardfs) et les points de montage typés
/// amovibles (`/media`, `/run/media`, `/mnt/*`, `/storage/*` non
/// émulé) basculent le drapeau.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn media_removable(path: &Path) -> bool {
    let Ok(canon) = path.canonicalize() else {
        // Chemin inexistant : impossible de résoudre le volume.
        return false;
    };
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else {
        return false;
    };
    let mut best: Option<(usize, String, String)> = None;
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let _source = it.next();
        let (Some(mnt), Some(fstype)) = (it.next(), it.next()) else {
            continue;
        };
        // Les espaces des points de montage sont échappés `\040`.
        let mnt = mnt.replace("\\040", " ");
        if !canon.starts_with(&mnt) {
            continue;
        }
        if best.as_ref().is_none_or(|(len, _, _)| mnt.len() > *len) {
            best = Some((mnt.len(), mnt, fstype.to_string()));
        }
    }
    let Some((_, mnt, fstype)) = best else {
        return false;
    };
    const NO_ACL_FS: &[&str] = &[
        "vfat",
        "msdos",
        "exfat",
        "ntfs",
        "ntfs3",
        "fuseblk",
        "sdcardfs",
        "exfat-fuse",
    ];
    // `fuse.xxx` couvre ntfs-3g / exfat-fuse / mount.exfat selon le
    // helper ; le `fuse` générique seul n'est pas flaggé (trop large).
    if NO_ACL_FS.contains(&fstype.as_str()) || fstype.starts_with("fuse.") {
        return true;
    }
    // Points de montage par convention réservés aux médias amovibles.
    // `/storage/` = Android (les cartes externes y vivent en
    // `XXXX-XXXX` ; `/storage/emulated` est interne mais déjà couvert
    // par son fstype sdcardfs/fuse quand il est sans ACL).
    for prefix in ["/media/", "/run/media/", "/mnt/", "/storage/"] {
        if mnt.starts_with(prefix) && mnt != prefix.trim_end_matches('/') {
            return true;
        }
    }
    false
}

/// macOS : `statfs(2)` donne le nom du système de fichiers —
/// `msdos`/`exfat`/`ntfs` n'ont pas d'ACL ; tout volume monté sous
/// `/Volumes/` autre que la racine système est externe par
/// convention.
#[cfg(target_os = "macos")]
pub fn media_removable(path: &Path) -> bool {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let Ok(c_path) = CString::new(canon.as_os_str().as_bytes()) else {
        return false;
    };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
        return false;
    }
    let fstype = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) }.to_string_lossy();
    if matches!(fstype.as_ref(), "msdos" | "exfat" | "ntfs") {
        return true;
    }
    let mnt = unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) }.to_string_lossy();
    mnt.starts_with("/Volumes/")
}

/// Autres unix : pas de table de montage portable fiable — repli
/// conservateur `false` (le bandeau est une proposition, jamais un
/// verrou).
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
pub fn media_removable(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La fonction ne panique jamais, même sur un chemin arbitraire
    /// ou inexistant — le bandeau UI est non bloquant.
    #[test]
    fn media_removable_ne_panique_pas() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _ = media_removable(tmp.path());
        let _ = media_removable(Path::new("/nonexistent/onionbit/path"));
    }
}
