// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Zone privee sur le backend **bitdaemon** — implementation
//! `DiskStorage` piece-alignee au-dessus du meme format `OBD` que
//! [`crate::storage_private`] (ADR-0018).
//!
//! BitDaemon injecte un `StorageFactory` **par session** (couture
//! ADR-0008 upstream) : la fabrique ci-dessous dispatche par
//! `params.info_hash` — torrent inscrit dans le registre prive →
//! [`BdPrivateStorage`] (`.obd` chiffres + noms HMAC), sinon
//! `PreadStorage` standard. L'inscription se fait dans
//! `BtEngine::add` quand `AddDownloadOptions.storage_factory` porte
//! une spec privee (cles + racine de zone derivees de l'identite).
//!
//! **Aucun cleartext prive sur disque** — y compris le part-file des
//! zones `dont_download`, remplace par un `.obd` dedie (`__parts`).
//!
//! Limites assumees de cette tranche :
//! - `verify_resume_data` renvoie `false` → recheck complet (le hash
//!   relit le clair dechiffre — correct mais plus lent qu'un resume).
//! - `move_storage` refuse (non expose a la zone privee aujourd'hui).
//! - `rename_file` est un no-op : les noms sur disque sont opaques.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bitdaemon_common::bitfield::Bitfield;
use bitdaemon_common::types::StorageIndex;
use bitdaemon_common::types::{DownloadPriority, FileIndex, PieceIndex};
use bitdaemon_core::file_storage::FileStorage;
use bitdaemon_disk::open_mode::OpenMode;
use bitdaemon_disk::storage::{
    DiskStorage, MoveFlags, PreadStorage, StorageFactory, StorageParams,
};
use bitdaemon_disk::storage_error::{Operation, StorageError, StorageResult};
use librqbit::storage::TorrentStorage as _;
use onionbit_crypto::obdfile::PrivateStoreKeys;

use crate::storage_private::{PrivateFile, PrivateStorage};

/// Spec d'une zone privee (cles d'identite + racine + granularite).
/// `Clone` : le registre partage la meme instance que la fabrique.
#[derive(Clone)]
pub(crate) struct BdPrivateSpec {
    pub keys: Arc<PrivateStoreKeys>,
    /// Racine de la zone (`<data>/private/…`) — le sous-dossier de
    /// groupe `<hmac>` y est cree par torrent.
    pub root: PathBuf,
    pub chunk_log2: u8,
}

/// Registre `infohash → spec privee` alimente a `BtEngine::add`
/// (avant `add_torrent_params`, qui cree le storage via la factory).
pub(crate) type PrivateRegistry = Arc<Mutex<HashMap<[u8; 20], BdPrivateSpec>>>;

/// `StorageFactory` de session : dispatch prive/public par infohash.
/// La cle n'apparait jamais dans le registre — seulement la spec
/// (la `PrivateStorageFactory` rqbit reste le porteur cote session).
pub(crate) fn dispatch_factory(registry: PrivateRegistry) -> StorageFactory {
    Arc::new(move |params: &StorageParams<'_>, pool| {
        let spec = registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&params.info_hash)
            .cloned();
        match spec {
            Some(spec) => Box::new(BdPrivateStorage::new(params, spec)) as Box<dyn DiskStorage>,
            None => Box::new(PreadStorage::new(params.clone(), pool)) as Box<dyn DiskStorage>,
        }
    })
}

/// Retire le composant racine `<name>/` quand le layout du torrent
/// l'inclut (multi-fichiers avec racine) — parite du domaine cle
/// `K_file` avec le chemin rqbit.
fn strip_root(rel: Vec<u8>, name: &[u8]) -> Vec<u8> {
    if !name.is_empty() && rel.len() > name.len() {
        let sep = rel[name.len()];
        if rel.starts_with(name) && (sep == b'/' || sep == b'\\') {
            return rel[name.len() + 1..].to_vec();
        }
    }
    rel
}

fn io_err(e: anyhow::Error) -> io::Error {
    io::Error::other(e.to_string())
}

