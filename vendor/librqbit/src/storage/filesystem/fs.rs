use std::{
    io::IoSlice,
    path::{Path, PathBuf},
};

use anyhow::Context;
use tracing::warn;

use crate::{
    storage::{StorageFactoryExt, filesystem::opened_file::OurFileExt},
    torrent_state::{ManagedTorrentShared, TorrentMetadata},
};

use crate::storage::{StorageFactory, TorrentStorage};

use super::opened_file::OpenedFile;

#[derive(Default, Clone, Copy)]
pub struct FilesystemStorageFactory {}

impl StorageFactory for FilesystemStorageFactory {
    type Storage = FilesystemStorage;

    fn create(
        &self,
        shared: &ManagedTorrentShared,
        _metadata: &TorrentMetadata,
    ) -> anyhow::Result<FilesystemStorage> {
        Ok(FilesystemStorage {
            output_folder: shared.options.output_folder.clone(),
            opened_files: Default::default(),
        })
    }

    fn clone_box(&self) -> crate::storage::BoxStorageFactory {
        self.boxed()
    }
}

pub struct FilesystemStorage {
    pub(crate) output_folder: PathBuf,
    pub(crate) opened_files: Vec<OpenedFile>,
}

impl FilesystemStorage {
    #[allow(dead_code)]
    pub(crate) fn take_fs(&self) -> anyhow::Result<Self> {
        Ok(Self {
            opened_files: self
                .opened_files
                .iter()
                .map(|f| f.take_clone())
                .collect::<anyhow::Result<Vec<_>>>()?,
            output_folder: self.output_folder.clone(),
        })
    }
}

impl TorrentStorage for FilesystemStorage {
    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        self.opened_files
            .get(file_id)
            .context("no such file")?
            .lock_read()?
            .pread_exact(offset, buf)
    }

    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        let of = self.opened_files.get(file_id).context("no such file")?;
        #[cfg(windows)]
        return of.try_mark_sparse()?.pwrite_all(offset, buf);
        #[cfg(not(windows))]
        return of.lock_read()?.pwrite_all(offset, buf);
    }

    fn pwrite_all_vectored(
        &self,
        file_id: usize,
        offset: u64,
        bufs: [IoSlice<'_>; 2],
    ) -> anyhow::Result<usize> {
        let of = self.opened_files.get(file_id).context("no such file")?;
        #[cfg(windows)]
        return of.try_mark_sparse()?.pwrite_all_vectored(offset, bufs);
        #[cfg(not(windows))]
        return of.lock_read()?.pwrite_all_vectored(offset, bufs);
    }

    fn remove_file(&self, _file_id: usize, filename: &Path) -> anyhow::Result<()> {
        Ok(std::fs::remove_file(self.output_folder.join(filename))?)
    }

    fn ensure_file_length(&self, file_id: usize, len: u64) -> anyhow::Result<()> {
        let f = &self.opened_files.get(file_id).context("no such file")?;
        // Tribler : paresseux — si le fichier n'est pas encore ouvert,
        // la longueur est enregistree et appliquee a l'ouverture.
        #[cfg(windows)]
        if f.is_open() {
            f.try_mark_sparse()?;
        }
        Ok(f.ensure_len(len)?)
    }

    fn take(&self) -> anyhow::Result<Box<dyn TorrentStorage>> {
        Ok(Box::new(Self {
            opened_files: self
                .opened_files
                .iter()
                .map(|f| f.take_clone())
                .collect::<anyhow::Result<Vec<_>>>()?,
            output_folder: self.output_folder.clone(),
        }))
    }

    fn remove_directory_if_empty(&self, path: &Path) -> anyhow::Result<()> {
        let path = self.output_folder.join(path);
        if !path.is_dir() {
            anyhow::bail!("cannot remove dir: {path:?} is not a directory")
        }
        if std::fs::read_dir(&path)?.count() == 0 {
            std::fs::remove_dir(&path).with_context(|| format!("error removing {path:?}"))
        } else {
            warn!("did not remove {path:?} as it was not empty");
            Ok(())
        }
    }

    fn init(
        &mut self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<()> {
        // Tribler : ouverture paresseuse — on enregistre seulement les
        // chemins attendus. Les `CreateFile`/sparse/`set_len` par
        // fichier (qui prenaient ~30 ms chacun sous Windows, soit des
        // minutes sur un torrent a milliers de fichiers) sont differes
        // a la premiere lecture/ecriture (`OpenedFile::ensure_open`).
        let files = metadata
            .file_infos
            .iter()
            .map(|file_details| {
                if file_details.attrs.padding {
                    OpenedFile::new_dummy()
                } else {
                    OpenedFile::new_lazy(
                        self.output_folder.join(&file_details.relative_filename),
                        shared.options.allow_overwrite,
                    )
                }
            })
            .collect();
        self.opened_files = files;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::storage::filesystem::OpenedFile;

    /// Un fichier enregistre en lazy dont le chemin est impossible a
    /// ouvrir (parent = fichier ordinaire) : les E/S echouent
    /// proprement au premier acces au lieu de paniquer.
    #[test]
    fn test_erreur_differee_sur_io() {
        let td = TempDir::with_prefix("test_fs_lazy_err").unwrap();
        let blocker = td.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();

        let storage = FilesystemStorage {
            output_folder: td.path().to_path_buf(),
            opened_files: vec![OpenedFile::new_lazy(blocker.join("f.bin"), true)],
        };

        // ensure_file_length reste differe (aucun acces disque).
        storage.ensure_file_length(0, 16).unwrap();
        assert!(!blocker.join("f.bin").exists());

        let mut buf = [0u8; 8];
        assert!(storage.pread_exact(0, 0, &mut buf).is_err());
        assert!(storage.pwrite_all(0, 0, &buf).is_err());
        // id de fichier hors bornes : erreur, pas de panique.
        assert!(storage.pread_exact(9, 0, &mut buf).is_err());
    }
}
