// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Format `OBD` — fichier chiffre par chunks pour la zone privee
//! (ADR-0018, etape 60).
//!
//! ```text
//! fichier  :  HDR_SLOT(4 Kio) ‖ chunk_0 ‖ chunk_1 ‖ …
//! HDR_SLOT :  hdr_nonce(12) ‖ hdr_ct ‖ scan_nonce(12) ‖ scan_ct ‖ pad(0)
//! hdr_ct   :  AEAD(K_file, hdr_nonce, "obd/hdr")〔"OBD" ‖ v(1)
//!             ‖ chunk_log2(1) ‖ file_id(16) ‖ plain_len(8) ‖ pad(0)〕
//! scan_ct  :  AEAD(K_scan, scan_nonce, "obd/scan")〔infohash(20)
//!             ‖ relpath_len(2) ‖ relpath(var) ‖ pad(0)〕
//! chunk_i  :  nonce_i(12) ‖ AEAD(K_file, nonce_i, file_id‖i:u64be)〔plain_i〕
//! ```
//!
//! Proprietes cles (voir l'ADR pour la rationale complete) :
//!
//! - **aucun marqueur statique** : magic et version vivent dans
//!   l'en-tete *chiffre* ; sans la graine, un `.obd` est
//!   indiscernable de bruit sous un nom HMAC ;
//! - **nonce aleatoire a chaque ecriture** (stocke en tete de chunk)
//!   — `pwrite_all` reecrit des chunks : un nonce derive de l'index
//!   serait une reutilisation ChaCha20-Poly1305 fatale ;
//! - **sceau de decouverte `scan_ct`** sous `K_scan` (cle unique
//!   derivee de la graine seule) : le balayage de secours ouvre
//!   aveuglement chaque `.obd` et retrouve `(infohash, relpath)` →
//!   derive `K_file` → ouvre `hdr_ct`. Sans ce sceau dedie la
//!   recuperation serait circulaire (`K_file` exige deja le couple) ;
//! - **integrite deleguee au hash de piece BitTorrent** : slot
//!   entierement nul → chunk absent → zeros ; slot non nul mais AEAD
//!   invalide ou physiquement tronque → `warn!` + zeros (jamais de
//!   gel du telechargement) ;
//! - **pas de verrou interne** : la serialisation RMW par (fichier,
//!   index de chunk) est une responsabilite de la factory
//!   (`storage_private`, etape 61 — verrous rayes `RwLock`) ;
//! - **sparse honnete** : un slot nul se lit comme des zeros partout,
//!   mais l'economie d'espace n'existe que sur les FS a creux
//!   supportes (NTFS marque `FSCTL_SET_SPARSE`, ext4, APFS) —
//!   FAT32/exFAT remplissent physiquement.
//!
//! Longueur du dernier chunk : non stockee, deduite de `plain_len`
//! (`ct = (plain_len mod chunk_size) + 28`, plein si reste nul) —
//! dependance assumee a l'en-tete chiffre.

use std::fs::File;
use std::io;
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::Rng;
use sha2::Sha256;
use thiserror::Error;

/// Taille fixe du slot d'en-tete : les positions des chunks sont
/// constantes quelle que soit la longueur du `relpath`.
pub const HDR_SLOT: usize = 4096;

/// Nonce ChaCha20-Poly1305 prefixe a chaque ciphertext.
pub const NONCE_LEN: usize = 12;
/// Tag Poly1305 suffixe a chaque ciphertext.
pub const TAG_LEN: usize = 16;
/// Surcout par chunk (`nonce ‖ tag`).
pub const CHUNK_OVERHEAD: usize = NONCE_LEN + TAG_LEN;

/// Capacite du `relpath` dans le sceau de decouverte : au-dela,
/// `relpath_len = 0` (secours absent — le format reste operationnel
/// via le manifest `OBM`).
pub const SCAN_RELPATH_CAP: usize = 3300;

const MAGIC: &[u8; 3] = b"OBD";
const VERSION: u8 = 1;

/// Domaine HKDF de la racine du stockage prive.
const INFO_STORE: &[u8] = b"onionbit/private-store/v1";
/// Domaines HKDF secondaires sous `K_store` (ADR-0018).
const INFO_SCAN: &[u8] = b"scan";
const INFO_NAMES: &[u8] = b"names";
const INFO_FILE_PREFIX: &[u8] = b"file/";
const INFO_MANIFEST: &[u8] = b"manifest";

/// AAD des sceaux d'en-tete (lien fonctionnel : un `scan_ct` colle a
/// la place d'un `hdr_ct` echoue au decrypt).
const AAD_HDR: &[u8] = b"obd/hdr";
const AAD_SCAN: &[u8] = b"obd/scan";

/// Domaines HMAC des noms opaques (`K_names`).
const HMAC_GROUP_PREFIX: &[u8] = b"grp/";
const HMAC_NAME_PREFIX: &[u8] = b"nam/";
const HMAC_BITV_PREFIX: &[u8] = b"bitv/";

/// Longueur hex des noms opaques (16 octets de HMAC → 32 chars).
const NAME_HMAC_LEN: usize = 16;