fn store_err(e: anyhow::Error, file: FileIndex, op: Operation) -> StorageError {
    StorageError::new(io_err(e), file, op)
}

/// `DiskStorage` prive — reutilise le moteur `OBD` de
/// `storage_private` (verrous rayes, ouverture paresseuse).
pub(crate) struct BdPrivateStorage {
    inner: PrivateStorage,
    files: FileStorage,
    /// Priorites courantes (routing part-file comme `PreadStorage`) —
    /// mutees via `set_file_priority(&mut self)` sous le `RwLock`
    /// disque, lues sans verrou comme upstream `m_file_priority`.
    prios: Vec<DownloadPriority>,
    /// Part-file chiffre pour les segments `dont_download`
    /// (`__parts` — domaine cle dedie, jamais de cleartext).
    part: Arc<PrivateFile>,
    storage_index: Mutex<StorageIndex>,
}

impl BdPrivateStorage {
    fn new(params: &StorageParams<'_>, spec: BdPrivateSpec) -> Self {
        let ih = params.info_hash;
        let group_dir = spec.root.join(spec.keys.group_name(&ih));
        // Ouverture paresseuse (parite rqbit `init`) : enregistre
        // chemins/longueurs, cree le dossier de groupe ; le premier
        // `.obd` reel est materialise pour le sceau `scan_ct`
        // (reconstruction du groupe apres double perte du manifeste).
        let _ = std::fs::create_dir_all(&group_dir);
        let files: Vec<Arc<PrivateFile>> = (0..params.files.num_files())
            .map(|i| {
                let idx = FileIndex(i);
                if params.files.pad_file_at(idx) {
                    Arc::new(PrivateFile::padding())
                } else {
                    // `file_path` inclut le dossier racine du torrent
                    // (`<name>/f`) ; la convention cle `K_file` d'OBD
                    // (parite rqbit `full_path`) stocke le chemin
                    // relatif **sans** ce composant.
                    let rel = strip_root(params.files.file_path(idx, b""), params.files.name());
                    Arc::new(PrivateFile::new(
                        group_dir.join(spec.keys.file_name(&ih, &rel)),
                        rel,
                        params.files.file_size(idx).max(0) as u64,
                    ))
                }
            })
            .collect();
        let inner = PrivateStorage::new_raw(
            spec.keys.clone(),
            ih,
            spec.chunk_log2,
            group_dir.clone(),
            files,
        );
        if let Some(f) = inner.files.iter().find(|f| !f.padding) {
            let _ = f.with_obd(&inner, true, |_| Ok(()));
        }
        let part = Arc::new(PrivateFile::new(
            group_dir.join(spec.keys.file_name(&ih, b"__parts")),
            b"__parts".to_vec(),
            0,
        ));
        Self {
            inner,
            files: params.files.clone(),
            prios: params.priorities.to_vec(),
            part,
            storage_index: Mutex::new(StorageIndex(0)),
        }
    }

    /// `true` si `idx` est `dont_download` → segments routes vers le
    /// part-file chiffre (upstream `use_partfile` defaut vrai).
    fn dont_download(&self, idx: FileIndex) -> bool {
        self.prios
            .get(idx.get())
            .is_some_and(|p| *p == DownloadPriority::DONT_DOWNLOAD)
    }

    /// Offset du segment dans le part-file (`piece*len + start`,
    /// meme mapping que `PartFile` upstream).
    fn part_offset(&self, file: FileIndex, file_offset: i64) -> u64 {
        let map = self.files.map_file(file, file_offset, 0);
        (i64::from(map.piece.0) * i64::from(self.files.piece_length()) + i64::from(map.start))
            .max(0) as u64
    }

