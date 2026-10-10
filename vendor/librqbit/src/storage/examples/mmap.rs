use std::path::Path;

use anyhow::Context;
use memmap2::{MmapMut, MmapOptions};
use parking_lot::{RwLock, RwLockReadGuard};

use crate::torrent_state::{ManagedTorrentShared, TorrentMetadata};

use crate::storage::{StorageFactory, StorageFactoryExt, TorrentStorage};

use crate::storage::filesystem::{FilesystemStorage, FilesystemStorageFactory};

#[derive(Default, Clone, Copy)]
pub struct MmapFilesystemStorageFactory {}

type OpenedMmap = RwLock<Option<MmapMut>>;

fn dummy_mmap() -> anyhow::Result<MmapMut> {
    Ok(memmap2::MmapOptions::new().len(1).map_anon()?)
}

impl StorageFactory for MmapFilesystemStorageFactory {
    type Storage = MmapFilesystemStorage;

    fn create(
        &self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<Self::Storage> {
        let fs_storage = FilesystemStorageFactory::default().create(shared, metadata)?;

        Ok(MmapFilesystemStorage {
            opened_mmaps: Vec::new(),
            lens: Vec::new(),
            fs: fs_storage,
        })
    }

    fn clone_box(&self) -> crate::storage::BoxStorageFactory {
        self.boxed()
    }
}

pub struct MmapFilesystemStorage {
    // Tribler : `None` = pas encore mappe — le mmap (et l'ouverture du
    // fichier sous-jacent) est cree a la premiere lecture/ecriture,
    // comme le file pool de libtorrent.
    opened_mmaps: Vec<OpenedMmap>,
    lens: Vec<u64>,
    fs: FilesystemStorage,
}

impl MmapFilesystemStorage {
    /// Guard sur le mmap du fichier, le creat si necessaire.
    fn mmap(&self, file_id: usize) -> anyhow::Result<impl std::ops::Deref<Target = MmapMut> + '_> {
        let cell = self.opened_mmaps.get(file_id).context("no such file")?;
        {
            let g = cell.read();
            if g.is_some() {
                return Ok(RwLockReadGuard::try_map(g, |o| o.as_ref())
                    .ok()
                    .context("bug")?);
            }
        }
        let mut g = cell.write();
        if g.is_none() {
            let len = *self.lens.get(file_id).context("no such file")?;
            // ensure_file_length ouvre le fichier paresseux et fixe sa
            // longueur avant le mapping.
            self.fs.ensure_file_length(file_id, len)?;
            let fg = self
                .fs
                .opened_files
                .get(file_id)
                .context("no such file")?
                .ensure_open()?;
            let mmap = unsafe { MmapOptions::new().map_mut(&*fg) }.context("error mapping file")?;
            *g = Some(mmap);
        }
        let g = parking_lot::RwLockWriteGuard::downgrade(g);
        Ok(RwLockReadGuard::try_map(g, |o| o.as_ref())
            .ok()
            .context("bug")?)
    }
}

impl TorrentStorage for MmapFilesystemStorage {
    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        // Lecture directe via le backend fichier : un `pread` ne cree
        // PAS le fichier ni ne le mappe — un absent reste absent (le
        // hashcheck marque la piece manquante au lieu de materialiser
        // un stub vide, ce qui corromprait la detection « fichiers
        // manquants »). Le page cache partage rend les ecritures mmap
        // visibles meme sans flush.
        self.fs.pread_exact(file_id, offset, buf)
    }

    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        let g = self.mmap(file_id)?;
        let start = offset;
        let end = offset + buf.len() as u64;
        let start = start.try_into()?;
        let end = end.try_into()?;
        // Safety: `mmap` returns a read guard; MmapMut write goes
        // through unsafe pointer cast avoided — use a write lock
        // instead by taking the cell again.
        drop(g);
        let mut g = self
            .opened_mmaps
            .get(file_id)
            .context("no such file")?
            .write();
        g.as_mut()
            .context("bug")?
            .get_mut(start..end)
            .context("bug")?
            .copy_from_slice(buf);
        Ok(())
    }

    fn remove_file(&self, file_id: usize, filename: &Path) -> anyhow::Result<()> {
        self.fs.remove_file(file_id, filename)
    }

    fn remove_directory_if_empty(&self, path: &Path) -> anyhow::Result<()> {
        self.fs.remove_directory_if_empty(path)
    }

    fn ensure_file_length(&self, file_id: usize, len: u64) -> anyhow::Result<()> {
        self.fs.ensure_file_length(file_id, len)
    }

    fn take(&self) -> anyhow::Result<Box<dyn TorrentStorage>> {
        Ok(Box::new(Self {
            opened_mmaps: self
                .opened_mmaps
                .iter()
                .map(|m| {
                    let mut g = m.write();
                    let moved = match g.as_mut() {
                        Some(mmap) => Some(std::mem::replace(mmap, dummy_mmap()?)),
                        None => None,
                    };
                    Ok::<_, anyhow::Error>(RwLock::new(moved))
                })
                .collect::<anyhow::Result<_>>()?,
            lens: self.lens.clone(),
            fs: self.fs.take_fs()?,
        }))
    }

    fn init(
        &mut self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<()> {
        // Tribler : paresseux — les fichiers restent fermes et non
        // mappes jusqu'au premier acces. L'init ne fait aucun appel
        // disque (les erreurs de chemin/permission seront reportees au
        // premier acces au lieu de l'ajout).
        self.fs.init(shared, metadata)?;
        self.lens = metadata.file_infos.iter().map(|fi| fi.len).collect();
        self.opened_mmaps = metadata
            .file_infos
            .iter()
            .map(|_| RwLock::new(None))
            .collect();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::storage::filesystem::OpenedFile;

    /// Chemin impossible a ouvrir (parent = fichier ordinaire) : la
    /// materialisation du mmap au premier acces doit echouer
    /// proprement — erreur remontee, mmap non materialise, pas de
    /// panique ni de faute d'acces.
    #[test]
    fn test_erreur_differee_au_mapping() {
        let td = TempDir::with_prefix("test_mmap_lazy_err").unwrap();
        let blocker = td.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();

        let storage = MmapFilesystemStorage {
            opened_mmaps: vec![RwLock::new(None)],
            lens: vec![16],
            fs: FilesystemStorage {
                output_folder: td.path().to_path_buf(),
                opened_files: vec![OpenedFile::new_lazy(blocker.join("f.bin"), true)],
            },
        };

        let mut buf = [0u8; 8];
        assert!(storage.pread_exact(0, 0, &mut buf).is_err());
        assert!(storage.pwrite_all(0, 0, &buf).is_err());
        // Le mmap n'a pas ete materialise apres l'echec.
        assert!(storage.opened_mmaps[0].read().is_none());
        // id de fichier hors bornes : erreur, pas de panique.
        assert!(storage.pread_exact(9, 0, &mut buf).is_err());
    }
}