// En-tete chiffre : clair bourre a taille fixe (64 o) → `hdr_ct` a
// longueur constante (80 o), parsing univoque, zero fuite.
// clair : magic(3) ‖ v(1) ‖ chunk_log2(1) ‖ file_id(16) ‖ plain_len(8) ‖ pad(35)
const HDR_PLAIN_LEN: usize = 64;
const HDR_CT_LEN: usize = HDR_PLAIN_LEN + TAG_LEN;
const HDR_NONCE_OFF: usize = 0;
const HDR_CT_OFF: usize = NONCE_LEN;
const SCAN_NONCE_OFF: usize = HDR_CT_OFF + HDR_CT_LEN;
const SCAN_CT_OFF: usize = SCAN_NONCE_OFF + NONCE_LEN;

// Sceau de decouverte : clair fixe `infohash(20) ‖ len(2) ‖ relpath ‖ pad`.
const SCAN_PLAIN_LEN: usize = 20 + 2 + SCAN_RELPATH_CAP;
const SCAN_CT_LEN: usize = SCAN_PLAIN_LEN + TAG_LEN;
const _: () = assert!(
    SCAN_CT_OFF + SCAN_CT_LEN <= HDR_SLOT,
    "HDR_SLOT trop petit pour les deux sceaux"
);

/// Bornes du format = bornes de la config `private_chunk_kib`
/// (16 Kio…1 Mio) : 16 Kio = un bloc BitTorrent, borne l'amplification
/// RMW ; 1 Mio = plafond de lecture d'un chunk en clair.
const MIN_CHUNK_LOG2: u8 = 14; // 16 Kio
const MAX_CHUNK_LOG2: u8 = 20; // 1 Mio

/// Couple `(infohash, relpath)` extrait d'un sceau de decouverte
/// (`None` = secours absent, `relpath_len == 0`).
pub type ScanPayload = Option<([u8; 20], Vec<u8>)>;

/// Erreur du format `OBD`.
#[derive(Debug, Error)]
pub enum ObdError {
    /// Ouverture refusee — **uniforme** quelle que soit la cause
    /// (mauvaise cle, fichier corrompu, non-OBD, troncature) :
    /// aucun oracle différencie (convention blobs portables).
    #[error("ouverture OBD refusee (cle, format ou troncature)")]
    Open,
    /// Ecriture/lecture hors des bornes logiques (`plain_len`).
    #[error("acces OBD hors limites")]
    Bounds,
    /// E/S sous-jacente.
    #[error("E/S OBD: {0}")]
    Io(#[from] io::Error),
}

/// Lecture positionnee (`pread`) — `pread_exact` partiel : retourne
/// le nombre d'octets effectivement lus (troncature physique
/// detectee par l'appelant, pas par une erreur).
fn pread(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        f.read_at(buf, off)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        f.seek_read(buf, off)
    }
}

/// Ecriture positionnee (`pwrite`).
fn pwrite(f: &File, buf: &[u8], off: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        f.write_at(buf, off)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        f.seek_write(buf, off)
    }
}

/// Lit exactement `buf.len()` octets ou moins si l'EOF physique est
/// atteinte ; retourne le nombre lu.
fn pread_up_to(f: &File, buf: &mut [u8], mut off: u64) -> io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        let n = pread(f, &mut buf[done..], off)?;
        if n == 0 {
            break;
        }
        done += n;
        off += n as u64;
    }
    Ok(done)
}

/// Derive `32` octets HKDF-SHA256 depuis `prk` sous `info`.
fn hkdf32(prk: &[u8; 32], info: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let hk = Hkdf::<Sha256>::from_prk(prk).expect("PRK de 32 octets toujours valide");
    hk.expand(info, &mut out).expect("32 octets < limite HKDF");
    out
}

fn cipher(key: [u8; 32]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new((&key).into())
}

fn seal(c: &ChaCha20Poly1305, aad: &[u8], plain: &[u8]) -> ([u8; NONCE_LEN], Vec<u8>) {
    use chacha20poly1305::aead::Payload;
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let n = Nonce::from(nonce);
    let ct = c
        .encrypt(&n, Payload { msg: plain, aad })
        .expect("AEAD encrypt n'echoue pas");
    (nonce, ct)
}

fn open_seal(
    c: &ChaCha20Poly1305,
    aad: &[u8],
    nonce: &[u8; NONCE_LEN],
    ct: &[u8],
) -> Result<Vec<u8>, ObdError> {
    use chacha20poly1305::aead::Payload;
    let n = Nonce::from(*nonce);
    c.decrypt(&n, Payload { msg: ct, aad })
        .map_err(|_| ObdError::Open)
}

/// Cles du stockage prive — racine unique `K_store` derivee de la
/// graine d'identite (ou du `crypt_sk` legacy), sous-cles a domaines
/// separes (meme discipline qu'ADR-0016).
///
/// `ZeroizeOnDrop` : la racine ne survit pas en memoire apres usage.
#[derive(Clone, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct PrivateStoreKeys {
    k_store: [u8; 32],
}

impl PrivateStoreKeys {
    /// `K_store = HKDF(racine, "onionbit/private-store/v1")` — racine =
    /// graine d'identite `IdentitySeed` (installee) **ou** `crypt_sk`
    /// (identite legacy, ADR-0018 §3).
    pub fn from_root(root: &[u8; 32]) -> Self {
        Self {
            k_store: hkdf32(root, INFO_STORE),
        }
    }

    fn expand(&self, info: &[u8]) -> [u8; 32] {
        hkdf32(&self.k_store, info)
    }

    /// `K_scan` — cle de balayage unique : ouvre `scan_ct` sur tout
    /// `.obd` avec la seule graine (secours `OBM` perdu).
    pub fn scan_cipher(&self) -> ChaCha20Poly1305 {
        cipher(self.expand(INFO_SCAN))
    }

