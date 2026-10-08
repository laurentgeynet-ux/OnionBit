// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `PrivateZone` — zone de telechargement privee liee a l'identite
//! (ADR-0018, etape 62).
//!
//! La zone regroupe sous une identite deverrouillee :
//!
//! - les **cles** `PrivateStoreKeys` derivees de la graine d'identite
//!   (`store_root` — jamais persistees, jamais dans les logs) ;
//! - le **manifeste** `data/private/manifest.obm` (codec
//!   `onionbit-crypto::obm`) : catalogue JSON des entrees privees,
//!   clees par `HMAC(K_names, infohash)` — la base publique ne porte
//!   que cette cle opaque, avec `name`/`source_uri`/`torrent_data`
//!   vides ;
//! - les **fabriques rqbit** : `PrivateStorageFactory` par sous-
//!   racine (`temp`/`downloads`) + `OpaqueBitV` partage (fastresume
//!   `<hmac>.bitv`, set `private_hashes` alimente a `create`).
//!
//! Cycle de vie : `mount` apres `try_start_identity` (zone absente
//! tant que l'identite est pending/locked → aucune lecture `.obd`),
//! `flush` a chaque mutation du catalogue, `purge_guest` au `stop`
//! d'une session invitee (zone `temp/.guest/` ephemere, jamais de
//! manifeste). Pertes : `manifest.obm` corrompu → `.bak` (codec) ;
//! double perte → reconstruction par balayage `scan_ct` des `.obd`.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use onionbit_bittorrent::Id20;
use onionbit_bittorrent::{OpaqueBitV, PrivateStorageFactory};
use onionbit_crypto::obdfile::{ObdFile, PrivateStoreKeys};
use onionbit_crypto::obm;
use serde::{Deserialize, Serialize};

use crate::paths::PathRoots;
use crate::CoreError;
use crate::Result;

/// Sous-racine physique d'une zone privee (`temp` = en cours,
/// `downloads` = termine).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateSubdir {
    /// `data/private/temp`.
    Temp,
    /// `data/private/downloads`.
    Downloads,
}

impl PrivateSubdir {
    /// Spec portable persistee (`@private/…`) associee.
    pub fn spec(self) -> &'static str {
        match self {
            Self::Temp => "@private/temp",
            Self::Downloads => "@private/downloads",
        }
    }
}

/// Entree du catalogue prive — l'equivalent prive des colonnes
/// `downloads` (qui restent opaques pour ces lignes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Infohash reel (hex 40) — jamais dans la base publique.
    pub infohash: String,
    /// Nom du contenu (libre, chiffre ici).
    pub name: Option<String>,
    /// URI d'origine (magnet/http/fichier) — permet le re-add.
    pub source_uri: String,
    /// Octets `.torrent` (hex) — necessaire quand `source_uri` est un
    /// simple magnet non resolu hors-ligne.
    pub torrent_data: Option<String>,
    /// Spec `@private/temp|downloads` de la sous-racine courante.
    pub output_dir: String,
    /// Ajoute en pause (`user_stopped` reste cote ligne publique).
    pub paused: bool,
    /// Date d'ajout (secondes Unix).
    pub added_on: i64,
}

/// Enveloppe versionnee du manifeste (le codec `obm` gere
/// l'en-tete `"OBM"‖v` ; ce `version` interne permet une evolution du
/// schema JSON sans toucher au format de transport).
#[derive(Serialize, Deserialize)]
struct ManifestDoc {
    version: u32,
    entries: BTreeMap<String, ManifestEntry>,
}

/// Bilan du balayage de montage — orphelins `.obd`/`.bitv` (presents
/// sur disque, absents du manifeste). Rapportes en `warn!`, purges a
/// la demande (`purge`) : jamais supprimes silencieusement.
#[derive(Debug, Default, Clone)]
pub struct OrphanReport {
    /// Dossiers de groupe `data/private/{temp,downloads}/<hmac>` sans
    /// entree manifeste correspondante.
    pub obd_groups: Vec<PathBuf>,
    /// Fichiers `<hmac>.bitv` de `data/private/rqbit` sans entree.
    pub bitv: Vec<PathBuf>,
}

/// Zone privee montee — une instance par session identitaire
/// deverrouillee. `Clone`able via `Arc` a l'exterieur.
pub struct PrivateZone {
    keys: Arc<PrivateStoreKeys>,
    roots: PathRoots,
    /// `log2` de la taille de chunk `OBD` (`private_chunk_kib`).
    chunk_log2: u8,
    /// Set partage avec les fabriques (alimente a `create` + montage).
    opaque: OpaqueBitV,
    /// Catalogue charge — cle = `row_key` hex (opaque).
    entries: Mutex<BTreeMap<String, ManifestEntry>>,
    /// Session invitee : aucun manifeste, racine `.guest/` ephemere.
    guest: bool,
}

