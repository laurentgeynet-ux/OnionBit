// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adaptateur `librqbit::TorrentStorage` -> `bitdaemon-disk` (port
//! libtorrent) : `FilePool` LRU + ouverture paresseuse + `PartFile`
//! pour les fichiers non selectionnes, au lieu du `FilesystemStorage`
//! historique.
//!
//! Pourquoi : le stockage de rqbit garde un handle `File` par fichier
//! pour toujours (pas de LRU), ecrit les fichiers non selectionnes
//! quand une piece est a cheval, et n'a ni cache de stat ni
//! redirection `dont_download`. `bitdaemon-disk` porte exactement ces
//! pieces depuis libtorrent (`file_pool_impl`, `part_file`,
//! `readwrite`/`pread_storage`).
//!
//! Ecart assume : `take()` partage les handles via le pool (cle
//! `(StorageIndex, FileIndex)`) — l'ancien objet n'est pas rendu inerte
//! explicitement, mais il n'est plus utilise apres la pause (le pool
//! garde la derniere instance vivante, ce qui est le comportement
//! attendu cote rqbit).

use std::io;
use std::io::IoSlice;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context as _};
use bitdaemon_common::types::{FileIndex, StorageIndex};
use bitdaemon_core::file_storage::{FileFlags, FileStorage};
use bitdaemon_disk::file::{pread_all, pwrite_all, pwritev_all};
use bitdaemon_disk::file_pool::FilePool;
use bitdaemon_disk::fs;
use bitdaemon_disk::open_mode::OpenMode;
use bitdaemon_disk::part_file::PartFile;
use librqbit::storage::{StorageFactory, StorageFactoryExt, TorrentStorage};
use librqbit::{ManagedTorrentShared, TorrentMetadata};
use librqbit_core::lengths::ValidPieceIndex;

/// Taille du pool de handles (defaut `file_pool_size` de libtorrent :
/// 40 — reglage session, pas une constante du protocole).
const FILE_POOL_SIZE: i32 = 40;

/// `StorageFactory` adossant les I/O torrent a `bitdaemon-disk`.
#[derive(Clone)]
pub struct BitdaemonStorageFactory {
    pool: Arc<FilePool>,
    next_storage: Arc<AtomicU32>,
}

impl BitdaemonStorageFactory {
    /// `pool_size` — borne LRU du `FilePool` partage entre tous les
    /// torrents (0 = defaut libtorrent 40).
    pub fn new(pool_size: i32) -> Self {
        let size = if pool_size <= 0 {
            FILE_POOL_SIZE
        } else {
            pool_size
        };
        Self {
            pool: Arc::new(FilePool::new(size)),
            next_storage: Arc::new(AtomicU32::new(0)),
        }
    }
}

impl Default for BitdaemonStorageFactory {
    fn default() -> Self {
        Self::new(0)
    }
}

impl StorageFactory for BitdaemonStorageFactory {
    type Storage = BitdaemonStorage;

