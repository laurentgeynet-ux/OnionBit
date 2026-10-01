use std::{
    fs::File,
    io::IoSlice,
    ops::{Deref, DerefMut},
    path::PathBuf,
};

use anyhow::Context;
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::Error;

pub trait OurFileExt {
    fn pwrite_all_vectored(&self, offset: u64, bufs: [IoSlice<'_>; 2]) -> anyhow::Result<usize>;
    fn pread_exact(&self, offset: u64, buf: &mut [u8]) -> anyhow::Result<()>;
    fn pwrite_all(&self, offset: u64, buf: &[u8]) -> anyhow::Result<()>;
}

impl OurFileExt for File {
    #[cfg(unix)]
    fn pwrite_all_vectored(&self, offset: u64, bufs: [IoSlice<'_>; 2]) -> anyhow::Result<usize> {
        nix::sys::uio::pwritev(self, &bufs, offset.try_into()?).context("error calling pwritev")
    }

    #[cfg(not(unix))]
    fn pwrite_all_vectored(&self, offset: u64, bufs: [IoSlice<'_>; 2]) -> anyhow::Result<usize> {
        match (bufs[0].len(), bufs[1].len()) {
            (len, 0) if len > 0 => {
                self.pwrite_all(offset, &bufs[0])?;
                Ok(len)
            }
            (0, len) if len > 0 => {
                self.pwrite_all(offset, &bufs[1])?;
                Ok(len)
            }
            (0, 0) => Ok(0),
            (l0, l1) => {
                // concatenate the buffers in memory so that we issue one write call instead of 2
                // assumes the message is <= CHUNK_SIZE
                use librqbit_core::constants::CHUNK_SIZE;
                let mut buf = [0u8; CHUNK_SIZE as usize];

                buf.get_mut(..l0)
                    .context("buf too small")?
                    .copy_from_slice(&bufs[0]);
                buf.get_mut(l0..l0 + l1)
                    .context("buf too small")?
                    .copy_from_slice(&bufs[1]);
                self.pwrite_all(offset, &buf[..l0 + l1])?;
                Ok(l0 + l1)
            }
        }
    }

    #[cfg(unix)]
    fn pread_exact(&self, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        use std::os::unix::fs::FileExt;

        Ok(self.read_exact_at(buf, offset)?)
    }

    #[cfg(windows)]
    fn pread_exact(&self, mut offset: u64, mut buf: &mut [u8]) -> anyhow::Result<()> {
        use std::os::windows::fs::FileExt;
        while !buf.is_empty() {
            let n = self.seek_read(buf, offset)?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof").into());
            }
            offset += n as u64;
            buf = &mut buf[n..];
        }
        Ok(())
    }

    #[cfg(not(any(windows, unix)))]
    fn pread_exact(&self, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        anyhow::bail!("pread_exact not implemented for your platform")
    }

    #[cfg(unix)]
    fn pwrite_all(&self, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        use std::os::unix::fs::FileExt;
        Ok(self.write_all_at(buf, offset)?)
    }

    #[cfg(windows)]
    fn pwrite_all(&self, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        use std::os::windows::fs::FileExt;

        let mut remaining = buf.len();
        let mut buf = buf;
        let mut offset = offset;
        while remaining > 0 {
            let written = self.seek_write(&buf[..remaining], offset)?;
            remaining -= written;
            offset += written as u64;
            buf = &buf[written..];
        }
        Ok(())
    }

    #[cfg(not(any(windows, unix)))]
    fn pwrite_all(&self, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        anyhow::bail!("pwrite_all not implemented for your platform")
    }
}

#[derive(Default, Debug)]
struct OpenedFileLocked {
    path: PathBuf,
    fd: Option<File>,
    /// Tribler : semantique d'ouverture a reproduire lors de
    /// l'ouverture paresseuse (`false` = `create_new` strict, comme
    /// l'init eager d'origine).
    allow_overwrite: bool,
    /// Longueur demandee par `ensure_file_length` avant que le fichier
    /// ne soit ouvert — appliquee juste apres l'open.
    pending_len: Option<u64>,
    #[cfg(windows)]
    tried_marking_sparse: bool,
}

impl Deref for OpenedFileLocked {
    type Target = Option<File>;

    fn deref(&self) -> &Self::Target {
        &self.fd
    }
}

impl DerefMut for OpenedFileLocked {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.fd
    }
}

#[derive(Debug)]
pub(crate) struct OpenedFile {
    file: RwLock<OpenedFileLocked>,
}

impl OpenedFile {
    /// Tribler : fichier enregistre sans etre ouvert. Le `fd` est cree
    /// a la premiere lecture/ecriture (`ensure_open`) — l'init d'un
    /// torrent multi-fichiers ne fait plus des milliers d'appels
    /// `CreateFile`/sparse/`SetEndOfFile` synchrones au demarrage.
    pub fn new_lazy(path: PathBuf, allow_overwrite: bool) -> Self {
        Self {
            file: RwLock::new(OpenedFileLocked {
                path,
                fd: None,
                allow_overwrite,
                pending_len: None,
                #[cfg(windows)]
                tried_marking_sparse: false,
            }),
        }
    }

    pub fn new_dummy() -> Self {
        Self {
            file: RwLock::new(Default::default()),
        }
    }

    /// `true` si le fichier a son `fd` ouvert (lazy deja materialise).
    pub fn is_open(&self) -> bool {
        self.file.read().fd.is_some()
    }

    pub fn take_clone(&self) -> anyhow::Result<Self> {
        let f = std::mem::take(&mut *self.file.write());
        Ok(Self {
            file: RwLock::new(f),
        })
    }

    /// Ouvre le fichier sous `g` s'il ne l'est pas encore (creation du
    /// dossier parent, options d'ouverture repliquees de l'init eager,
    /// `pending_len` appliquee).
    fn open_locked(g: &mut OpenedFileLocked) -> crate::Result<()> {
        if g.fd.is_some() {
            return Ok(());
        }
        // Fichier "dummy" (padding) : jamais de fd.
        if g.path.as_os_str().is_empty() {
            return Err(Error::FsFileIsNone);
        }
        let path = g.path.clone();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Anyhow(anyhow::anyhow!("error creating dir {parent:?}: {e:#}"))
            })?;
        }
        let f = if g.allow_overwrite {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
        } else {
            // Meme sequence que l'init eager : create_new puis rw.
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .and_then(|_| {
                    std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                })
        }
        .map_err(|e| Error::Anyhow(anyhow::anyhow!("error opening {path:?}: {e:#}")))?;
        if let Some(len) = g.pending_len.take() {
            f.set_len(len).map_err(|e| {
                Error::Anyhow(anyhow::anyhow!("error setting len {len} on {path:?}: {e:#}"))
            })?;
        }
        g.fd = Some(f);
        Ok(())
    }

    /// Retourne un guard sur le `File`, en l'ouvrant si necessaire.
    pub fn ensure_open(&self) -> crate::Result<impl Deref<Target = File>> {
        {
            let g = self.file.read();
            if g.fd.is_some() {
                return RwLockReadGuard::try_map(g, |f| f.fd.as_ref())
                    .ok()
                    .ok_or(Error::FsFileIsNone);
            }
        }
        let mut g = self.file.write();
        Self::open_locked(&mut g)?;
        let g = parking_lot::RwLockWriteGuard::downgrade(g);
        RwLockReadGuard::try_map(g, |f| f.fd.as_ref())
            .ok()
            .ok_or(Error::FsFileIsNone)
    }

    /// `set_len` immediat si le fichier est ouvert, sinon la longueur
    /// est enregistree et appliquee a l'ouverture paresseuse.
    pub fn ensure_len(&self, len: u64) -> crate::Result<()> {
        let mut g = self.file.write();
        if let Some(f) = g.fd.as_ref() {
            f.set_len(len)
                .map_err(|e| Error::Anyhow(anyhow::anyhow!("error setting len: {e:#}")))?;
        } else {
            g.pending_len = Some(len);
        }
        Ok(())
    }

    pub fn lock_read(&self) -> crate::Result<impl Deref<Target = File>> {
        self.ensure_open()
    }

    #[allow(dead_code)]
    pub fn lock_write(&self) -> crate::Result<impl DerefMut<Target = File>> {
        let mut g = self.file.write();
        Self::open_locked(&mut g)?;
        RwLockWriteGuard::try_map(g, |f| f.fd.as_mut())
            .ok()
            .ok_or(Error::FsFileIsNone)
    }

    #[cfg(windows)]
    pub fn try_mark_sparse(&self) -> crate::Result<impl Deref<Target = File>> {
        {
            let g = self.file.read();
            if g.fd.is_some() && g.tried_marking_sparse {
                return RwLockReadGuard::try_map(g, |f| f.fd.as_ref())
                    .ok()
                    .ok_or(Error::FsFileIsNone);
            }
        }
        let mut g = self.file.write();
        Self::open_locked(&mut g)?;
        if !g.tried_marking_sparse {
            g.tried_marking_sparse = true;
            let f = g.fd.as_ref().ok_or(Error::FsFileIsNone)?;
            tracing::debug!(path=?g.path, marked=super::sparse::mark_file_sparse(f), "marking sparse");
        }
        let g = parking_lot::RwLockWriteGuard::downgrade(g);
        RwLockReadGuard::try_map(g, |f| f.fd.as_ref())
            .ok()
            .ok_or(Error::FsFileIsNone)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use librqbit_core::constants::CHUNK_SIZE;
    use peer_binary_protocol::DoubleBufHelper;
    use tempfile::TempDir;

    use crate::storage::filesystem::opened_file::{OpenedFile, OurFileExt};
    use crate::Error;

    /// Chemin dont le parent est un fichier ordinaire : `create_dir_all`
    /// et l'ouverture echoueront au premier acces.
    fn unopenable_path(td: &TempDir) -> std::path::PathBuf {
        let blocker = td.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        blocker.join("child.bin")
    }

    #[test]
    fn test_lazy_erreur_differee_au_premier_acces() {
        let td = TempDir::with_prefix("test_lazy_err").unwrap();
        let path = unopenable_path(&td);

        // L'enregistrement paresseux ne touche pas le disque : aucune
        // erreur a la construction ni a ensure_len (longueur enregistree).
        let f = OpenedFile::new_lazy(path, true);
        f.ensure_len(42).unwrap();
        assert!(!f.is_open());

        // L'erreur (dossier/permissions) remonte au premier acces,
        // sans panique, et le fichier reste non ouvert.
        assert!(f.ensure_open().is_err());
        assert!(!f.is_open());
    }

    #[test]
    fn test_lazy_pending_len_appliquee_a_l_ouverture() {
        let td = TempDir::with_prefix("test_lazy_len").unwrap();
        let path = td.path().join("sub").join("f.bin");
        let f = OpenedFile::new_lazy(path.clone(), true);
        f.ensure_len(123).unwrap();
        // ensure_len differe : le fichier n'existe pas encore.
        assert!(!path.exists());
        let g = f.ensure_open().unwrap();
        assert_eq!(g.metadata().unwrap().len(), 123);
        assert!(f.is_open());
    }

    #[test]
    fn test_lazy_pread_sur_fichier_inaccessible() {
        let td = TempDir::with_prefix("test_lazy_pread").unwrap();
        let f = OpenedFile::new_lazy(unopenable_path(&td), true);
        // lock_read declenche ensure_open : erreur propre, pas de panique.
        assert!(f.lock_read().is_err());
        assert!(f.lock_write().is_err());
    }

    #[test]
    fn test_dummy_renvoie_fs_file_is_none() {
        let f = OpenedFile::new_dummy();
        assert!(matches!(f.ensure_open(), Err(Error::FsFileIsNone)));
    }

    #[test]
    fn test_pwrite_all_vectored() {
        let td = TempDir::with_prefix("test_pwrite_all_vectored").unwrap();
        let mut tmp_buf = [0u8; CHUNK_SIZE as usize];
        for bufsize in [10000usize, CHUNK_SIZE as usize] {
            let mut buf = vec![0u8; bufsize];
            rand::fill(&mut buf[..]);
            for split_point in [0, bufsize / 2, bufsize] {
                let path = td.path().join(format!("file_{bufsize}_{split_point}"));
                let file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&path)
                    .unwrap();
                let (first, second) = buf.split_at(split_point);
                let bufs = DoubleBufHelper::new(first, second).as_ioslices(bufsize);
                file.pwrite_all_vectored(0, bufs).unwrap();

                let mut file = std::fs::File::open(&path).unwrap();
                assert_eq!(file.metadata().unwrap().len(), bufsize as u64, "{path:?}");
                file.read_exact(&mut tmp_buf[..bufsize]).unwrap();
                assert_eq!(&tmp_buf[..bufsize], buf);
            }
        }
    }
}
