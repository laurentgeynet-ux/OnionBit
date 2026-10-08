// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pieces jointes de messagerie (ADR-0019 §4, etape 67) —
//! orchestration cote session.
//!
//! Pipeline d'envoi : fichier source → staging borne sous
//! `@state/messaging/attachments/<attach_id>/` (jamais sous
//! `data/public` — le staging n'est pas un contenu publie) → torrent
//! standard `librqbit::create_torrent` → **salage post-encode** de
//! `x-onionbit` dans le dict `info` par `onionbit-format` (zero patch
//! vendored, infohash recalcule) → seed anonyme sur la lane
//! messagerie (`safe_seeding`, `downloads.origin='messaging'`) →
//! trames `attach` v2 aux destinataires.
//!
//! Pipeline de reception : `MessagingService` decode et persiste la
//! ligne `msg_attachments` en `offered` ; l'acceptation est un clic
//! explicite → magnet de l'infohash sale → `add_download_anon_area`
//! vers `destination {area, dir?}` (public → `@public/messaging/`,
//! prive → `temp`/`downloads` via `TorrentStorage`, `409
//! identity_locked` si zone verrouillee). Jamais de contenu de
//! fichier dans les trames.
//!
//! Expiration : `attach_reaper` expire les offres au-dela de
//! `attach_seed_ttl` et purge le staging residuel au-dela de
//! `upload_ttl` quand `attach_purge_on_expire`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use onionbit_db::conversations as dbc;
use onionbit_messaging::attach::AttachDesc;

use crate::config::StorageArea;
use crate::error::{CoreError, Result};
use crate::services::messaging::MessagingService;

use super::CoreSession;

/// Secondes Unix courantes.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Cible d'une offre de piece jointe.
#[derive(Debug, Clone)]
pub enum AttachTarget {
    /// Conversation directe — `pk_bin` du correspondant (la `conv`
    /// deterministe est derivee cote service).
    Contact(Vec<u8>),
    /// Conversation de groupe — `conv_id` (fan-out roster).
    Group([u8; 16]),
}

/// Resultat d'une offre de piece jointe.
#[derive(Debug)]
pub struct AttachOffer {
    /// `attach_id` local (cle `msg_attachments`, hex dans l'API).
    pub attach_id: [u8; 16],
    /// `conv_id` resolu de l'envoi.
    pub conv: [u8; 16],
    /// Infohash **sale** (essaim ephemere).
    pub ih: [u8; 20],
    /// Nom affiche.
    pub name: String,
    /// Taille du fichier en octets.
    pub size: u64,
    /// Nombre de trames `attach` effectivement emises.
    pub sent: usize,
}

/// Upload stage (import utilisateur en attente d'offre — TTL
/// `upload_ttl`, borne `attach_max_bytes`/`attach_stage_max_bytes`).
#[derive(Debug)]
pub struct UploadInfo {
    /// `upload_id` (hex dans l'API — dossier `uploads/<id>/`).
    pub upload_id: [u8; 16],
    /// Nom affiche (basename).
    pub name: String,
    /// Taille du fichier stage.
    pub size: u64,
    /// Chemin du fichier stage (source d'`attach_offer`).
    pub path: PathBuf,
}

/// Resultat d'une acceptation — la resolution du magnet est
/// deportee, aucun `Download` n'est materialise a la reponse.
#[derive(Debug)]
pub struct AttachAccept {
    /// Infohash sale (hex) — le download apparaitra dans
    /// `GET /api/downloads` a la materialisation.
    pub infohash: String,
    /// Nom affiche de l'offre acceptee.
    pub name: String,
}

impl CoreSession {
    /// Service messagerie courant (`None` si desactive ou stack non
    /// prete — aucune piece jointe possible alors).
    fn messaging(&self) -> Option<Arc<MessagingService>> {
        self.inner.ipv8_stack().and_then(|s| s.messaging.clone())
    }