    fn create(
        &self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<Self::Storage> {
        BitdaemonStorage::new(
            self.pool.clone(),
            StorageIndex(self.next_storage.fetch_add(1, Ordering::Relaxed)),
            shared,
            metadata,
        )
    }

    fn clone_box(&self) -> librqbit::storage::BoxStorageFactory {
        self.clone().boxed()
    }
}

struct Inner {
    pool: Arc<FilePool>,
    st: StorageIndex,
    save_path: PathBuf,
    /// Chemins resolus par fichier (`save_path` + chemin relatif).
    paths: Vec<PathBuf>,
    /// Taille declaree par fichier (pour `ensure_file_length` et le
    /// hint de taille a l'ouverture).
    lens: Vec<u64>,
    pad: Vec<bool>,
    /// `dont_download` && fichier absent a l'init — les ecritures sont
    /// redirigees vers le `.parts`.
    use_partfile: Vec<bool>,
    /// Longueurs demandees par `ensure_file_length` — appliquees au
    /// premier open en ecriture (paresseux, parite `OpenedFile`).
    pending_len: Mutex<Vec<u64>>,
    fs: FileStorage,
    piece_len: i32,
    num_pieces: i32,
    part_file_dir: PathBuf,
    part_file_name: PathBuf,
    part_file: Mutex<Option<PartFile>>,
}

/// Stockage rqbit implemente sur `FilePool` + `PartFile`.
pub struct BitdaemonStorage {
    inner: Option<Arc<Inner>>,
}

impl BitdaemonStorage {
    fn new(
        pool: Arc<FilePool>,
        st: StorageIndex,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<Self> {
        let save_path = shared.output_folder().to_path_buf();
        let mut fs = FileStorage::new();
        let piece_len = i32::try_from(metadata.lengths().default_piece_length())
            .context("piece_length tient dans i32")?;
        let num_pieces = i32::try_from(metadata.lengths().total_pieces())
            .context("total_pieces tient dans i32")?;
        fs.set_piece_length(piece_len);
        fs.set_num_pieces(num_pieces);

        let selected: Option<std::collections::HashSet<usize>> =
            shared.only_files().map(|v| v.iter().copied().collect());

        let mut paths = Vec::with_capacity(metadata.file_infos.len());
        let mut lens = Vec::with_capacity(metadata.file_infos.len());
        let mut pad = Vec::with_capacity(metadata.file_infos.len());
        let mut use_partfile = Vec::with_capacity(metadata.file_infos.len());

        for (i, fd) in metadata.file_infos.iter().enumerate() {
            let rel = fd.relative_filename.as_os_str().as_encoded_bytes();
            let flags = if fd.attrs.padding {
                FileFlags::PAD_FILE
            } else {
                FileFlags::from_bits(0)
            };
            fs.add_file(
                None,
                rel,
                i64::try_from(fd.len).unwrap_or(0),
                flags,
                0,
                &[],
                None,
            )
            .with_context(|| format!("add_file {rel:?}"))?;
            let p = save_path.join(&fd.relative_filename);
            paths.push(p);
            lens.push(fd.len);
            pad.push(fd.attrs.padding);
            // Regle de compatibilite libtorrent : un fichier
            // `dont_download` deja present sur disque n'est PAS
            // deplace vers le part-file (le reel prend precedence).
            let is_selected = selected.as_ref().map(|s| s.contains(&i)).unwrap_or(true);
            let absent = !fs::exists(&paths[i], fs::FileStatusFlags::DONT_FOLLOW_LINKS);
            use_partfile.push(!is_selected && absent);
        }

        // Nom du `.parts` : `.{info_hash hex}.parts` (convention
        // libtorrent, cf. `pread_storage::new`).
        let mut name = String::from(".");
        for b in shared.info_hash.0.iter() {
            name.push_str(&format!("{b:02x}"));
        }
        name.push_str(".parts");

        Ok(Self {
            inner: Some(Arc::new(Inner {
                pool,
                st,
                save_path,
                paths,
                lens,
                pad,
                use_partfile,
                pending_len: Mutex::new(vec![0; metadata.file_infos.len()]),
                fs,
                piece_len,
                num_pieces,
                part_file_dir: PathBuf::new(), // vide = sous save_path
                part_file_name: PathBuf::from(name),
                part_file: Mutex::new(None),
            })),
        })
    }

    fn inner(&self) -> anyhow::Result<&Arc<Inner>> {
        self.inner
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("storage remplace (torrent en pause)"))
    }

    /// Ouvre le fichier `i` via le pool LRU (paresseux a la premiere
    /// I/O, dedup des ouvertures concurrentes).
    fn open_file(
        inner: &Inner,
        i: usize,
        mode: OpenMode,
    ) -> Result<Arc<bitdaemon_disk::file::FileHandle>, anyhow::Error> {
        // Hint `size` = longueur effective la plus grande connue —
        // `FileHandle::open` l'applique via `set_len` a l'ouverture en
        // ecriture (longueur finale connue d'office, ou celle enregistree
        // par `ensure_file_length`).
        let size = {
            let pending = inner.pending_len.lock().unwrap_or_else(|e| e.into_inner());
            pending
                .get(i)
                .copied()
                .unwrap_or(0)
                .max(inner.lens.get(i).copied().unwrap_or(0))
        };
        let open = || {
            inner.pool.open_file_at(
                (inner.st, FileIndex(i as i32)),
                &inner.paths[i],
                i64::try_from(size).unwrap_or(0),
                mode,
            )
        };
        match open() {
            Err(e) if mode.contains(OpenMode::WRITE) && e.io.kind() == io::ErrorKind::NotFound => {
                // Ouverture paresseuse : le dossier parent est cree au
                // premier write (upstream le fait dans `initialize`,
                // rqbit OnionBit le differe pour garder `init` sans I/O).
                if let Some(parent) = inner.paths[i].parent() {
                    fs::create_directories(parent)?;
                }
                open()
            }
            r => r,
        }
        .map_err(|e| anyhow::anyhow!("open_file {}: {}", inner.paths[i].display(), e))
    }