impl std::fmt::Debug for PrivateZone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Cles et catalogue prives jamais dans les logs.
        f.debug_struct("PrivateZone")
            .field("guest", &self.guest)
            .field(
                "entries",
                &self.entries.lock().unwrap_or_else(|e| e.into_inner()).len(),
            )
            .finish_non_exhaustive()
    }
}

/// Lit la cle hex d'une entree (`HMAC` 20 o → hex 40).
fn row_key_hex(keys: &PrivateStoreKeys, infohash: &[u8; 20]) -> String {
    hex::encode(keys.row_key(infohash))
}

fn parse_ih(hex_str: &str) -> Option<Id20> {
    let bytes = hex::decode(hex_str).ok()?;
    let arr: [u8; 20] = bytes.try_into().ok()?;
    Some(Id20::new(arr))
}

impl PrivateZone {
    /// Monte la zone privee pour l'identite deverrouillee.
    ///
    /// - `guest` : racine `temp/.guest/`, aucun manifeste lu/ecrit,
    ///   `.bitv` non persistants (`OpaqueBitV` en mode invite).
    /// - `public_hashes` : infohashes des lignes publiques — sert a
    ///   distinguer un `.bitv` legitime d'un orphelin opaque.
    ///
    /// Retourne la zone et le rapport d'orphelins du balayage.
    pub fn mount(
        roots: PathRoots,
        keys: PrivateStoreKeys,
        guest: bool,
        chunk_log2: u8,
        public_hashes: &[Id20],
    ) -> (Self, OrphanReport) {
        let keys = Arc::new(keys);
        let (root, persistence_dir) = if guest {
            let guest_dir = roots.private_guest_temp();
            (guest_dir.clone(), guest_dir.join("rqbit"))
        } else {
            (roots.private_temp(), roots.private_rqbit())
        };
        for d in [
            root.clone(),
            roots.private_downloads(),
            persistence_dir.clone(),
        ] {
            if let Err(e) = std::fs::create_dir_all(&d) {
                tracing::warn!(dir = %d.display(), error = %e, "dossier prive non cree");
            }
        }
        let opaque = OpaqueBitV::new(
            Some(keys.clone()),
            Arc::new(AtomicBool::new(guest)),
            persistence_dir.clone(),
        );
        let zone = Self {
            keys,
            roots,
            chunk_log2,
            opaque,
            entries: Mutex::new(BTreeMap::new()),
            guest,
        };
        let mut report = OrphanReport::default();
        if !guest {
            zone.load_manifest();
            report = zone.scan_orphans(public_hashes);
        }
        (zone, report)
    }

    /// Chemin du manifeste (`data/private/manifest.obm`).
    fn manifest_path(&self) -> PathBuf {
        self.roots.private_manifest()
    }

    /// Racine physique d'une sous-zone.
    pub fn subdir_root(&self, sub: PrivateSubdir) -> PathBuf {
        if self.guest {
            return self.roots.private_guest_temp();
        }
        match sub {
            PrivateSubdir::Temp => self.roots.private_temp(),
            PrivateSubdir::Downloads => self.roots.private_downloads(),
        }
    }

    /// Fabrique de stockage chiffre pour une sous-zone — le set
    /// `private_hashes` partage y est branche.
    pub fn factory(&self, sub: PrivateSubdir) -> PrivateStorageFactory {
        PrivateStorageFactory::new(self.keys.clone(), self.subdir_root(sub), self.chunk_log2)
            .with_private_hashes(self.opaque.private_hashes().clone())
    }

    /// Objet partage `.bitv` opaque — a injecter dans
    /// `EngineConfig::opaque_bitv` au demarrage du moteur.
    pub fn opaque_bitv(&self) -> OpaqueBitV {
        self.opaque.clone()
    }

    /// Cle opaque de ligne publique pour `infohash`
    /// (`HMAC(K_names,"row/"‖h)` — la colonne `infohash` des lignes
    /// `private` porte ce hex, jamais l'infohash reel).
    pub fn row_key_hex(&self, infohash: &Id20) -> String {
        row_key_hex(&self.keys, &infohash.0)
    }

    /// Cle opaque en octets — valeur stockee dans la colonne
    /// `infohash` des lignes `private` de la base publique.
    pub fn row_key(&self, infohash: &Id20) -> [u8; 20] {
        self.keys.row_key(&infohash.0)
    }

    /// Cles de la zone (E/S directes `OBD` hors rqbit — bascules
    /// inter-zones de `move_storage`).
    pub fn keys(&self) -> &Arc<PrivateStoreKeys> {
        &self.keys
    }

    /// `true` en session invitee (zone ephemere, aucun manifeste).
    pub fn is_guest(&self) -> bool {
        self.guest
    }

