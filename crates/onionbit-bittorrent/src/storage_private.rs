// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Zone privee — `StorageFactory`/`TorrentStorage` librqbit au-dessus
//! du format `OBD` (ADR-0018, etape 61).
//!
//! Un torrent prive s'ecrit sous `<root>/<groupe>/<nom>.obd` ou
//! `<groupe>`/`nom` sont des HMAC opaques (`PrivateStoreKeys`) : ni
//! l'infohash ni les vrais noms de fichiers n'apparaissent sur disque.
//! Chaque `.obd` est chiffre par chunks ChaCha20-Poly1305 sous
//! `K_file = HKDF(K_store, "file/"‖infohash‖"/"‖relpath)`.
//!
//! **Concurrence** — le verrou librqbit est *par piece* et un chunk
//! `OBD` chevauche deux pieces (offsets non alignes) : deux
//! `pwrite_all` concurrents sur un meme chunk feraient des
//! lecture-modification-ecriture entrelacees → perte silencieuse, et
//! un `pread_exact` pendant une RMW lirait un ciphertext dechire (tag
//! invalide parasite). Chaque fichier porte donc des **verrous rayes
//! `RwLock`** indexes par `chunk_index % STRIPES` : `read()` partage
//! en lecture, `write()` exclusif en ecriture, acquis en ordre trie
//! (pas de deadlock). Pas de verrou global (revue externe 2).
//!
//! **Ouverture paresseuse** (parite `FilesystemStorage`) : `init`
//! n'ouvre rien — le `.obd` est cree/ouvert au premier acces, ce qui
//! evite les ~30 ms par fichier sous Windows sur les torrents a
//! milliers de fichiers.

use std::io::IoSlice;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{bail, Context};
use librqbit::storage::{BoxStorageFactory, StorageFactory, StorageFactoryExt, TorrentStorage};
use librqbit::{ManagedTorrentShared, TorrentMetadata};
use librqbit_core::lengths::ValidPieceIndex;
use onionbit_crypto::obdfile::{ObdFile, PrivateStoreKeys};

/// Nombre de rayures de verrouillage par fichier — bornant le cout
/// memoire (128 x `RwLock`) tout en conservant le parallelisme sur
/// les ecritures de blocs 16 Kio (un `pwrite` couvre ≤ 2 chunks).
const STRIPES: usize = 128;

/// Fabrique de stockage prive — une instance par couple (zone,
/// identite). `Clone` pour `StorageFactory::clone_box`.
#[derive(Clone)]
pub struct PrivateStorageFactory {
    keys: Arc<PrivateStoreKeys>,
    /// Racine de la zone (`<data>/private/temp` ou `…/downloads`).
    root: PathBuf,
    /// `log2` de la taille de chunk `OBD` (`private_chunk_kib`).
    chunk_log2: u8,
    /// Set partage des infohashes prives (`OpaqueBitV`) — alimente a
    /// `create`, avant le `bitv.load` de `initializing`, pour que le
    /// fastresume soit ecrit sous le nom opaque des le depart.
    private_hashes: Option<Arc<std::sync::Mutex<std::collections::HashSet<librqbit_core::Id20>>>>,
}

impl std::fmt::Debug for PrivateStorageFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // La cle n'apparait jamais dans les logs.
        f.debug_struct("PrivateStorageFactory")
            .field("root", &self.root)
            .field("chunk_log2", &self.chunk_log2)
            .finish_non_exhaustive()
    }
}

impl PrivateStorageFactory {
    pub fn new(keys: Arc<PrivateStoreKeys>, root: PathBuf, chunk_log2: u8) -> Self {
        Self {
            keys,
            root,
            chunk_log2,
            private_hashes: None,
        }
    }

    /// Branche le set partage `OpaqueBitV::private_hashes` : `create`
    /// y inscrit l'infohash du torrent (avant `bitv.load`), ce qui
    /// fait basculer son `.bitv` sous le nom opaque `<hmac>.bitv`.
    pub fn with_private_hashes(
        mut self,
        set: Arc<std::sync::Mutex<std::collections::HashSet<librqbit_core::Id20>>>,
    ) -> Self {
        self.private_hashes = Some(set);
        self
    }
}