    fn read_segment(
        &self,
        idx: FileIndex,
        file_offset: i64,
        buf: &mut [u8],
    ) -> Result<usize, StorageError> {
        if self.files.pad_file_at(idx) {
            buf.fill(0);
            return Ok(buf.len());
        }
        if self.dont_download(idx) {
            let off = self.part_offset(idx, file_offset);
            return match self.part.with_obd(&self.inner, false, |o| {
                o.read_range(off, buf).map_err(|e| anyhow::anyhow!(e))
            }) {
                Ok(()) => Ok(buf.len()),
                // Part-file absent ou plage jamais ecrite → zeros
                // (parite `PartFile` qui lit du creux).
                Err(_) => {
                    buf.fill(0);
                    Ok(buf.len())
                }
            };
        }
        self.inner
            .pread_exact(idx.get(), file_offset.max(0) as u64, buf)
            .map(|_| buf.len())
            .map_err(|e| store_err(e, idx, Operation::FileRead))
    }

    fn write_segment(
        &self,
        idx: FileIndex,
        file_offset: i64,
        buf: &[u8],
    ) -> Result<usize, StorageError> {
        if self.files.pad_file_at(idx) {
            return Ok(buf.len());
        }
        if self.dont_download(idx) {
            let off = self.part_offset(idx, file_offset);
            return self
                .part
                .with_obd(&self.inner, true, |o| {
                    // Le part-file grandit a la demande (upstream
                    // `PartFile` sparse) : `write_range` borne a
                    // `plain_len`, qu'on etend au besoin.
                    let need = off + buf.len() as u64;
                    if o.plain_len() < need {
                        o.set_len(need).map_err(|e| anyhow::anyhow!(e))?;
                    }
                    o.write_range(off, buf).map_err(|e| anyhow::anyhow!(e))
                })
                .map(|_| buf.len())
                .map_err(|e| store_err(e, idx, Operation::PartfileWrite));
        }
        self.inner
            .pwrite_all(idx.get(), file_offset.max(0) as u64, buf)
            .map(|_| buf.len())
            .map_err(|e| store_err(e, idx, Operation::FileWrite))
    }
}

impl DiskStorage for BdPrivateStorage {
    fn set_storage_index(&self, idx: StorageIndex) {
        *self.storage_index.lock().unwrap_or_else(|e| e.into_inner()) = idx;
    }

    fn files(&self) -> &FileStorage {
        &self.files
    }

    fn file_priorities(&self) -> &[DownloadPriority] {
        &self.prios
    }

    fn read(
        &self,
        buf: &mut [u8],
        piece: PieceIndex,
        offset: i64,
        _mode: OpenMode,
        _volatile: bool,
    ) -> StorageResult<i64> {
        bitdaemon_disk::readwrite::readwrite_mut(&self.files, buf, piece, offset, |i, o, b| {
            self.read_segment(i, o, b)
        })
    }

    fn write(
        &self,
        buf: &[u8],
        piece: PieceIndex,
        offset: i64,
        _mode: OpenMode,
    ) -> StorageResult<i64> {
        bitdaemon_disk::readwrite::readwrite_buf(&self.files, buf, piece, offset, |i, o, b| {
            self.write_segment(i, o, b)
        })
    }

    fn hash_with(
        &self,
        feed: &mut dyn FnMut(&[u8]),
        len: i64,
        piece: PieceIndex,
        offset: i64,
        _mode: OpenMode,
        _volatile: bool,
    ) -> StorageResult<i64> {
        let len_usize = usize::try_from(len).unwrap_or(0);
        let phantom = vec![0u8; len_usize];
        let mut scratch = Vec::new();
        bitdaemon_disk::readwrite::readwrite_buf(&self.files, &phantom, piece, offset, |i, o, b| {
            scratch.clear();
            scratch.resize(b.len(), 0);
            self.read_segment(i, o, &mut scratch)?;
            feed(&scratch);
            Ok(b.len())
        })
    }

    fn release_files(&self) -> StorageResult<()> {
        self.inner.release();
        self.part.close();
        Ok(())
    }

    fn move_storage(&self, _path: PathBuf, _flags: MoveFlags) -> StorageResult<(u8, PathBuf)> {
        Err(StorageError::new(
            io::Error::new(
                io::ErrorKind::Unsupported,
                "move_storage non supporte (zone privee)",
            ),
            FileIndex(-1),
            Operation::FileRename,
        ))
    }