    /// `K_manifest` — cle du catalogue prive `manifest.obm`
    /// (`HKDF(K_store, "manifest")`, codec `obm.rs`, etape 62).
    pub fn manifest_cipher(&self) -> ChaCha20Poly1305 {
        cipher(self.expand(INFO_MANIFEST))
    }

    /// Cle de ligne publique : `HMAC(K_names, "row/"‖infohash)` —
    /// la colonne `infohash` des lignes `private` porte cette cle
    /// opaque, jamais l'infohash reel (ADR-0018 §catalogue).
    pub fn row_key(&self, infohash: &[u8; 20]) -> [u8; 20] {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.expand(INFO_NAMES))
            .expect("HMAC accepte toute taille de cle");
        mac.update(b"row/");
        mac.update(infohash);
        mac.finalize().into_bytes()[..20].try_into().unwrap()
    }

    /// `K_file(infohash, relpath)` — cle par (torrent, fichier) :
    /// l'infohash dans le domaine interdit le swap de chunks entre
    /// deux torrents prives partageant un `relpath`.
    pub fn file_cipher(&self, infohash: &[u8; 20], relpath: &[u8]) -> ChaCha20Poly1305 {
        let mut info = Vec::with_capacity(INFO_FILE_PREFIX.len() + 20 + 1 + relpath.len());
        info.extend_from_slice(INFO_FILE_PREFIX);
        info.extend_from_slice(infohash);
        info.push(b'/');
        info.extend_from_slice(relpath);
        cipher(self.expand(&info))
    }

    /// Nom de groupe opaque : `HMAC(K_names, "grp/"‖infohash)[..16]`
    /// en hex — l'infohash n'apparait jamais dans les noms (un
    /// infohash identifiable identifie le contenu via DHT/swarm).
    pub fn group_name(&self, infohash: &[u8; 20]) -> String {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.expand(INFO_NAMES))
            .expect("HMAC accepte toute taille de cle");
        mac.update(HMAC_GROUP_PREFIX);
        mac.update(infohash);
        hex::encode(&mac.finalize().into_bytes()[..NAME_HMAC_LEN])
    }

    /// Nom de fichier opaque : `HMAC(K_names, "nam/"‖infohash‖"/"‖
    /// relpath)[..16]` en hex + `.obd` — les vrais noms de fichiers
    /// (souvent plus revelateurs que le contenu) ne quittent jamais la
    /// zone en clair.
    pub fn file_name(&self, infohash: &[u8; 20], relpath: &[u8]) -> String {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.expand(INFO_NAMES))
            .expect("HMAC accepte toute taille de cle");
        mac.update(HMAC_NAME_PREFIX);
        mac.update(infohash);
        mac.update(b"/");
        mac.update(relpath);
        format!(
            "{}.obd",
            hex::encode(&mac.finalize().into_bytes()[..NAME_HMAC_LEN])
        )
    }

    /// Nom fastresume opaque : `HMAC(K_names, "bitv/"‖infohash)[..20]`
    /// — le `.bitv` prive est `<hmac>.bitv` au lieu de
    /// `<infohash>.bitv` en clair (`OpaqueBitVFactory`, etape 61).
    pub fn bitv_name(&self, infohash: &[u8; 20]) -> [u8; 20] {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.expand(INFO_NAMES))
            .expect("HMAC accepte toute taille de cle");
        mac.update(HMAC_BITV_PREFIX);
        mac.update(infohash);
        mac.finalize().into_bytes()[..20].try_into().unwrap()
    }
}

/// Fichier `OBD` ouvert — codec de chunks AEAD au-dessus de
/// `std::fs::File` positionne.
pub struct ObdFile {
    file: File,
    cipher: ChaCha20Poly1305,
    file_id: [u8; 16],
    chunk_log2: u8,
    /// Longueur logique — `AtomicU64` : `set_len` (`ensure_file_length`
    /// rqbit) peut concourir avec des lectures sous les verrous rayes
    /// de la factory.
    plain_len: std::sync::atomic::AtomicU64,
}