/// Normalise un chemin relatif de torrent en octets `a/b/c` — le
/// separateur `/` est impose pour que `K_file`/`K_names` restent
/// stables d'un OS a l'autre (le bundle est portable).
fn relpath_bytes(relative: &Path) -> Vec<u8> {
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
        .into_bytes()
}

impl StorageFactory for PrivateStorageFactory {
    type Storage = PrivateStorage;

    fn create(
        &self,
        shared: &ManagedTorrentShared,
        _metadata: &TorrentMetadata,
    ) -> anyhow::Result<PrivateStorage> {
        let infohash = shared.info_hash.0;
        // Enregistre l'infohash dans le set partage AVANT le premier
        // `bitv.load` (`create_and_init` precede `initializing` chez
        // rqbit) → le fastresume est `<hmac>.bitv` des le depart.
        if let Some(set) = &self.private_hashes {
            set.lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(shared.info_hash);
        }
        let group_dir = self.root.join(self.keys.group_name(&infohash));
        Ok(PrivateStorage {
            keys: self.keys.clone(),
            infohash,
            chunk_log2: self.chunk_log2,
            group_dir,
            files: Vec::new(),
            retired: AtomicBool::new(false),
        })
    }

    fn clone_box(&self) -> BoxStorageFactory {
        self.clone().boxed()
    }
}

/// Un fichier du torrent prive : padding (factice) ou `.obd` reel.
struct PrivateFile {
    /// Padding BitTorrent : jamais materialise.
    padding: bool,
    /// `<root>/<groupe>/<hmac>.obd`.
    path: PathBuf,
    /// `relpath` normalise `/` — domaine de `K_file`.
    relpath: Vec<u8>,
    /// Longueur logique attendue (`ensure_file_length` avant le
    /// premier acces → appliquee a la creation).
    expected_len: AtomicU64,
    /// `.obd` materialise — `RwLock` : `read` pour les E/S courantes,
    /// `write` pour la creation/la suppression.
    cell: RwLock<Option<ObdFile>>,
    /// Suppression demandee : un acces ulterieur echoue au lieu de
    /// re-creer le fichier.
    removed: AtomicBool,
    /// Verrous rayes par index de chunk (`index % STRIPES`).
    stripes: Box<[RwLock<()>]>,
}

impl PrivateFile {
    fn new(path: PathBuf, relpath: Vec<u8>, len: u64) -> Self {
        Self {
            padding: false,
            path,
            relpath,
            expected_len: AtomicU64::new(len),
            cell: RwLock::new(None),
            removed: AtomicBool::new(false),
            stripes: (0..STRIPES).map(|_| RwLock::new(())).collect(),
        }
    }

    fn padding() -> Self {
        Self {
            padding: true,
            path: PathBuf::new(),
            relpath: Vec::new(),
            expected_len: AtomicU64::new(0),
            cell: RwLock::new(None),
            removed: AtomicBool::new(false),
            stripes: Box::new([]),
        }
    }