    fn delete_files(&self, _options: u8) -> StorageResult<()> {
        for (i, f) in self.inner.files.iter().enumerate() {
            if f.padding {
                continue;
            }
            let rel = strip_root(
                self.files.file_path(FileIndex(i as i32), b""),
                self.files.name(),
            );
            let rel_str = String::from_utf8_lossy(&rel).into_owned();
            let _ = self.inner.remove_file(i, std::path::Path::new(&rel_str));
        }
        // Part-file chiffre : meme mecanique de retrait que les
        // fichiers — un acces ulterieur echouerait sinon.
        self.part.remove();
        let _ = self
            .inner
            .remove_directory_if_empty(std::path::Path::new(""));
        Ok(())
    }

    fn rename_file(&mut self, _index: FileIndex, _new_name: &[u8]) -> StorageResult<()> {
        // Noms opaques HMAC : le rename n'a pas d'effet sur disque.
        Ok(())
    }

    fn set_file_priority(&mut self, prio: &[DownloadPriority]) -> StorageResult<()> {
        self.prios
            .resize(prio.len().max(self.prios.len()), DownloadPriority::DEFAULT);
        for (i, p) in prio.iter().enumerate() {
            self.prios[i] = *p;
        }
        Ok(())
    }

    fn verify_resume_data(
        &self,
        _have_pieces: &Bitfield,
        _seed_mode: bool,
        _links: &[PathBuf],
    ) -> StorageResult<bool> {
        // Pas de fastresume prive pour l'instant → recheck complet :
        // `check_pieces` re-hache via `hash_with` (clair dechiffre).
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitdaemon_core::file_storage::FileFlags;
    use bitdaemon_core::file_storage::RenamedFiles;
    use bitdaemon_disk::storage::StorageMode;

    fn make_fs() -> FileStorage {
        let mut fs = FileStorage::new();
        fs.set_name(b"t");
        fs.set_piece_length(1024);
        // piece 0 = a.bin[0..1000] + b.bin[0..24] ; piece 1 = b.bin[24..1000].
        fs.add_file(None, b"t/a.bin", 1000, FileFlags::NONE, 0, b"", None)
            .unwrap();
        fs.add_file(None, b"t/b.bin", 1000, FileFlags::NONE, 0, b"", None)
            .unwrap();
        fs.set_num_pieces(2);
        fs
    }

    fn params<'a>(
        fs: &'a FileStorage,
        renamed: &'a RenamedFiles,
        ih: [u8; 20],
        prios: &'a [DownloadPriority],
    ) -> StorageParams<'a> {
        StorageParams {
            files: fs,
            renamed,
            path: PathBuf::from("inutile-en-prive"),
            part_file_dir: PathBuf::new(),
            mode: StorageMode::Sparse,
            priorities: prios,
            info_hash: ih,
            v1: true,
            v2: false,
        }
    }

    fn storage_privee(dir: &tempfile::TempDir) -> (Box<dyn DiskStorage>, PathBuf) {
        let ih = [7u8; 20];
        let reg: PrivateRegistry = Default::default();
        reg.lock().unwrap().insert(
            ih,
            BdPrivateSpec {
                keys: Arc::new(PrivateStoreKeys::from_root(&[9u8; 32])),
                root: dir.path().to_path_buf(),
                chunk_log2: 14,
            },
        );
        let fs = make_fs();
        // La fabrique emprunte `params` — on construit le storage via
        // `BdPrivateStorage::new` directement (meme chemin que le
        // dispatch, sans l'emprunt de `params` dans le test).
        let spec = reg.lock().unwrap().get(&ih).cloned().unwrap();
        let st = BdPrivateStorage::new(
            &params(
                &fs,
                &RenamedFiles::default(),
                ih,
                &[DownloadPriority::DEFAULT; 2],
            ),
            spec,
        );
        (Box::new(st), dir.path().to_path_buf())
    }

    /// Collecte recursive des octets bruts sur disque (verifie
    /// l'absence de cleartext).
    fn disk_bytes(root: &std::path::Path) -> Vec<u8> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(root).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.extend(disk_bytes(&p));
            } else {
                out.extend(std::fs::read(&p).unwrap());
            }
        }
        out
    }

    #[test]
    fn ecrit_lit_chiffre_sur_disque() {
        let dir = tempfile::tempdir().unwrap();
        let (st, root) = storage_privee(&dir);
        let data: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
        st.write(&data, PieceIndex(0), 0, OpenMode::WRITE).unwrap();
        let mut back = vec![0u8; 1024];
        st.read(&mut back, PieceIndex(0), 0, OpenMode::READ_ONLY, false)
            .unwrap();
        assert_eq!(back, data);
        // Rien en clair sur disque : un bloc de 128 octets du motif
        // ne peut pas apparaitre dans du ciphertext ChaCha20.
        let raw = disk_bytes(&root);
        assert!(!raw.is_empty(), "des .obd doivent etre ecrits");
        assert!(
            !raw.windows(128).any(|w| w == &data[..128]),
            "cleartext detecte dans la zone privee"
        );
        // Noms opaques : pas de `a.bin`/`b.bin` sur disque.
        let names: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flat_map(|e| {
                let d = e.unwrap();
                vec![d
                    .path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()]
            })
            .collect();
        assert!(names.iter().all(|n| !n.contains("a.bin")));
    }

    #[test]
    fn hash_releit_le_clair() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _root) = storage_privee(&dir);
        let data: Vec<u8> = (0..1024).map(|i| (i % 197) as u8).collect();
        st.write(&data, PieceIndex(0), 0, OpenMode::WRITE).unwrap();
        let mut fed = Vec::new();
        st.hash_with(
            &mut |d| fed.extend_from_slice(d),
            1024,
            PieceIndex(0),
            0,
            OpenMode::READ_ONLY,
            false,
        )
        .unwrap();
        assert_eq!(fed, data, "le hash doit voir le clair dechiffre");
    }

    #[test]
    fn dont_download_passe_par_partfile_chiffre() {
        let dir = tempfile::tempdir().unwrap();
        let ih = [7u8; 20];
        let spec = BdPrivateSpec {
            keys: Arc::new(PrivateStoreKeys::from_root(&[9u8; 32])),
            root: dir.path().to_path_buf(),
            chunk_log2: 14,
        };
        let fs = make_fs();
        let mut st = BdPrivateStorage::new(
            &params(
                &fs,
                &RenamedFiles::default(),
                ih,
                &[DownloadPriority::DEFAULT; 2],
            ),
            spec,
        );
        st.set_file_priority(&[DownloadPriority::DEFAULT, DownloadPriority::DONT_DOWNLOAD])
            .unwrap();
        // La piece 0 chevauche a.bin (voulu) + debut de b.bin (exclu).
        let data: Vec<u8> = (0..1024).map(|i| (i % 149) as u8).collect();
        st.write(&data, PieceIndex(0), 0, OpenMode::WRITE).unwrap();
        let mut back = vec![0u8; 1024];
        st.read(&mut back, PieceIndex(0), 0, OpenMode::READ_ONLY, false)
            .unwrap();
        assert_eq!(back, data, "le part-file doit servir les segments exclus");
        // Aucun cleartext (part-file inclus) sur disque : les 128
        // derniers octets (segment exclus en part-file) n'apparaissent
        // pas en clair.
        let raw = disk_bytes(dir.path());
        assert!(!raw.windows(128).any(|w| w == &data[896..1024]));
    }

    #[test]
    fn delete_purge_le_groupe() {
        let dir = tempfile::tempdir().unwrap();
        let (st, root) = storage_privee(&dir);
        st.write(&[1u8; 64], PieceIndex(0), 0, OpenMode::WRITE)
            .unwrap();
        assert!(!disk_bytes(&root).is_empty());
        st.delete_files(0).unwrap();
        assert!(disk_bytes(&root).is_empty(), "le groupe doit etre vide");
    }
}