    /// `log2` de la taille de chunk `OBD` configuree.
    pub fn chunk_log2(&self) -> u8 {
        self.chunk_log2
    }

    /// Dossier de groupe opaque d'un torrent dans une sous-zone
    /// (`<root>/<HMAC(grp/ih)>`).
    pub fn group_dir(&self, infohash: &Id20, sub: PrivateSubdir) -> PathBuf {
        self.subdir_root(sub)
            .join(self.keys.group_name(&infohash.0))
    }

    /// Racines portables (resolution des specs `@private/…`).
    pub fn roots(&self) -> &PathRoots {
        &self.roots
    }

    /// `h` est-il un telechargement prive connu ?
    pub fn is_private(&self, h: &Id20) -> bool {
        self.opaque.is_private(h)
    }

    /// Sous-zone courante d'un prive (`@private/…` spec → sous-dossier).
    fn subdir_of_spec(spec: &str) -> PrivateSubdir {
        if spec == PrivateSubdir::Downloads.spec() {
            PrivateSubdir::Downloads
        } else {
            PrivateSubdir::Temp
        }
    }

    /// Spec `@private/…` → sous-racine (exposee pour la restauration
    /// `readd_row` — `row.output_dir` persiste le spec).
    pub fn subdir_of_spec_pub(spec: &str) -> PrivateSubdir {
        Self::subdir_of_spec(spec)
    }