    fn ensure_partfile(inner: &Inner) {
        let mut g = inner.part_file.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            let dir = if inner.part_file_dir.as_os_str().is_empty() {
                inner.save_path.clone()
            } else {
                inner.part_file_dir.clone()
            };
            *g = Some(PartFile::new(
                dir,
                inner.part_file_name.clone(),
                inner.num_pieces,
                inner.piece_len,
            ));
        }
    }

    /// Boucle `map_file` : un appel `pread`/`pwrite` au niveau fichier
    /// peut traverser plusieurs pieces — on decoupe par bornes de
    /// piece (`piece_len - start`) et on redirige chaque segment
    /// `dont_download` vers le `.parts`.
    fn write_partfile(
        inner: &Inner,
        file_index: usize,
        file_offset: u64,
        buf: &[u8],
    ) -> anyhow::Result<usize> {
        Self::ensure_partfile(inner);
        let g = inner.part_file.lock().unwrap_or_else(|e| e.into_inner());
        let pf = g
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("part_file non cree"))?;
        let mut done = 0usize;
        let mut off = i64::try_from(file_offset).unwrap_or(0);
        let mut remaining = i64::try_from(buf.len()).unwrap_or(0);
        while remaining > 0 {
            let req = inner
                .fs
                .map_file(FileIndex(file_index as i32), off, remaining as i32);
            if req.length <= 0 {
                break;
            }
            let in_piece = i64::from(inner.piece_len) - i64::from(req.start);
            let chunk = (i64::from(req.length).min(in_piece)) as i32;
            let n = pf
                .write(&buf[done..done + chunk as usize], req.piece, req.start)
                .map_err(|e| anyhow::anyhow!("partfile write: {e}"))?;
            done += usize::try_from(n).unwrap_or(0);
            off += i64::from(n);
            remaining -= i64::from(n);
        }
        Ok(done)
    }

    fn read_partfile(
        inner: &Inner,
        file_index: usize,
        file_offset: u64,
        buf: &mut [u8],
    ) -> anyhow::Result<usize> {
        let g = inner.part_file.lock().unwrap_or_else(|e| e.into_inner());
        let pf = g
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("part_file non cree"))?;
        let mut done = 0usize;
        let mut off = i64::try_from(file_offset).unwrap_or(0);
        let mut remaining = i64::try_from(buf.len()).unwrap_or(0);
        while remaining > 0 {
            let req = inner
                .fs
                .map_file(FileIndex(file_index as i32), off, remaining as i32);
            if req.length <= 0 {
                break;
            }
            let in_piece = i64::from(inner.piece_len) - i64::from(req.start);
            let chunk = (i64::from(req.length).min(in_piece)) as i32;
            let n = pf
                .read(&mut buf[done..done + chunk as usize], req.piece, req.start)
                .map_err(|e| anyhow::anyhow!("partfile read: {e}"))?;
            done += usize::try_from(n).unwrap_or(0);
            off += i64::from(n);
            remaining -= i64::from(n);
        }
        Ok(done)
    }
}