impl ObdFile {
    /// Cree un `OBD` : en-tete scelle + sceau de decouverte ecrits
    /// dans `HDR_SLOT`, `plain_len` fixe des le depart (la longueur
    /// *logique* — le fichier physique ne croit que des chunks ecrits).
    ///
    /// Le fichier est cree/tronque en lecture+ecriture : un `.obd`
    /// porte toujours un seul contenu. `relpath` >
    /// [`SCAN_RELPATH_CAP`] → sceau de secours ecrit avec
    /// `relpath_len = 0` (documente, format operationnel).
    pub fn create(
        path: &Path,
        keys: &PrivateStoreKeys,
        infohash: &[u8; 20],
        relpath: &[u8],
        chunk_log2: u8,
        plain_len: u64,
    ) -> Result<Self, ObdError> {
        if !(MIN_CHUNK_LOG2..=MAX_CHUNK_LOG2).contains(&chunk_log2) {
            return Err(ObdError::Open);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        let cipher = keys.file_cipher(infohash, relpath);
        let mut file_id = [0u8; 16];
        rand::rng().fill_bytes(&mut file_id);

        // En-tete principal (clair bourre a taille fixe).
        let mut hdr_plain = [0u8; HDR_PLAIN_LEN];
        hdr_plain[..3].copy_from_slice(MAGIC);
        hdr_plain[3] = VERSION;
        hdr_plain[4] = chunk_log2;
        hdr_plain[5..21].copy_from_slice(&file_id);
        hdr_plain[21..29].copy_from_slice(&plain_len.to_be_bytes());
        let (hdr_nonce, hdr_ct) = seal(&cipher, AAD_HDR, &hdr_plain);
        debug_assert_eq!(hdr_ct.len(), HDR_CT_LEN);

        // Sceau de decouverte (clair bourre fixe lui aussi).
        let mut scan_plain = [0u8; SCAN_PLAIN_LEN];
        scan_plain[..20].copy_from_slice(infohash);
        let rel_len = relpath.len().min(SCAN_RELPATH_CAP);
        let rel_len = if relpath.len() > SCAN_RELPATH_CAP {
            0
        } else {
            rel_len as u16
        };
        scan_plain[20..22].copy_from_slice(&rel_len.to_be_bytes());
        if rel_len > 0 {
            scan_plain[22..22 + rel_len as usize].copy_from_slice(relpath);
        }
        let (scan_nonce, scan_ct) = seal(&keys.scan_cipher(), AAD_SCAN, &scan_plain);
        debug_assert_eq!(scan_ct.len(), SCAN_CT_LEN);

        let mut slot = vec![0u8; HDR_SLOT];
        slot[HDR_NONCE_OFF..HDR_CT_OFF].copy_from_slice(&hdr_nonce);
        slot[HDR_CT_OFF..SCAN_NONCE_OFF].copy_from_slice(&hdr_ct);
        slot[SCAN_NONCE_OFF..SCAN_CT_OFF].copy_from_slice(&scan_nonce);
        slot[SCAN_CT_OFF..SCAN_CT_OFF + SCAN_CT_LEN].copy_from_slice(&scan_ct);
        let mut w = 0;
        while w < slot.len() {
            w += pwrite(&file, &slot[w..], w as u64)?;
        }
        file.sync_data()?;
        Ok(Self {
            file,
            cipher,
            file_id,
            chunk_log2,
            plain_len: std::sync::atomic::AtomicU64::new(plain_len),
        })
    }

    /// Ouvre un `OBD` existant (lecture+ecriture : RMW et `set_len`)
    /// — [`ObdError::Open`] uniforme quelle que soit la cause (pas
    /// d'oracle magic/version/cle).
    pub fn open(path: &Path, cipher: &ChaCha20Poly1305) -> Result<Self, ObdError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        if file.metadata()?.len() < HDR_SLOT as u64 {
            return Err(ObdError::Open);
        }
        let mut slot = [0u8; HDR_CT_OFF + HDR_CT_LEN];
        if pread_up_to(&file, &mut slot, 0)? < slot.len() {
            return Err(ObdError::Open);
        }
        let nonce: [u8; NONCE_LEN] = slot[HDR_NONCE_OFF..HDR_CT_OFF].try_into().unwrap();
        let plain = open_seal(cipher, AAD_HDR, &nonce, &slot[HDR_CT_OFF..])?;
        if plain.len() != HDR_PLAIN_LEN
            || plain[..3] != MAGIC[..]
            || plain[3] != VERSION
            || !(MIN_CHUNK_LOG2..=MAX_CHUNK_LOG2).contains(&plain[4])
        {
            return Err(ObdError::Open);
        }
        let mut file_id = [0u8; 16];
        file_id.copy_from_slice(&plain[5..21]);
        let plain_len = u64::from_be_bytes(plain[21..29].try_into().unwrap());
        Ok(Self {
            file,
            cipher: cipher.clone(),
            file_id,
            chunk_log2: plain[4],
            plain_len: std::sync::atomic::AtomicU64::new(plain_len),
        })
    }

    /// Sceau de decouverte d'un `.obd` : `(infohash, relpath)` —
    /// `Ok(None)` = secours absent (`relpath_len == 0`, chemin trop
    /// long pour le slot). [`ObdError::Open`] = sceau illisible.
    pub fn open_scan(file: &File, scan: &ChaCha20Poly1305) -> Result<ScanPayload, ObdError> {
        if file.metadata()?.len() < (SCAN_CT_OFF + SCAN_CT_LEN) as u64 {
            return Err(ObdError::Open);
        }
        let mut buf = [0u8; NONCE_LEN + SCAN_CT_LEN];
        if pread_up_to(file, &mut buf, SCAN_NONCE_OFF as u64)? < buf.len() {
            return Err(ObdError::Open);
        }
        let nonce: [u8; NONCE_LEN] = buf[..NONCE_LEN].try_into().unwrap();
        let plain = open_seal(scan, AAD_SCAN, &nonce, &buf[NONCE_LEN..])?;
        if plain.len() != SCAN_PLAIN_LEN {
            return Err(ObdError::Open);
        }
        let mut infohash = [0u8; 20];
        infohash.copy_from_slice(&plain[..20]);
        let rel_len = u16::from_be_bytes(plain[20..22].try_into().unwrap()) as usize;
        if rel_len == 0 || rel_len > SCAN_RELPATH_CAP {
            return Ok(None);
        }
        Ok(Some((infohash, plain[22..22 + rel_len].to_vec())))
    }