    /// Rayures couvrant `[offset, offset+len)` — ids tries dedupes
    /// (ordre d'acquisition global → pas de deadlock).
    fn stripe_ids(&self, offset: u64, len: usize, chunk_log2: u8) -> Vec<usize> {
        let cs = 1u64 << chunk_log2;
        let lo = offset / cs;
        let hi = if len == 0 {
            lo
        } else {
            (offset + len as u64 - 1) / cs
        };
        let mut v: Vec<usize> = (lo..=hi).map(|i| (i % STRIPES as u64) as usize).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Ouvre le `.obd` sous double-check. `for_write=false` : aucune
    /// creation — un fichier absent echoue en `NotFound` (le hashcheck
    /// marque la piece manquante au lieu de materialiser un stub, ce
    /// qui corromprait aussi la detection « fichiers manquants »).
    /// `for_write=true` : creation paresseuse ; un en-tete illisible
    /// (corruption) est **recree** — le fastresume/hash de piece
    /// invalidera le contenu et le retirera.
    fn materialize(&self, st: &PrivateStorage, for_write: bool) -> anyhow::Result<()> {
        st.ensure_alive()?;
        if self.removed.load(Ordering::Acquire) {
            bail!("fichier prive supprime");
        }
        {
            let g = self.cell.read().unwrap_or_else(|e| e.into_inner());
            if g.is_some() {
                return Ok(());
            }
        }
        let mut g = self.cell.write().unwrap_or_else(|e| e.into_inner());
        if g.is_some() {
            return Ok(());
        }
        if self.removed.load(Ordering::Acquire) {
            bail!("fichier prive supprime");
        }
        let cipher = st.keys.file_cipher(&st.infohash, &self.relpath);
        let expected = self.expected_len.load(Ordering::Acquire);
        let obd = match ObdFile::open(&self.path, &cipher) {
            Ok(o) => {
                if for_write && o.plain_len() != expected {
                    o.set_len(expected)?;
                }
                o
            }
            Err(onionbit_crypto::obdfile::ObdError::Io(e))
                if e.kind() == std::io::ErrorKind::NotFound && !for_write =>
            {
                return Err(anyhow::anyhow!(e)).context("OBD absent");
            }
            Err(onionbit_crypto::obdfile::ObdError::Io(e))
                if e.kind() == std::io::ErrorKind::NotFound =>
            {
                ObdFile::create(
                    &self.path,
                    &st.keys,
                    &st.infohash,
                    &self.relpath,
                    st.chunk_log2,
                    expected,
                )?
            }
            Err(e) => {
                if !for_write {
                    return Err(anyhow::anyhow!(e)).context("OBD illisible");
                }
                // En-tete corrompu : recree a neuf — le contenu serait
                // invalide au hash de piece de toute facon.
                tracing::warn!(
                    path = ?self.path,
                    error = %e,
                    "OBD illisible — recree (le contenu sera revalide)"
                );
                ObdFile::create(
                    &self.path,
                    &st.keys,
                    &st.infohash,
                    &self.relpath,
                    st.chunk_log2,
                    expected,
                )?
            }
        };
        *g = Some(obd);
        Ok(())
    }

    /// Execute `f` sur le `.obd` materialise (verrou `cell` partage).
    /// `for_write` propage la semantique creation/lecture a
    /// `materialize`.
    fn with_obd<R>(
        &self,
        st: &PrivateStorage,
        for_write: bool,
        f: impl FnOnce(&ObdFile) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        self.materialize(st, for_write)?;
        let g = self.cell.read().unwrap_or_else(|e| e.into_inner());
        f(g.as_ref().context("OBD vient d'etre materialise")?)
    }
}

/// `TorrentStorage` prive d'un torrent.
pub struct PrivateStorage {
    keys: Arc<PrivateStoreKeys>,
    infohash: [u8; 20],
    chunk_log2: u8,
    group_dir: PathBuf,
    files: Vec<Arc<PrivateFile>>,
    /// `take()` transfere la propriete au clone (parite
    /// `FilesystemStorage::take` qui *deplace* les handles) : sans
    /// cette marque l'ancien objet resterait pleinement fonctionnel
    /// — les taches orphelines d'un torrent supprime ou mis en pause
    /// poursuivraient leurs E/S, et `materialize` recreerait des
    /// `.obd` fraichement effaces.
    retired: AtomicBool,
}

impl PrivateStorage {
    fn file(&self, file_id: usize) -> anyhow::Result<&Arc<PrivateFile>> {
        self.files.get(file_id).context("no such file")
    }