    /// Enregistre/met a jour l'entree catalogue d'un prive puis
    /// reecrit `manifest.obm` (atomique + rotation `.bak`).
    /// No-op persistant en invite (catalogue memoire seulement).
    pub fn upsert(&self, infohash: &Id20, entry: ManifestEntry) -> Result<()> {
        let key = self.row_key_hex(infohash);
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, entry);
        self.opaque.mark_private(*infohash);
        self.flush()
    }

    /// Retire l'entree catalogue (suppression d'un prive) + flush.
    pub fn remove(&self, infohash: &Id20) -> Result<()> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.row_key_hex(infohash));
        self.flush()
    }

    /// Entree par cle opaque (la colonne `infohash` d'une ligne
    /// `private` en base est ce hex).
    pub fn entry_by_row_key(&self, row_key_hex: &str) -> Option<ManifestEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(row_key_hex)
            .cloned()
    }

    /// Entree par infohash reel.
    pub fn entry(&self, infohash: &Id20) -> Option<ManifestEntry> {
        self.entry_by_row_key(&self.row_key_hex(infohash))
    }

    /// Toutes les entrees du catalogue (ordre stable — `BTreeMap`
    /// cle opaque). Reserve a l'endpoint protege `/api/private`.
    pub fn entries(&self) -> Vec<ManifestEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Sous-zone actuelle d'un prive (depuis le manifeste).
    pub fn subdir_of(&self, infohash: &Id20) -> PrivateSubdir {
        self.entry(infohash)
            .map(|e| Self::subdir_of_spec(&e.output_dir))
            .unwrap_or(PrivateSubdir::Temp)
    }

    /// Fabrique pointant la sous-zone actuelle du prive.
    pub fn factory_for(&self, infohash: &Id20) -> PrivateStorageFactory {
        self.factory(self.subdir_of(infohash))
    }

    /// Reecrit le manifeste depuis l'etat memoire (atomique).
    /// Echec `warn!` non fatal : le catalogue memoire reste coherent,
    /// le fichier sera regenere a la prochaine mutation.
    fn flush(&self) -> Result<()> {
        if self.guest {
            return Ok(());
        }
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let doc = ManifestDoc {
            version: 1,
            entries,
        };
        let json = serde_json::to_vec(&doc)
            .map_err(|e| CoreError::State(format!("serialisation manifest.obm: {e}")))?;
        obm::store_manifest(&self.manifest_path(), &self.keys, &json)
            .map_err(|e| CoreError::State(format!("ecriture manifest.obm: {e}")))
    }

    /// Charge `manifest.obm` (courant puis `.bak` via le codec) dans
    /// la map memoire + le set `private_hashes`. Corruption/double
    /// perte → `scan_orphans` reconstruit ce qui est lisible.
    fn load_manifest(&self) {
        let path = self.manifest_path();
        let plain = match obm::load_manifest(&path, &self.keys) {
            Ok(Some(p)) => p,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "lecture manifest.obm impossible");
                return;
            }
        };
        let doc: ManifestDoc = match serde_json::from_slice(&plain) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "manifest.obm JSON invalide — ignore");
                return;
            }
        };
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        for (key, entry) in doc.entries {
            if let Some(ih) = parse_ih(&entry.infohash) {
                self.opaque.mark_private(ih);
            }
            map.insert(key, entry);
        }
    }

    /// Balayage de secours + detection d'orphelins au montage.
    ///
    /// Pour chaque `.obd` sous `temp/`/`downloads/` : `scan_ct` rend
    /// `(infohash, relpath)` avec la seule graine — un groupe dont
    /// l'infohash n'a pas d'entree manifeste est reconstruit (entree
    /// minimale : `source_uri` = magnet `btih`, sous-racine observee)
    /// puis reporte si la reconstruction n'a rien donne. Idem pour
    /// les `<hmac>.bitv` de `data/private/rqbit` hors set connu.
    fn scan_orphans(&self, public_hashes: &[Id20]) -> OrphanReport {
        let mut report = OrphanReport::default();
        // Infohashes connus = manifeste + lignes publiques.
        let mut known: HashSet<Id20> = public_hashes.iter().copied().collect();
        {
            let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            for e in map.values() {
                if let Some(ih) = parse_ih(&e.infohash) {
                    known.insert(ih);
                }
            }
        }
        // Balayage des groupes `.obd`.
        for sub in [PrivateSubdir::Temp, PrivateSubdir::Downloads] {
            let root = self.subdir_root(sub);
            let Ok(groups) = std::fs::read_dir(&root) else {
                continue;
            };
            for g in groups.flatten() {
                let Ok(meta) = g.metadata() else { continue };
                if !meta.is_dir() || g.file_name() == ".guest" {
                    continue;
                }
                let mut group_ih: Option<Id20> = None;
                if let Ok(files) = std::fs::read_dir(g.path()) {
                    for f in files.flatten() {
                        let p = f.path();
                        if p.extension().is_some_and(|e| e == "obd") {
                            if let Ok(Some((ih, _relpath))) = ObdFile::scan_path(&p, &self.keys) {
                                group_ih = Some(Id20::new(ih));
                                break;
                            }
                        }
                    }
                }
                match group_ih {
                    Some(ih) if known.contains(&ih) => {}
                    Some(ih) => {
                        // Secours : groupe sans entree manifeste →
                        // entree minimale reconstruite (magnet btih —
                        // les metadonnees peuvent etre re-resolvees si
                        // le swarm existe encore).
                        tracing::warn!(
                            "groupe .obd orphelin — entree manifeste reconstruite depuis scan_ct"
                        );
                        let entry = ManifestEntry {
                            infohash: format!("{ih:?}"),
                            name: None,
                            source_uri: format!("magnet:?xt=urn:btih:{ih:?}"),
                            torrent_data: None,
                            output_dir: sub.spec().to_string(),
                            paused: true,
                            added_on: 0,
                        };
                        let key = self.row_key_hex(&ih);
                        self.entries
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(key, entry);
                        self.opaque.mark_private(ih);
                        known.insert(ih);
                    }
                    None => report.obd_groups.push(g.path()),
                }
            }
        }
        // `.bitv` opaques sans entree.
        if let Ok(files) = std::fs::read_dir(self.roots.private_rqbit()) {
            for f in files.flatten() {
                let p = f.path();
                if p.extension().is_none_or(|e| e != "bitv") {
                    continue;
                }
                let name = f.file_name();
                let Some(stem) = name.to_str().and_then(|s| s.strip_suffix(".bitv")) else {
                    continue;
                };
                let Some(ih) = parse_ih(stem) else { continue };
                // Le nom opaque d'un prive connu est `bitv_name(ih)` —
                // verifie ici pour distinguer legitime/orphelin.
                let legit = known
                    .iter()
                    .any(|h| h == &ih || Id20::new(self.keys.bitv_name(&h.0)) == ih);
                if !legit {
                    report.bitv.push(p);
                }
            }
        }
        if !report.obd_groups.is_empty() || !report.bitv.is_empty() {
            tracing::warn!(
                obd = report.obd_groups.len(),
                bitv = report.bitv.len(),
                "orphelins prives detectes au montage (rapportes, non purges)"
            );
        }
        report
    }

    /// Purge les orphelins d'un rapport (a la demande — jamais
    /// automatique : un `.obd` d'un torrent encore en cours de
    /// materialisation ne doit pas etre supprime par megarde).
    pub fn purge_orphans(&self, report: &OrphanReport) {
        for g in &report.obd_groups {
            if let Err(e) = std::fs::remove_dir_all(g) {
                tracing::warn!(dir = %g.display(), error = %e, "orphelin .obd non purge");
            }
        }
        for b in &report.bitv {
            if let Err(e) = std::fs::remove_file(b) {
                tracing::warn!(file = %b.display(), error = %e, "orphelin .bitv non purge");
            }
        }
    }

    /// Purge de la zone ephemere invitee au `stop` de session —
    /// `data/private/temp/.guest/` disparait entierement.
    pub fn purge_guest(&self) {
        if !self.guest {
            return;
        }
        let dir = self.roots.private_guest_temp();
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(dir = %dir.display(), error = %e, "zone invitee non purgee");
            }
        }
    }
}