impl TorrentStorage for BitdaemonStorage {
    fn init(
        &mut self,
        _shared: &ManagedTorrentShared,
        _metadata: &TorrentMetadata,
    ) -> anyhow::Result<()> {
        // Entierement paresseux (parite `FilesystemStorage::init`
        // OnionBit) : aucun I/O ici — une sortie inaccessible doit
        // rester une erreur differee au premier acces, pas un echec
        // de restauration. Les dossiers parents sont crees a la
        // premiere ecriture (`open_file` retente sur NotFound).
        Ok(())
    }

    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        let inner = self.inner()?;
        if inner.pad.get(file_id).copied().unwrap_or(false) {
            buf.fill(0);
            return Ok(());
        }
        if inner.use_partfile.get(file_id).copied().unwrap_or(false) {
            let n = Self::read_partfile(inner, file_id, offset, buf)?;
            if n != buf.len() {
                bail!("partfile read court {}/{}", n, buf.len());
            }
            return Ok(());
        }
        let h = Self::open_file(inner, file_id, OpenMode::READ_ONLY)?;
        let n = pread_all(h.file(), buf, offset).map_err(|e| {
            anyhow::anyhow!(
                "pread {} @{}: {}",
                inner.paths[file_id].display(),
                offset,
                e
            )
        })?;
        if n != buf.len() {
            bail!("pread court {}/{}", n, buf.len());
        }
        Ok(())
    }

    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        let inner = self.inner()?;
        if inner.pad.get(file_id).copied().unwrap_or(false) {
            return Ok(()); // ecriture vers un pad : jetee
        }
        if inner.use_partfile.get(file_id).copied().unwrap_or(false) {
            let n = Self::write_partfile(inner, file_id, offset, buf)?;
            if n != buf.len() {
                bail!("partfile write court {}/{}", n, buf.len());
            }
            return Ok(());
        }
        let h = Self::open_file(inner, file_id, OpenMode::WRITE)?;
        // Marquage sparse au premier open en ecriture (Windows) — le
        // `set_len` de `FileHandle::open` n'est pas sparse de lui-meme.
        #[cfg(windows)]
        {
            use librqbit::storage::filesystem::mark_file_sparse;
            let _ = mark_file_sparse(h.file());
        }
        let n = pwrite_all(h.file(), buf, offset).map_err(|e| {
            anyhow::anyhow!(
                "pwrite {} @{}: {}",
                inner.paths[file_id].display(),
                offset,
                e
            )
        })?;
        if n != buf.len() {
            bail!("pwrite court {}/{}", n, buf.len());
        }
        Ok(())
    }

    fn pwrite_all_vectored(
        &self,
        file_id: usize,
        offset: u64,
        bufs: [IoSlice<'_>; 2],
    ) -> anyhow::Result<usize> {
        let inner = self.inner()?;
        if inner.pad.get(file_id).copied().unwrap_or(false) {
            return Ok(bufs.iter().map(|b| b.len()).sum());
        }
        if inner.use_partfile.get(file_id).copied().unwrap_or(false) {
            // flatten pour conserver la semantique piece-par-piece du
            // part-file (les buffers sont contigus dans le meme fichier)
            let flat: Vec<u8> = bufs.iter().flat_map(|b| b.iter().copied()).collect();
            return Self::write_partfile(inner, file_id, offset, &flat);
        }
        let h = Self::open_file(inner, file_id, OpenMode::WRITE)?;
        #[cfg(windows)]
        {
            use librqbit::storage::filesystem::mark_file_sparse;
            let _ = mark_file_sparse(h.file());
        }
        let refs: [&[u8]; 2] = [&bufs[0], &bufs[1]];
        pwritev_all(h.file(), &refs, offset).map_err(|e| {
            anyhow::anyhow!(
                "pwritev {} @{}: {}",
                inner.paths[file_id].display(),
                offset,
                e
            )
        })
    }

    fn ensure_file_length(&self, file_id: usize, length: u64) -> anyhow::Result<()> {
        let inner = self.inner()?;
        // Paresseux : longueur enregistree, appliquee au premier open
        // en ecriture via le hint `size` de `FileHandle::open` (sparse
        // marque a ce moment-la). Aucun I/O disque ici.
        let mut g = inner.pending_len.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = g.get_mut(file_id) {
            *slot = (*slot).max(length);
        }
        Ok(())
    }

    fn remove_file(&self, file_id: usize, _filename: &Path) -> anyhow::Result<()> {
        let inner = self.inner()?;
        // Fermer le handle avant la suppression (Windows refuse de
        // supprimer un fichier ouvert).
        inner.pool.release_one(inner.st, FileIndex(file_id as i32));
        match fs::remove(&inner.paths[file_id]) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r.map_err(|e| anyhow::anyhow!("remove {}: {}", inner.paths[file_id].display(), e)),
        }
    }

    fn remove_directory_if_empty(&self, path: &Path) -> anyhow::Result<()> {
        if std::fs::read_dir(path)?.count() == 0 {
            std::fs::remove_dir(path).with_context(|| format!("error removing {path:?}"))
        } else {
            tracing::warn!("did not remove {path:?} as it was not empty");
            Ok(())
        }
    }

    fn take(&self) -> anyhow::Result<Box<dyn TorrentStorage>> {
        // Le pool partage les handles par `(st, file_index)` — la
        // nouvelle instance reutilise les memes handles ; l'ancienne
        // n'est plus appelee (comportement upstream `take_clone`).
        Ok(Box::new(Self {
            inner: self.inner.clone(),
        }))
    }

    fn on_piece_completed(&self, _piece_index: ValidPieceIndex) -> anyhow::Result<()> {
        if let Some(inner) = self.inner.as_ref() {
            inner.tick();
        }
        Ok(())
    }
}

impl Inner {
    /// Flush des metadatas du part-file (dirty → header reecrit).
    fn tick(&self) {
        let g = self.part_file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pf) = g.as_ref() {
            let _ = pf.flush_metadata();
        }
    }
}