    /// L'objet est-il encore vivant : `take()` l'a retire au profit
    /// du clone retourne. Les acces sur un objet retire echouent.
    fn ensure_alive(&self) -> anyhow::Result<()> {
        if self.retired.load(Ordering::Acquire) {
            bail!("stockage prive retire (take)");
        }
        Ok(())
    }
}

impl TorrentStorage for PrivateStorage {
    fn init(
        &mut self,
        _shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<()> {
        // Ouverture paresseuse (parite FilesystemStorage) : on
        // enregistre chemins et longueurs, la materialisation est
        // differee au premier acces. Le dossier de groupe est cree
        // ici (une seule creation par torrent).
        std::fs::create_dir_all(&self.group_dir)?;
        self.files = metadata
            .file_infos
            .iter()
            .map(|fi| {
                if fi.attrs.padding {
                    Arc::new(PrivateFile::padding())
                } else {
                    let rel = relpath_bytes(&fi.relative_filename);
                    Arc::new(PrivateFile::new(
                        self.group_dir
                            .join(self.keys.file_name(&self.infohash, &rel)),
                        rel,
                        fi.len,
                    ))
                }
            })
            .collect();
        // ADR-0018 etape 62 : materialise le premier `.obd` reel des
        // l'init — son sceau `scan_ct` est la seule cle de
        // reconstruction d'un groupe apres double perte du manifeste
        // (`manifest.obm` + `.bak`). Sans lui, un ajout prive reste
        // en pause avant toute E/S serait anonyme sur disque a tout
        // jamais (le nom de groupe et la ligne DB sont des HMAC
        // non inversibles). Les autres fichiers restent paresseux.
        if let Some(f) = self.files.iter().find(|f| !f.padding) {
            f.materialize(self, true)?;
        }
        Ok(())
    }

    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        self.ensure_alive()?;
        let f = self.file(file_id)?;
        if f.padding {
            buf.fill(0);
            return Ok(());
        }
        // Verrous rayes en lecture partagee : un `pread` pendant la
        // RMW d'un voisin lirait un ciphertext dechire sinon.
        let guards: Vec<_> = f
            .stripe_ids(offset, buf.len(), self.chunk_log2)
            .iter()
            .map(|&s| f.stripes[s].read().unwrap_or_else(|e| e.into_inner()))
            .collect();
        f.with_obd(self, false, |o| {
            o.read_range(offset, buf).map_err(|e| anyhow::anyhow!(e))
        })?;
        drop(guards);
        Ok(())
    }

    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        self.ensure_alive()?;
        let f = self.file(file_id)?;
        if f.padding {
            return Ok(());
        }
        // Verrous rayes exclusifs : les RMW concurrentes sur un meme
        // chunk (piece lock librqbit ≠ chunk OBD) perdraient un bloc.
        let guards: Vec<_> = f
            .stripe_ids(offset, buf.len(), self.chunk_log2)
            .iter()
            .map(|&s| f.stripes[s].write().unwrap_or_else(|e| e.into_inner()))
            .collect();
        f.with_obd(self, true, |o| {
            o.write_range(offset, buf).map_err(|e| anyhow::anyhow!(e))
        })?;
        drop(guards);
        Ok(())
    }