    /// Service messagerie ou erreur — les operations de piece jointe
    /// exigent la messagerie active (404 cote API).
    fn require_messaging(&self) -> Result<Arc<MessagingService>> {
        self.messaging().ok_or(CoreError::InvalidState(
            "messagerie desactivee ou stack non prete",
        ))
    }

    /// Racine du staging des pieces jointes : `@state/messaging/
    /// attachments/` — hors du stockage public (le staging seede
    /// l'essaim ephemere, ce n'est pas un contenu publie).
    pub fn attach_stage_dir(&self) -> PathBuf {
        self.paths().state().join("messaging").join("attachments")
    }

    /// Racine des uploads stages : `@state/messaging/uploads/` —
    /// imports utilisateur en attente d'offre (TTL `upload_ttl`),
    /// distinct du staging d'envoi `attachments/`.
    pub fn upload_stage_dir(&self) -> PathBuf {
        self.paths().state().join("messaging").join("uploads")
    }

    /// Taille totale d'un dossier (borne `attach_stage_max_bytes`).
    fn dir_size(dir: &Path) -> u64 {
        let mut total = 0u64;
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    total += Self::dir_size(&p);
                } else if let Ok(m) = e.metadata() {
                    total += m.len();
                }
            }
        }
        total
    }

    /// Nom de piece jointe valide : non vide, borne
    /// `group_name_max_len`, basename seul (jamais de separateur).
    fn check_attach_name(cfg: &onionbit_messaging::MessagingConfig, name: &str) -> Result<()> {
        if name.is_empty()
            || name.len() > cfg.group_name_max_len
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(CoreError::InvalidState(
                "attach : nom de piece jointe hors borne",
            ));
        }
        Ok(())
    }

    /// Garde commune staging : borne par fichier + borne globale sur
    /// la racine `messaging/` (uploads + attachments confondus).
    fn check_stage_bounds(
        cfg: &onionbit_messaging::MessagingConfig,
        stage_parent: &Path,
        file_len: u64,
        dir: &Path,
    ) -> Result<()> {
        if file_len == 0 || file_len > cfg.attach_max_bytes {
            let _ = std::fs::remove_dir_all(dir);
            return Err(CoreError::InvalidState(
                "attach : taille hors borne attach_max_bytes",
            ));
        }
        if Self::dir_size(stage_parent) > cfg.attach_stage_max_bytes {
            let _ = std::fs::remove_dir_all(dir);
            return Err(CoreError::InvalidState(
                "attach : staging sature (attach_stage_max_bytes)",
            ));
        }
        Ok(())
    }

    /// `POST /uploads {path}` — stage un fichier local sous
    /// `@state/messaging/uploads/<id>/<name>`. `@private` refuse
    /// (pas de copie claire sauvage d'un contenu chiffre OBD).
    pub fn stage_upload_path(
        &self,
        source: &Path,
        display_name: Option<String>,
    ) -> Result<UploadInfo> {
        let svc = self.require_messaging()?;
        let cfg = svc.config();
        let meta = std::fs::metadata(source)
            .map_err(|e| CoreError::State(format!("upload : source illisible: {e}")))?;
        if !meta.is_file() {
            return Err(CoreError::InvalidState("upload : pas un fichier"));
        }
        if self
            .inner
            .paths
            .to_portable(source)
            .is_some_and(|s| s == "@private" || s.starts_with("@private/"))
        {
            return Err(CoreError::InvalidState(
                "upload : source refusee depuis @private",
            ));
        }
        let name = display_name
            .or_else(|| source.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "fichier".into());
        Self::check_attach_name(cfg, &name)?;
        let upload_id: [u8; 16] = rand::random();
        let parent = self.paths().state().join("messaging");
        let dir = self
            .upload_stage_dir()
            .join(onionbit_crypto::hash::to_hex(&upload_id));
        std::fs::create_dir_all(&dir)
            .map_err(|e| CoreError::State(format!("upload : staging: {e}")))?;
        let staged = dir.join(&name);
        if let Err(e) = std::fs::copy(source, &staged) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(CoreError::State(format!("upload : copie: {e}")));
        }
        Self::check_stage_bounds(cfg, &parent, meta.len(), &dir)?;
        Ok(UploadInfo {
            upload_id,
            name,
            size: meta.len(),
            path: staged,
        })
    }

    /// `POST /uploads` octets — variante pour les envois API/UI
    /// (drag & drop, web) : ecrit les octets sous
    /// `uploads/<id>/<name>`. `bytes.len()` borne avant ecriture.
    pub fn stage_upload_bytes(&self, name: &str, bytes: &[u8]) -> Result<UploadInfo> {
        let svc = self.require_messaging()?;
        let cfg = svc.config();
        Self::check_attach_name(cfg, name)?;
        if bytes.is_empty() || bytes.len() as u64 > cfg.attach_max_bytes {
            return Err(CoreError::InvalidState(
                "upload : taille hors borne attach_max_bytes",
            ));
        }
        let upload_id: [u8; 16] = rand::random();
        let parent = self.paths().state().join("messaging");
        let dir = self
            .upload_stage_dir()
            .join(onionbit_crypto::hash::to_hex(&upload_id));
        std::fs::create_dir_all(&dir)
            .map_err(|e| CoreError::State(format!("upload : staging: {e}")))?;
        let staged = dir.join(name);
        if let Err(e) = std::fs::write(&staged, bytes) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(CoreError::State(format!("upload : ecriture: {e}")));
        }
        Self::check_stage_bounds(cfg, &parent, bytes.len() as u64, &dir)?;
        Ok(UploadInfo {
            upload_id,
            name: name.to_string(),
            size: bytes.len() as u64,
            path: staged,
        })
    }

    /// Resout `upload_id` → chemin du fichier stage (offre a partir
    /// d'un upload `POST /uploads`).
    pub fn upload_path(&self, upload_id: &[u8]) -> Result<PathBuf> {
        if upload_id.len() != 16 {
            return Err(CoreError::InvalidState("upload : id attendu 16 octets"));
        }
        let dir = self
            .upload_stage_dir()
            .join(onionbit_crypto::hash::to_hex(upload_id));
        let rd = std::fs::read_dir(&dir)
            .map_err(|_| CoreError::InvalidState("upload : inconnu ou expire"))?;
        for e in rd.flatten() {
            if e.path().is_file() {
                return Ok(e.path());
            }
        }
        Err(CoreError::InvalidState("upload : dossier vide"))
    }

    /// Offre un fichier en piece jointe (pipeline d'envoi complet).
    ///
    /// `source` : chemin lisible du fichier — `@private` est refuse
    /// (l'upload `{path}` d'un contenu prive passerait par
    /// `TorrentStorage` dechiffre → jamais de copie claire sauvage ;
    /// le chemin direct reste interdit par l'ADR).
    /// `display_name` : nom affiche (defaut : nom du fichier).
    pub async fn attach_offer(
        &self,
        target: AttachTarget,
        source: &Path,
        display_name: Option<String>,
    ) -> Result<AttachOffer> {
        let svc = self.require_messaging()?;
        let cfg = svc.config();
        let meta = std::fs::metadata(source)
            .map_err(|e| CoreError::State(format!("attach : source illisible: {e}")))?;
        if !meta.is_file() {
            return Err(CoreError::InvalidState(
                "attach : la source n'est pas un fichier",
            ));
        }
        if meta.len() == 0 || meta.len() > cfg.attach_max_bytes {
            return Err(CoreError::InvalidState(
                "attach : taille hors borne attach_max_bytes",
            ));
        }
        if self
            .inner
            .paths
            .to_portable(source)
            .is_some_and(|s| s == "@private" || s.starts_with("@private/"))
        {
            return Err(CoreError::InvalidState(
                "attach : upload direct refuse depuis @private",
            ));
        }
        let name = display_name
            .or_else(|| source.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "fichier".into());
        // Le nom circule en clair dans la trame — basename seul,
        // jamais de separateur de chemin.
        Self::check_attach_name(cfg, &name)?;
        // Staging `<@state>/messaging/attachments/<attach_id>/<name>`.
        let attach_id: [u8; 16] = rand::random();
        let stage_root = self.attach_stage_dir();
        let stage = stage_root.join(onionbit_crypto::hash::to_hex(&attach_id));
        std::fs::create_dir_all(&stage)
            .map_err(|e| CoreError::State(format!("attach : staging: {e}")))?;
        let staged = stage.join(&name);
        if let Err(e) = std::fs::copy(source, &staged) {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(CoreError::State(format!("attach : copie staging: {e}")));
        }
        // Borne globale du staging — un abus d'uploads ne doit pas
        // remplir `@state`.
        if Self::dir_size(&stage_root) > cfg.attach_stage_max_bytes {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(CoreError::InvalidState(
                "attach : staging sature (attach_stage_max_bytes)",
            ));
        }
        // Torrent standard, puis salage `x-onionbit` post-encode :
        // le dict `info` serialise gagne `10:x-onionbit16:<sel>` et
        // l'infohash est recalcule — indevinable pour un contenu
        // connu, sans patch vendored (ADR-0019 §4).
        let opts = librqbit::CreateTorrentOptions {
            name: Some(&name),
            trackers: Vec::new(),
            piece_length: None,
        };
        let spawner = librqbit::spawn_utils::BlockingSpawner::new(1);
        let created = librqbit::create_torrent(&staged, opts, &spawner)
            .await
            .map_err(|e| CoreError::State(format!("attach : create_torrent: {e}")))?;
        let raw = created
            .as_bytes()
            .map_err(|e| CoreError::State(format!("attach : torrent: {e}")))?;
        let salt: [u8; onionbit_format::torrent::ONIONBIT_SALT_LEN] = rand::random();
        let (bytes, ih) = onionbit_format::torrent::salt_torrent(&raw, &salt)?;
        // Seed anonyme : lane messagerie, `safe_seeding` (obligatoire
        // si `anon_hops>0`), le fichier etant deja dans
        // `output_folder` l'ajout passe directement en seeding.
        let hops = self.inner.config.ipv8.messaging_hops as u32;
        if hops == 0 {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(CoreError::InvalidState(
                "attach : une piece jointe exige l'anonymat (messaging_hops = 0)",
            ));
        }
        self.add_torrent_bytes_anon_area(
            bytes,
            false,
            hops,
            true,
            Some(stage.clone()),
            StorageArea::Public,
        )
        .await?;
        // Origine `messaging` : la ligne est distinguee des ajouts
        // utilisateur (purge TTL, affichage UI).
        self.inner
            .db_arc()
            .with(|c| onionbit_db::downloads::set_origin(c, &ih, "messaging"))?;
        // Trames `attach` v2 vers la conversation.
        let conv = match &target {
            AttachTarget::Contact(pk) => svc.direct_conv(pk),
            AttachTarget::Group(c) => *c,
        };
        let mid: [u8; 16] = rand::random();
        let desc = AttachDesc {
            ih,
            mid,
            name: name.clone(),
            size: meta.len(),
        };
        let sent = svc.attach_send(conv, attach_id, &desc).await?;
        Ok(AttachOffer {
            attach_id,
            conv,
            ih,
            name,
            size: meta.len(),
            sent,
        })
    }

    /// Accepte une offre recue : telechargement anonyme de l'infohash
    /// sale vers `destination {area, dir?}` (ADR-0019 §4 — grammaire
    /// ADR-0018). `area=None` → `attach_area` du service ;
    /// `destination=None` → `@public/messaging/` (public) ou
    /// `@private/downloads` (prive, noms opaques OBD).
    ///
    /// La resolution BEP 9 du magnet est **deportee en tache** (idem
    /// `PUT /downloads` — elle peut durer longtemps en lane anonyme) :
    /// l'offre passe `accepted` tout de suite, `downloading` a la
    /// materialisation, `done` au reaper quand le download termine.
    /// Un echec de resolution rend l'offre `offered` (reessai).
    pub async fn attach_accept(
        &self,
        attach_id: &[u8],
        destination: Option<PathBuf>,
        area: Option<StorageArea>,
    ) -> Result<AttachAccept> {
        let svc = self.require_messaging()?;
        let db = self.inner.db_arc();
        let row = db
            .with(|c| dbc::get_attachment(c, attach_id))?
            .ok_or(CoreError::InvalidState("attach : offre inconnue"))?;
        if row.role != "recv" || row.state != "offered" {
            return Err(CoreError::InvalidState(
                "attach : offre deja traitee ou non recue",
            ));
        }
        let area = area.unwrap_or(StorageArea::parse(&svc.config().attach_area));
        if area == StorageArea::Private && self.private_area_state() == "locked" {
            return Err(CoreError::InvalidState("identity_locked"));
        }
        let dest = destination.or(match area {
            StorageArea::Public => Some(self.paths().public().join("messaging")),
            StorageArea::Private => Some(self.paths().private_downloads()),
        });
        let hops = self.inner.config.ipv8.messaging_hops as u32;
        if hops == 0 {
            return Err(CoreError::InvalidState(
                "attach : une piece jointe exige l'anonymat (messaging_hops = 0)",
            ));
        }
        let ih_hex = onionbit_crypto::hash::to_hex(&row.ih);
        let magnet = format!("magnet:?xt=urn:btih:{ih_hex}");
        self.inner
            .db_arc()
            .with(|c| dbc::set_attachment_state(c, attach_id, "accepted"))?;
        let session = self.clone();
        let aid = attach_id.to_vec();
        let ih = row.ih.clone();
        tokio::spawn(async move {
            match session
                .add_download_anon_area(&magnet, false, hops, true, dest, area)
                .await
            {
                Ok(_) => {
                    let _ = session
                        .inner
                        .db_arc()
                        .with(|c| onionbit_db::downloads::set_origin(c, &ih, "messaging"));
                    let _ = session
                        .inner
                        .db_arc()
                        .with(|c| dbc::set_attachment_state(c, &aid, "downloading"));
                }
                Err(e) => {
                    // Annulation (suppression du pending) ou echec :
                    // l'offre redevient `offered` — l'acceptation est
                    // rejouable explicitement.
                    tracing::warn!(error = %e, "attach : resolution magnet echouee");
                    let _ = session
                        .inner
                        .db_arc()
                        .with(|c| dbc::set_attachment_state(c, &aid, "offered"));
                }
            }
        });
        Ok(AttachAccept {
            infohash: ih_hex,
            name: row.name,
        })
    }

    /// Refuse une offre recue (`offered` → `declined` — aucun
    /// telechargement lance, rien ne fuit).
    pub fn attach_decline(&self, attach_id: &[u8]) -> Result<()> {
        let _svc = self.require_messaging()?;
        let db = self.inner.db_arc();
        let row = db
            .with(|c| dbc::get_attachment(c, attach_id))?
            .ok_or(CoreError::InvalidState("attach : offre inconnue"))?;
        if row.role != "recv" || row.state != "offered" {
            return Err(CoreError::InvalidState(
                "attach : offre deja traitee ou non recue",
            ));
        }
        db.with(|c| dbc::set_attachment_state(c, attach_id, "declined"))?;
        Ok(())
    }

    /// Pieces jointes d'une conversation (`conv=None` → toutes).
    pub fn attach_list(&self, conv: Option<[u8; 16]>) -> Result<Vec<dbc::MsgAttachmentRow>> {
        let db = self.inner.db_arc();
        match conv {
            Some(c) => Ok(db.with(|c2| dbc::list_attachments(c2, &c))?),
            None => {
                let convs = db.with(dbc::list_conversations)?;
                let mut out = Vec::new();
                for cv in convs {
                    let conv_id = cv.row.conv_id.clone();
                    out.extend(db.with(|c| dbc::list_attachments(c, &conv_id))?);
                }
                Ok(out)
            }
        }
    }

    /// Intervalle du reaper — derive des TTL messagerie (jamais de
    /// constante en dur) : plus courte des deux TTL / 8, bornee a
    /// [60 s, 1 h] pour un rearmement raisonnable.
    fn attach_reap_interval(&self) -> Duration {
        let Some(svc) = self.messaging() else {
            return Duration::from_secs(3600);
        };
        let cfg = svc.config();
        (cfg.attach_seed_ttl.min(cfg.upload_ttl) / 8)
            .clamp(Duration::from_secs(60), Duration::from_secs(3600))
    }

    /// Balayage d'expiration : offres `seeding` au-dela de
    /// `attach_seed_ttl` → `expired` (+ purge moteur/staging si
    /// `attach_purge_on_expire`) ; dossiers de staging residuels sans
    /// offre active au-dela de `upload_ttl` → supprimes.
    async fn attach_reap_once(&self) {
        let Some(svc) = self.messaging() else {
            return;
        };
        let cfg = svc.config();
        let now = now_secs() as i64;
        let db = self.inner.db_arc();
        let offers = db.with(dbc::list_seeding_offers).unwrap_or_default();
        let mut live: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
        for row in offers {
            if now - row.created_at >= cfg.attach_seed_ttl.as_secs() as i64 {
                let _ = db.with(|c| dbc::set_attachment_state(c, &row.attach_id, "expired"));
                if cfg.attach_purge_on_expire {
                    let ih_hex = onionbit_crypto::hash::to_hex(&row.ih);
                    let _ = self.remove(&ih_hex, true).await;
                    let stage = self
                        .attach_stage_dir()
                        .join(onionbit_crypto::hash::to_hex(&row.attach_id));
                    let _ = std::fs::remove_dir_all(&stage);
                }
            } else {
                live.insert(row.attach_id);
            }
        }
        // La transition `accepted|downloading → done` n'est PAS
        // ici : elle suit `stats.finished` dans la boucle de
        // progression (`spawn_progress_loop` — cadence UI). Le
        // reaper horaire ne traiterait la complétion qu'a retard.
        // Staging residuel : dossier sans offre `seeding` vivante et
        // plus vieux que `upload_ttl` (upload interrompu, offre
        // expiree sans purge, reste d'un crash). Meme discipline pour
        // les uploads `uploads/` — jamais d'offre associee.
        self.reap_stage_root(&self.attach_stage_dir(), cfg.upload_ttl, Some(&live));
        self.reap_stage_root(&self.upload_stage_dir(), cfg.upload_ttl, None);
    }

    /// Purge les sous-dossiers d'une racine de staging plus vieux
    /// que `ttl` — `live` retient les dossiers encore references par
    /// une offre `seeding` (`None` = tout est candidat).
    fn reap_stage_root(
        &self,
        root: &Path,
        ttl: Duration,
        live: Option<&std::collections::HashSet<Vec<u8>>>,
    ) {
        let Ok(rd) = std::fs::read_dir(root) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let aged_out = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age >= ttl);
            if !aged_out {
                continue;
            }
            if let Some(live) = live {
                let attach_id = p
                    .file_name()
                    .and_then(|n| onionbit_crypto::hash::from_hex(&n.to_string_lossy()));
                if attach_id.is_some_and(|id| live.contains(&id)) {
                    continue;
                }
            }
            let _ = std::fs::remove_dir_all(&p);
        }
    }

    /// Tache periodique d'expiration des pieces jointes — demarree
    /// une fois la session `Ready` (meme discipline que
    /// `spawn_restore`).
    pub(crate) fn spawn_attach_reaper(&self) {
        let session = self.clone();
        self.inner
            .asyncio
            .tasks
            .register(Some("CoreSession"), "attach_reaper", None);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(session.attach_reap_interval()).await;
                session.attach_reap_once().await;
            }
        });
    }
}
