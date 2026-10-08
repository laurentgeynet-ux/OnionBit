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

use onionbit_bittorrent::Download;
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
        if name.is_empty()
            || name.len() > cfg.group_name_max_len
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(CoreError::InvalidState(
                "attach : nom de piece jointe hors borne",
            ));
        }
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
    pub async fn attach_accept(
        &self,
        attach_id: &[u8],
        destination: Option<PathBuf>,
        area: Option<StorageArea>,
    ) -> Result<Download> {
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
        let magnet = format!(
            "magnet:?xt=urn:btih:{}",
            onionbit_crypto::hash::to_hex(&row.ih)
        );
        let dl = self
            .add_download_anon_area(&magnet, false, hops, true, dest, area)
            .await?;
        self.inner
            .db_arc()
            .with(|c| onionbit_db::downloads::set_origin(c, &row.ih, "messaging"))?;
        self.inner
            .db_arc()
            .with(|c| dbc::set_attachment_state(c, attach_id, "accepted"))?;
        Ok(dl)
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
        // Staging residuel : dossier sans offre `seeding` vivante et
        // plus vieux que `upload_ttl` (upload interrompu, offre
        // expiree sans purge, reste d'un crash).
        let stage_root = self.attach_stage_dir();
        if let Ok(rd) = std::fs::read_dir(&stage_root) {
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
                    .is_some_and(|age| age >= cfg.upload_ttl);
                if !aged_out {
                    continue;
                }
                let attach_id = p
                    .file_name()
                    .and_then(|n| onionbit_crypto::hash::from_hex(&n.to_string_lossy()));
                if attach_id.is_some_and(|id| live.contains(&id)) {
                    continue;
                }
                let _ = std::fs::remove_dir_all(&p);
            }
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