    fn pwrite_all_vectored(
        &self,
        file_id: usize,
        offset: u64,
        bufs: [IoSlice<'_>; 2],
    ) -> anyhow::Result<usize> {
        // Le defaut du trait ferait deux `pwrite_all` separes (deux
        // RMW sous deux verrouillages) — une seule plage sous un seul
        // verrouillage est strictement plus sûre et plus rapide.
        let total: usize = bufs.iter().map(|b| b.len()).sum();
        let mut joined = Vec::with_capacity(total);
        for b in &bufs {
            joined.extend_from_slice(b);
        }
        self.pwrite_all(file_id, offset, &joined)?;
        Ok(total)
    }

    fn ensure_file_length(&self, file_id: usize, length: u64) -> anyhow::Result<()> {
        self.ensure_alive()?;
        let f = self.file(file_id)?;
        if f.padding {
            return Ok(());
        }
        f.expected_len.store(length, Ordering::Release);
        // Si le fichier est deja materialise, on met a jour l'en-tete
        // — `set_len` reecrit `hdr_ct` (nonce neuf) ; exclusion
        // `cell.write()` contre la concurrence avec `materialize`.
        let g = f.cell.write().unwrap_or_else(|e| e.into_inner());
        if let Some(o) = g.as_ref() {
            o.set_len(length)?;
        }
        drop(g);
        Ok(())
    }

    fn remove_file(&self, file_id: usize, filename: &Path) -> anyhow::Result<()> {
        fn remove_path(path: &Path) -> anyhow::Result<()> {
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
        let Ok(f) = self.file(file_id) else {
            // Stockage jamais `init` (fallback `create` de
            // `Session::delete` quand l'etat n'exposait pas de
            // fichiers) : le nom `.obd` se derive du chemin relatif —
            // la suppression reste possible.
            let rel = relpath_bytes(filename);
            let path = self
                .group_dir
                .join(self.keys.file_name(&self.infohash, &rel));
            return remove_path(&path);
        };
        if f.padding {
            return Ok(());
        }
        f.removed.store(true, Ordering::Release);
        // Ferme le handle avant la suppression (Windows refuse de
        // supprimer un fichier ouvert).
        {
            let mut g = f.cell.write().unwrap_or_else(|e| e.into_inner());
            *g = None;
        }
        remove_path(&f.path)
    }

    fn remove_directory_if_empty(&self, _path: &Path) -> anyhow::Result<()> {
        // Notre arborescence est plate : `<root>/<groupe>/`. On tente
        // la suppression du groupe (rqbit passe des chemins relatifs
        // sans equivalent ici).
        match std::fs::remove_dir(&self.group_dir) {
            Ok(()) => Ok(()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                Ok(())
            }
            Err(e) => {
                tracing::warn!(dir = ?self.group_dir, error = %e, "groupe prive non supprime");
                Ok(())
            }
        }
    }

    fn take(&self) -> anyhow::Result<Box<dyn TorrentStorage>> {
        self.ensure_alive()?;
        // Retire l'ancien objet : les taches orphelines (check en vol,
        // dial annule) echouent au prochain acces au lieu de recreer
        // les fichiers qu'un `delete` vient d'effacer. Le clone herite
        // les `Arc<PrivateFile>` — cellules partagees, handles
        // conserves.
        self.retired.store(true, Ordering::Release);
        Ok(Box::new(Self {
            keys: self.keys.clone(),
            infohash: self.infohash,
            chunk_log2: self.chunk_log2,
            group_dir: self.group_dir.clone(),
            files: self.files.clone(),
            retired: AtomicBool::new(false),
        }))
    }

    fn on_piece_completed(&self, _piece_index: ValidPieceIndex) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage_de_test(dir: &tempfile::TempDir, len: u64) -> PrivateStorage {
        let keys = Arc::new(PrivateStoreKeys::from_root(&[9u8; 32]));
        let group_dir = dir.path().join("grp");
        std::fs::create_dir_all(&group_dir).unwrap();
        PrivateStorage {
            keys: keys.clone(),
            infohash: [7u8; 20],
            chunk_log2: 14,
            files: vec![Arc::new(PrivateFile::new(
                group_dir.join("f.obd"),
                b"f.bin".to_vec(),
                len,
            ))],
            group_dir,
            retired: AtomicBool::new(false),
        }
    }

    #[test]
    fn relpath_normalisee_en_slashs() {
        assert_eq!(
            relpath_bytes(Path::new("a").join("b").join("c.txt").as_path()),
            b"a/b/c.txt"
        );
        // Un chemin deja en `/` reste identique.
        assert_eq!(relpath_bytes(Path::new("x/y.bin")), b"x/y.bin");
    }

    /// Le verrou librqbit est par piece — des `pwrite_all` concurrents
    /// sur des zones disjointes *du meme chunk OBD* doivent tous
    /// survivre (sans verrous rayes, les RMW entrelacees perdraient un
    /// bloc).
    #[test]
    fn ecritures_concurrentes_meme_chunk_sans_perte() {
        let dir = tempfile::tempdir().unwrap();
        let cs = 16384usize;
        let st = storage_de_test(&dir, cs as u64);
        // 8 threads, chacun ecrit sa bande de 2 Kio dans le chunk 0.
        std::thread::scope(|s| {
            for t in 0..8u8 {
                let st = &st;
                s.spawn(move || {
                    st.pwrite_all(0, t as u64 * 2048, &vec![t + 1; 2048])
                        .unwrap();
                });
            }
        });
        let mut buf = vec![0u8; cs];
        st.pread_exact(0, 0, &mut buf).unwrap();
        for t in 0..8u8 {
            assert!(
                buf[t as usize * 2048..(t as usize + 1) * 2048]
                    .iter()
                    .all(|&b| b == t + 1),
                "bande {t} perdue — RMW entrelacees"
            );
        }
    }

    /// Deux ecritures concurrentes sur la MEME zone : le resultat est
    /// l'une des deux versions, jamais un entrelacement.
    #[test]
    fn ecritures_concurrentes_chevauchantes_sans_dechirure() {
        let dir = tempfile::tempdir().unwrap();
        let cs = 16384usize;
        let st = storage_de_test(&dir, 2 * cs as u64);
        std::thread::scope(|s| {
            for v in [0xAAu8, 0xBB] {
                let st = &st;
                s.spawn(move || {
                    // Chevauchement partiel : couvre la fin du chunk 0
                    // et le debut du chunk 1 → RMW des deux cotes.
                    st.pwrite_all(0, (cs - 4096) as u64, &vec![v; 8192])
                        .unwrap();
                });
            }
        });
        let mut buf = vec![0u8; cs * 2];
        st.pread_exact(0, 0, &mut buf).unwrap();
        let zone = &buf[cs - 4096..cs + 4096];
        assert!(
            zone.iter().all(|&b| b == 0xAA) || zone.iter().all(|&b| b == 0xBB),
            "zone dechiree — ni AA ni BB uniforme"
        );
    }

    /// Lecture pendant ecriture : un `pread_exact` concurrent ne voit
    /// jamais un ciphertext dechire (il lirait zeros via tag invalide
    /// — mais avec les verrous il voit avant/apres coherents).
    #[test]
    fn lecture_pendant_ecriture_coherente() {
        let dir = tempfile::tempdir().unwrap();
        let cs = 16384usize;
        let st = storage_de_test(&dir, cs as u64);
        st.pwrite_all(0, 0, &vec![0x11; cs]).unwrap();
        std::thread::scope(|s| {
            let writer = s.spawn(|| {
                for _ in 0..20 {
                    st.pwrite_all(0, 0, &vec![0x22; cs]).unwrap();
                    st.pwrite_all(0, 0, &vec![0x11; cs]).unwrap();
                }
            });
            let reader = s.spawn(|| {
                let mut buf = vec![0u8; cs];
                for _ in 0..40 {
                    st.pread_exact(0, 0, &mut buf).unwrap();
                    assert!(
                        buf.iter().all(|&b| b == 0x11) || buf.iter().all(|&b| b == 0x22),
                        "lecture dechiree"
                    );
                }
            });
            writer.join().unwrap();
            reader.join().unwrap();
        });
    }

    /// `take()` retire l'ancien stockage (parite
    /// `FilesystemStorage::take` qui deplace les handles) : les
    /// taches orphelines d'un delete/pause echouent au lieu de
    /// poursuivre leurs E/S, et `materialize` ne recree pas un
    /// `.obd` fraichement efface.
    #[test]
    fn take_retire_l_ancien_stockage() {
        let dir = tempfile::tempdir().unwrap();
        let st = storage_de_test(&dir, 16384);
        st.pwrite_all(0, 0, b"donnees").unwrap();
        let obd_path = st.files[0].path.clone();
        let new = st.take().unwrap();
        assert!(
            st.pread_exact(0, 0, &mut [0u8; 8]).is_err(),
            "ancien stockage encore vivant apres take()"
        );
        assert!(st.pwrite_all(0, 0, b"x").is_err());
        let mut buf = [0u8; 7];
        new.pread_exact(0, 0, &mut buf).unwrap();
        assert_eq!(&buf, b"donnees");
        // Suppression via le stockage vivant : le fichier part et
        // `removed` etant partage, aucun des deux objets ne le
        // recree.
        new.remove_file(0, Path::new("f.bin")).unwrap();
        assert!(!obd_path.exists());
        assert!(st.pread_exact(0, 0, &mut buf).is_err());
        assert!(new.pread_exact(0, 0, &mut buf).is_err());
        assert!(!obd_path.exists(), "obd recree apres remove_file");
    }

    /// `remove_file` sur un stockage jamais `init` (fallback
    /// `storage_factory.create` de `Session::delete`) : le nom `.obd`
    /// se derive du chemin relatif, la suppression fonctionne.
    #[test]
    fn remove_file_sans_init_derive_le_nom() {
        let dir = tempfile::tempdir().unwrap();
        let st = storage_de_test(&dir, 16384);
        let rel = relpath_bytes(Path::new("f.bin"));
        let derived = st.group_dir.join(st.keys.file_name(&st.infohash, &rel));
        std::fs::write(&derived, b"junk").unwrap();
        // Memes cles mais `files` vide — simule `create` sans `init`.
        let fresh = PrivateStorage {
            keys: st.keys.clone(),
            infohash: st.infohash,
            chunk_log2: st.chunk_log2,
            group_dir: st.group_dir.clone(),
            files: Vec::new(),
            retired: AtomicBool::new(false),
        };
        fresh.remove_file(0, Path::new("f.bin")).unwrap();
        assert!(!derived.exists());
    }
}