    /// Taille logique en clair (`ensure_file_length` ecrit ici).
    pub fn plain_len(&self) -> u64 {
        self.plain_len.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Identifiant aleatoire du fichier (lie chaque chunk a *ce*
    /// fichier via l'AAD — interdit le swap de chunks entre `.obd`).
    pub fn file_id(&self) -> &[u8; 16] {
        &self.file_id
    }

    /// Taille de chunk en clair (puissance de deux bornee).
    pub fn chunk_size(&self) -> usize {
        1usize << self.chunk_log2
    }

    /// Nombre de chunks logiques (`ceil(plain_len / chunk_size)`).
    pub fn chunk_count(&self) -> u64 {
        let cs = self.chunk_size() as u64;
        self.plain_len().div_ceil(cs)
    }

    /// AAD d'un chunk : `file_id ‖ index:u64be` — lie le chunk au
    /// fichier *et* a sa position (pas de permutation interne).
    fn chunk_aad(&self, index: u64) -> [u8; 24] {
        let mut aad = [0u8; 24];
        aad[..16].copy_from_slice(&self.file_id);
        aad[16..].copy_from_slice(&index.to_be_bytes());
        aad
    }

    /// `(offset_physique, ct_len)` du slot d'un chunk : taille plain du
    /// chunk deduite de `plain_len` pour le dernier (jamais stockee).
    fn chunk_slot(&self, index: u64) -> (u64, usize) {
        let cs = self.chunk_size() as u64;
        let start = index * cs;
        let plain = (self.plain_len() - start).min(cs);
        (
            HDR_SLOT as u64 + index * (cs + CHUNK_OVERHEAD as u64),
            plain as usize + CHUNK_OVERHEAD,
        )
    }

    /// Ecrit le contenu d'un chunk (nonce aleatoire + AEAD + pwrite du
    /// slot complet).
    fn write_chunk(&self, index: u64, plain: &[u8]) -> Result<(), ObdError> {
        let aad = self.chunk_aad(index);
        let (nonce, ct) = seal(&self.cipher, &aad, plain);
        let (slot_off, slot_len) = self.chunk_slot(index);
        debug_assert_eq!(ct.len() + NONCE_LEN, slot_len);
        let mut slot = Vec::with_capacity(slot_len);
        slot.extend_from_slice(&nonce);
        slot.extend_from_slice(&ct);
        let mut w = 0;
        while w < slot.len() {
            w += pwrite(&self.file, &slot[w..], slot_off + w as u64)?;
        }
        Ok(())
    }

    /// Lit un chunk complet en clair. Slot nul / absent → zeros ;
    /// AEAD invalide ou tronque → `warn!` + zeros (l'autorite
    /// d'integrite est le hash de piece — jamais de gel).
    fn read_chunk(&self, index: u64) -> io::Result<Vec<u8>> {
        let (slot_off, slot_len) = self.chunk_slot(index);
        let plain_len = slot_len - CHUNK_OVERHEAD;
        let physical = self.file.metadata()?.len();
        if physical <= slot_off {
            return Ok(vec![0u8; plain_len]); // jamais ecrit (creux)
        }
        let mut slot = vec![0u8; slot_len];
        let have = pread_up_to(&self.file, &mut slot, slot_off)?;
        if have == 0 || slot.iter().all(|&b| b == 0) {
            return Ok(vec![0u8; plain_len]); // slot nul = chunk absent
        }
        if have < slot_len {
            tracing::warn!(
                chunk = index,
                lu = have,
                attendu = slot_len,
                "OBD : slot physiquement tronque — chunk traite comme absent"
            );
            return Ok(vec![0u8; plain_len]);
        }
        let nonce: [u8; NONCE_LEN] = slot[..NONCE_LEN].try_into().unwrap();
        match open_seal(
            &self.cipher,
            &self.chunk_aad(index),
            &nonce,
            &slot[NONCE_LEN..],
        ) {
            Ok(p) if p.len() == plain_len => Ok(p),
            Ok(_) => {
                tracing::warn!(chunk = index, "OBD : clair de longueur inattendue — absent");
                Ok(vec![0u8; plain_len])
            }
            Err(_) => {
                tracing::warn!(
                    chunk = index,
                    "OBD : tag invalide (corruption) — chunk absent"
                );
                Ok(vec![0u8; plain_len])
            }
        }
    }

    /// Lecture en clair de `buf` a l'offset logique `off` — clippee a
    /// `plain_len` ; au-dela, zeros (comme un trou de fichier sparse).
    pub fn read_range(&self, mut off: u64, buf: &mut [u8]) -> Result<(), ObdError> {
        let cs = self.chunk_size() as u64;
        let mut done = 0usize;
        while done < buf.len() {
            if off >= self.plain_len() {
                buf[done..].fill(0);
                break;
            }
            let index = off / cs;
            let in_chunk = (off % cs) as usize;
            let want = (buf.len() - done).min(cs as usize - in_chunk);
            let plain = self.read_chunk(index)?;
            // La plage demandee peut depasser la fin logique du chunk
            // dernier : `plain` est la taille reelle, on borne.
            let have = want.min(plain.len().saturating_sub(in_chunk));
            buf[done..done + have].copy_from_slice(&plain[in_chunk..in_chunk + have]);
            if have < want {
                buf[done + have..done + want].fill(0);
            }
            done += want;
            off += want as u64;
        }
        Ok(())
    }

    /// Ecriture en clair a l'offset logique `off` — RMW par chunk pour
    /// les couvertures partielles (lecture de l'existant → patch →
    /// reecriture du slot complet avec nonce neuf).
    ///
    /// `off + data.len() > plain_len` → [`ObdError::Bounds`] : la
    /// longueur logique est fixee par [`Self::set_len`] /
    /// [`Self::create`], pas par les ecritures (l'ecriture au-dela
    /// serait un bug d'appelant, pas une extension implicite).
    ///
    /// **Non thread-safe** : la serialisation des RMW concurrentes sur
    /// un meme chunk est garantie par la factory (verrous rayes
    /// `(fichier, index)`, etape 61).
    pub fn write_range(&self, mut off: u64, data: &[u8]) -> Result<(), ObdError> {
        if off > self.plain_len() || data.len() as u64 > self.plain_len() - off {
            return Err(ObdError::Bounds);
        }
        let cs = self.chunk_size() as u64;
        let mut done = 0usize;
        while done < data.len() {
            let index = off / cs;
            let in_chunk = (off % cs) as usize;
            let want = (data.len() - done).min(cs as usize - in_chunk);
            let (_, slot_len) = self.chunk_slot(index);
            let chunk_plain = slot_len - CHUNK_OVERHEAD;
            if in_chunk == 0 && want == chunk_plain {
                // Couverture complete : pas de RMW.
                self.write_chunk(index, &data[done..done + want])?;
            } else {
                let mut plain = self.read_chunk(index)?;
                plain[in_chunk..in_chunk + want].copy_from_slice(&data[done..done + want]);
                self.write_chunk(index, &plain)?;
            }
            done += want;
            off += want as u64;
        }
        Ok(())
    }

    /// Fixe la longueur logique (`ensure_file_length` rqbit) :
    /// reecrit l'en-tete scelle avec un nonce neuf (le `scan_ct` reste
    /// octet pour octet — il ne depend pas de `plain_len`).
    pub fn set_len(&self, plain_len: u64) -> Result<(), ObdError> {
        let mut hdr_plain = [0u8; HDR_PLAIN_LEN];
        hdr_plain[..3].copy_from_slice(MAGIC);
        hdr_plain[3] = VERSION;
        hdr_plain[4] = self.chunk_log2;
        hdr_plain[5..21].copy_from_slice(&self.file_id);
        hdr_plain[21..29].copy_from_slice(&plain_len.to_be_bytes());
        let (nonce, ct) = seal(&self.cipher, AAD_HDR, &hdr_plain);
        let mut slot = Vec::with_capacity(HDR_CT_OFF + HDR_CT_LEN);
        slot.extend_from_slice(&nonce);
        slot.extend_from_slice(&ct);
        let mut w = 0;
        while w < slot.len() {
            w += pwrite(&self.file, &slot[w..], w as u64)?;
        }
        self.plain_len
            .store(plain_len, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Le `File` sous-jacent (fermeture/suppression par l'appelant).
    pub fn into_file(self) -> File {
        self.file
    }

    /// `true` si `path` est un `.obd` lisible par `keys` au sens du
    /// sceau de decouverte (le scan ne depend pas de `K_file`).
    pub fn scan_path(path: &Path, keys: &PrivateStoreKeys) -> Result<ScanPayload, ObdError> {
        let file = File::open(path)?;
        Self::open_scan(&file, &keys.scan_cipher())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> PrivateStoreKeys {
        PrivateStoreKeys::from_root(&[7u8; 32])
    }

    fn temp_obd() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.obd");
        (dir, path)
    }

    const IH: &[u8; 20] = b"01234567890123456789";

    #[test]
    fn roundtrip_multi_chunks_et_lectures_partielles() {
        let (_dir, path) = temp_obd();
        let plain: Vec<u8> = (0..100_000u64).map(|i| (i % 251) as u8).collect();
        let obd = ObdFile::create(&path, &keys(), IH, b"sub/dir/f.bin", 14, plain.len() as u64)
            .expect("create");
        assert_eq!(obd.plain_len(), plain.len() as u64);
        assert_eq!(obd.chunk_size(), 16384);

        // Ecriture non alignee couvrant plusieurs chunks (RMW).
        let part = &plain[5000..5000 + 40_000];
        obd.write_range(5000, part).unwrap();
        // Relecture integrale : hors zone ecrite → zeros.
        let mut got = vec![0xAA; plain.len()];
        obd.read_range(0, &mut got).unwrap();
        assert_eq!(&got[..5000], &vec![0u8; 5000][..]);
        assert_eq!(&got[5000..45_000], part);
        assert_eq!(&got[45_000..], &vec![0u8; 55_000][..]);
        // Ecrit les derniers octets (RMW partiel du dernier chunk)
        // pour verifier la frontiere logique/physique en relecture.
        obd.write_range(99_980, &plain[99_980..]).unwrap();
        drop(obd);

        // Reouverture : en-tete dechiffre, contenu relu identique.
        let obd = ObdFile::open(&path, &keys().file_cipher(IH, b"sub/dir/f.bin")).expect("open");
        assert_eq!(obd.plain_len(), plain.len() as u64);
        let mut got = vec![0u8; plain.len()];
        obd.read_range(0, &mut got).unwrap();
        assert_eq!(&got[5000..45_000], part);
        // Lecture au-dela de plain_len → zeros.
        let mut tail = [0xAAu8; 64];
        obd.read_range(plain.len() as u64 - 10, &mut tail).unwrap();
        assert_eq!(&tail[..10], &plain[plain.len() - 10..]);
        assert_eq!(&tail[10..], &[0u8; 54][..]);
    }

    #[test]
    fn ouverture_mauvaise_cle_refusee_uniformement() {
        let (_dir, path) = temp_obd();
        ObdFile::create(&path, &keys(), IH, b"a.bin", 14, 100).unwrap();
        let wrong = PrivateStoreKeys::from_root(&[9u8; 32]);
        assert!(matches!(
            ObdFile::open(&path, &wrong.file_cipher(IH, b"a.bin")),
            Err(ObdError::Open)
        ));
        // Mauvais relpath = autre K_file = meme refus uniforme.
        assert!(matches!(
            ObdFile::open(&path, &keys().file_cipher(IH, b"b.bin")),
            Err(ObdError::Open)
        ));
    }

    #[test]
    fn troncatures_a_chaque_borne_refusees() {
        let (_dir, path) = temp_obd();
        ObdFile::create(&path, &keys(), IH, b"a.bin", 14, 16).unwrap();
        let full = std::fs::read(&path).unwrap();
        for cut in [0, 1, NONCE_LEN, HDR_CT_OFF + 1, SCAN_CT_OFF, HDR_SLOT - 1] {
            std::fs::write(&path, &full[..cut.min(full.len())]).unwrap();
            assert!(
                ObdFile::open(&path, &keys().file_cipher(IH, b"a.bin")).is_err(),
                "troncature a {cut} acceptee"
            );
        }
    }

    #[test]
    fn bit_flip_chunk_donne_zeros_pas_erreur() {
        let (_dir, path) = temp_obd();
        let obd = ObdFile::create(&path, &keys(), IH, b"a.bin", 14, 4096).unwrap();
        obd.write_range(0, &vec![0x5A; 4096]).unwrap();
        drop(obd);
        // Corruption dans le chunk 0 (zone du ciphertext).
        let mut raw = std::fs::read(&path).unwrap();
        raw[HDR_SLOT + 20] ^= 0x01;
        std::fs::write(&path, &raw).unwrap();
        let obd = ObdFile::open(&path, &keys().file_cipher(IH, b"a.bin")).unwrap();
        let mut buf = vec![0xAA; 4096];
        obd.read_range(0, &mut buf).unwrap();
        assert!(
            buf.iter().all(|&b| b == 0),
            "chunk corrompu non remis a zero"
        );
    }

    #[test]
    fn bit_flip_en_tete_refuse_ouverture() {
        let (_dir, path) = temp_obd();
        ObdFile::create(&path, &keys(), IH, b"a.bin", 14, 16).unwrap();
        let mut raw = std::fs::read(&path).unwrap();
        raw[HDR_CT_OFF + 5] ^= 0x40;
        std::fs::write(&path, &raw).unwrap();
        assert!(matches!(
            ObdFile::open(&path, &keys().file_cipher(IH, b"a.bin")),
            Err(ObdError::Open)
        ));
    }

    #[test]
    fn nonce_neuf_a_chaque_reecriture() {
        let (_dir, path) = temp_obd();
        let obd = ObdFile::create(&path, &keys(), IH, b"a.bin", 14, 4096).unwrap();
        obd.write_range(0, &[1u8; 4096]).unwrap();
        let s1 = std::fs::read(&path).unwrap()[HDR_SLOT..HDR_SLOT + 40].to_vec();
        obd.write_range(0, &[2u8; 4096]).unwrap();
        let s2 = std::fs::read(&path).unwrap()[HDR_SLOT..HDR_SLOT + 40].to_vec();
        assert_ne!(s1, s2, "nonce/ciphertext reutilise a la reecriture");
        // Et le clair lu est la seconde version.
        let mut buf = [0u8; 4096];
        obd.read_range(0, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 2));
    }

    #[test]
    fn oracle_zero_constante() {
        // Deux fichiers de meme contenu sous deux identites : aucun
        // octet commun dans les regions chiffrees (le pad de fin de
        // HDR_SLOT est `0` par spec — exclu de la comparaison).
        let k2 = PrivateStoreKeys::from_root(&[8u8; 32]);
        let (_d1, p1) = temp_obd();
        let (_d2, p2) = temp_obd();
        let data = vec![0x11; 8192];
        let a = ObdFile::create(&p1, &keys(), IH, b"x", 14, 8192).unwrap();
        let b = ObdFile::create(&p2, &k2, IH, b"x", 14, 8192).unwrap();
        a.write_range(0, &data).unwrap();
        b.write_range(0, &data).unwrap();
        drop((a, b));
        let r1 = std::fs::read(&p1).unwrap();
        let r2 = std::fs::read(&p2).unwrap();
        let ciphered: Vec<(u8, u8)> = r1[..SCAN_CT_OFF + SCAN_CT_LEN]
            .iter()
            .chain(&r1[HDR_SLOT..])
            .zip(
                r2[..SCAN_CT_OFF + SCAN_CT_LEN]
                    .iter()
                    .chain(&r2[HDR_SLOT..]),
            )
            .map(|(x, y)| (*x, *y))
            .collect();
        let common = ciphered.iter().filter(|(x, y)| x == y).count();
        assert!(
            common < ciphered.len() / 100,
            "trop d'octets communs ({common}/{})",
            ciphered.len()
        );
        // La zone de pad, elle, est bien zero dans les deux fichiers.
        assert!(r1[SCAN_CT_OFF + SCAN_CT_LEN..HDR_SLOT]
            .iter()
            .all(|&b| b == 0));
    }

    #[test]
    fn scan_retrouve_infohash_et_relpath_sans_kfile() {
        let (_dir, path) = temp_obd();
        ObdFile::create(&path, &keys(), IH, b"un/chemin/long.txt", 14, 5).unwrap();
        let got = ObdFile::scan_path(&path, &keys()).unwrap().expect("scan");
        assert_eq!(&got.0, IH);
        assert_eq!(got.1, b"un/chemin/long.txt");
        // Mauvaise graine → sceau illisible.
        let wrong = PrivateStoreKeys::from_root(&[3u8; 32]);
        assert!(ObdFile::scan_path(&path, &wrong).is_err());
    }

    #[test]
    fn relpath_trop_long_secours_absent() {
        let (_dir, path) = temp_obd();
        let long = vec![b'a'; SCAN_RELPATH_CAP + 10];
        ObdFile::create(&path, &keys(), IH, &long, 14, 5).unwrap();
        assert!(ObdFile::scan_path(&path, &keys()).unwrap().is_none());
    }

    #[test]
    fn chunk_absent_lit_zeros_et_slot_tronque_invalide() {
        let (_dir, path) = temp_obd();
        let cs = 16384usize;
        let obd = ObdFile::create(&path, &keys(), IH, b"a.bin", 14, 3 * cs as u64).unwrap();
        // Ecrit le chunk 2 seulement : 0 et 1 restent des trous.
        obd.write_range(2 * cs as u64, &vec![7u8; cs]).unwrap();
        drop(obd);
        let obd = ObdFile::open(&path, &keys().file_cipher(IH, b"a.bin")).unwrap();
        let mut buf = vec![0xAA; 3 * cs];
        obd.read_range(0, &mut buf).unwrap();
        assert!(buf[..2 * cs].iter().all(|&b| b == 0));
        assert!(buf[2 * cs..].iter().all(|&b| b == 7));
        // Troncature physique en plein slot du chunk 2 → invalide → zeros.
        let end = (HDR_SLOT + 2 * (cs + CHUNK_OVERHEAD) + 100) as u64;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(end)
            .unwrap();
        let mut buf = vec![0xAA; cs];
        obd.read_range(2 * cs as u64, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 0), "slot tronque non invalide");
    }

    #[test]
    fn set_len_reecrit_en_tete_et_scan_inchange() {
        let (_dir, path) = temp_obd();
        let obd = ObdFile::create(&path, &keys(), IH, b"a.bin", 14, 4096).unwrap();
        obd.write_range(0, &[9u8; 4096]).unwrap();
        let hdr1 = std::fs::read(&path).unwrap()[..SCAN_NONCE_OFF].to_vec();
        obd.set_len(8192).unwrap();
        let hdr2 = std::fs::read(&path).unwrap()[..SCAN_NONCE_OFF].to_vec();
        assert_ne!(hdr1, hdr2, "nonce d'en-tete reutilise");
        let scan1 =
            std::fs::read(&path).unwrap()[SCAN_NONCE_OFF..SCAN_CT_OFF + SCAN_CT_LEN].to_vec();
        drop(obd);
        let obd = ObdFile::open(&path, &keys().file_cipher(IH, b"a.bin")).unwrap();
        assert_eq!(obd.plain_len(), 8192);
        let scan2 =
            std::fs::read(&path).unwrap()[SCAN_NONCE_OFF..SCAN_CT_OFF + SCAN_CT_LEN].to_vec();
        assert_eq!(scan1, scan2, "sceau de decouverte altere par set_len");
        // L'ancien dernier chunk (4096) est desormais trop court pour
        // son slot attendu (8192+28) → tronque → zeros : la longueur du
        // dernier chunk depend de `plain_len` (spec), l'integrite reste
        // au hash de piece (la piece de queue sera re-tiree).
        let mut buf = [0xAAu8; 8192];
        obd.read_range(0, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 0));
        // Reecriture complete a la nouvelle taille.
        obd.write_range(0, &[9u8; 8192]).unwrap();
        obd.read_range(0, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 9));
        // Hors limites de la nouvelle taille logique.
        assert!(matches!(obd.write_range(8192, &[1]), Err(ObdError::Bounds)));
    }

    #[test]
    fn noms_opaques_deterministes_et_distincts() {
        let k = keys();
        let g1 = k.group_name(IH);
        let f1 = k.file_name(IH, b"a/b.txt");
        let f2 = k.file_name(IH, b"a/c.txt");
        assert_eq!(g1.len(), 32);
        assert!(f1.ends_with(".obd"));
        assert_ne!(f1, f2);
        // Autre infohash → autre groupe, autre nom meme relpath.
        let ih2 = [1u8; 20];
        assert_ne!(k.group_name(&ih2), g1);
        assert_ne!(k.file_name(&ih2, b"a/b.txt"), f1);
        // Deterministe : la regeneration retrouve les memes noms.
        let k2 = PrivateStoreKeys::from_root(&[7u8; 32]);
        assert_eq!(k2.group_name(IH), g1);
        assert_eq!(k2.file_name(IH, b"a/b.txt"), f1);
        // Rien de lisible dans les noms (pas de sous-chaine du chemin).
        assert!(!f1.contains("b.txt"));
    }

    #[test]
    fn plain_len_zero_fichier_vide() {
        let (_dir, path) = temp_obd();
        let obd = ObdFile::create(&path, &keys(), IH, b"empty", 14, 0).unwrap();
        assert_eq!(obd.chunk_count(), 0);
        let mut buf = [1u8; 8];
        obd.read_range(0, &mut buf).unwrap();
        assert_eq!(buf, [0u8; 8]);
        assert!(matches!(obd.write_range(0, &[1]), Err(ObdError::Bounds)));
    }
}
