//! Handlers `/api/downloads` — equivalent de
//! `tribler.core.libtorrent.restapi.downloads_endpoint`.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use crate::dto::DownloadInfo;
use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/downloads` — liste tous les telechargements.
///
/// Flags Python implementes : `infohash`/`excluded` (filtres),
/// `get_peers` (stats par pair), `get_pieces` (bitfield base64) et
/// `get_availability` — uniquement quand le parametre vaut `"1"`,
/// exactement comme `params.get("get_peers", "0") == "1"` Python.
pub async fn get_downloads(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<DownloadQuery>,
) -> Json<serde_json::Value> {
    let hops_map = state.session.anon_hops_map();
    let defaults = state.session.config().download_defaults.clone();
    let want_peers = params.get_peers.as_deref() == Some("1");
    let want_pieces = params.get_pieces.as_deref() == Some("1");
    let want_availability = params.get_availability.as_deref() == Some("1");
    let downloads: Vec<serde_json::Value> = state
        .session
        .downloads()
        .iter()
        .filter(|s| {
            params
                .infohash
                .as_deref()
                .map(|ih| ih.eq_ignore_ascii_case(&s.info_hash))
                .unwrap_or(true)
        })
        .filter(|s| {
            params
                .excluded
                .as_deref()
                .map(|ih| !ih.eq_ignore_ascii_case(&s.info_hash))
                .unwrap_or(true)
        })
        .map(|s| {
            let mut info = DownloadInfo::from_stats(s);
            let dl = state.session.find_download(&s.info_hash);
            let ih_bytes = tribler_crypto::hash::from_hex(&s.info_hash);
            let row = ih_bytes.as_ref().and_then(|ih| {
                state
                    .session
                    .db()
                    .with(|c| tribler_db::downloads::get(c, ih))
                    .ok()
                    .flatten()
            });
            // Reglages persistes (`DownloadConfig` checkpointe
            // Python) : safe_seeding, limites, ratio, queue…
            if let Some(r) = &row {
                info.hops = r.anon_hops.max(0) as u32;
                info.safe_seeding = r.safe_seeding;
                info.user_stopped = r.user_stopped;
                info.upload_limit = u64::try_from(r.upload_limit).unwrap_or(0);
                info.download_limit = u64::try_from(r.download_limit).unwrap_or(0);
                // `config.get_seeding_ratio()` Python : individuel,
                // sinon defaut `download_defaults` en mode `ratio`.
                info.seeding_ratio = r.seeding_ratio.unwrap_or(defaults.seeding_ratio);
                info.queue_position = r.queue_position;
                info.auto_managed = r.auto_managed;
                info.completed_dir = r.completed_dir.clone().unwrap_or_default();
                info.time_added = r.added_on;
                info.time_finished = r.time_finished;
                if info.destination.is_empty() {
                    info.destination = r.output_dir.clone();
                }
            }
            let hops = hops_map.get(&s.info_hash).copied().unwrap_or(0);
            info.hops = hops.max(info.hops);
            info.anon_download = info.hops > 0;
            if let Some(dl) = &dl {
                info.destination = dl.output_folder().display().to_string();
                info.trackers = crate::handlers::downloads_extra::trackers_json(
                    dl.trackers(),
                    state.session.engine().config().enable_dht,
                );
                if let Some(n) = dl.total_pieces() {
                    info.total_pieces = n as usize;
                }
            }
            // Sante scrapee par le torrent checker (`num_seeds`/
            // `num_peers` = max(lt, scraped) en Python ; librqbit
            // n'expose pas les compteurs swarm lt — on retient le
            // scrape, coherent avec `metadata/torrents/.../health`).
            if let Some(ih) = &ih_bytes {
                if let Ok(Some(ts)) = state
                    .session
                    .db()
                    .with(|c| tribler_db::health::get_torrent_state(c, ih))
                {
                    info.num_seeds = ts.seeders.max(0) as u32;
                    info.num_peers = info.num_peers.max(ts.leechers.max(0) as u32);
                }
            }
            let mut v = serde_json::to_value(&info).unwrap_or_else(|_| serde_json::json!({}));
            if want_peers {
                v["peers"] = peers_json(&state, &s.info_hash);
            }
            if want_pieces {
                v["pieces"] = serde_json::Value::String(
                    state
                        .session
                        .have_pieces_base64(&s.info_hash)
                        .unwrap_or_default(),
                );
            }
            if want_availability {
                // `state.get_availability()` Python fusionne les
                // bitfields des pairs — librqbit ne les expose pas ;
                // approximation documentee : nombre de seeds
                // completes connus (scrape checker).
                v["availability"] = serde_json::json!(info.num_seeds as f64);
            }
            v
        })
        .collect();
    Json(serde_json::json!({
        "downloads": downloads,
        // `checkpoints` : champ Python emis pour compat. `clierrors`
        // = taille de la file d'erreurs CLI non lues (comme Python).
        "checkpoints": { "total": downloads.len(), "loaded": downloads.len(), "all_loaded": true },
        "clierrors": state.unhandled_cli.lock().unwrap().len(),
    }))
}

/// `peers` du `get_peer_list` Python (`include_have=False`) — memes
/// cles ; les champs que librqbit n'expose pas (flags choke/
/// interested par pair, dht/pex/lsd, peer-id) restent aux defauts.
fn peers_json(state: &AppState, infohash: &str) -> serde_json::Value {
    let peers = state.session.peer_stats(infohash).unwrap_or_default();
    serde_json::Value::Array(
        peers
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": "",
                    "extended_version": p.client_name.clone().unwrap_or_else(|| "unknown".into()),
                    "ip": p.ip,
                    "port": p.port,
                    "optimistic": false,
                    "direction": if p.incoming { "L" } else { "R" },
                    "uprate": 0,
                    "uinterested": false,
                    "uchoked": false,
                    "uhasqueries": false,
                    "uflushed": false,
                    "downrate": 0,
                    "dinterested": false,
                    "dchoked": false,
                    "snubbed": false,
                    "utotal": p.uploaded_bytes,
                    "dtotal": p.downloaded_bytes,
                    "completed": 0.0,
                    "speed": 0,
                    "connection_type": p.connection_kind.clone().unwrap_or_default(),
                    "seed": false,
                    "upload_only": false,
                    "from_dht": false,
                    "from_pex": false,
                    "from_lsd": false,
                })
            })
            .collect(),
    )
}

/// `GET /api/downloads/clierrors` — vide la file des erreurs CLI
/// (`get_unhandled_cli` Python : `{"errors": [...]}`, drain).
pub async fn get_cli_errors(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut log = state.unhandled_cli.lock().unwrap();
    let errors: Vec<String> = log.drain(..).collect();
    Json(serde_json::json!({ "errors": errors }))
}

/// Query params acceptes par `GET /api/downloads`.
#[derive(Debug, Default, Deserialize)]
pub struct DownloadQuery {
    /// Filtre sur un info-hash precis.
    pub infohash: Option<String>,
    /// Exclut un info-hash.
    pub excluded: Option<String>,
    /// Compat (non implemente).
    pub get_peers: Option<String>,
    /// Compat (non implemente).
    pub get_pieces: Option<String>,
    /// Compat (non implemente).
    pub get_availability: Option<String>,
}

/// Query params acceptes par `PUT /api/downloads` et `GET /api/downloads`.
#[derive(Debug, Default, Deserialize)]
pub struct AddDownloadQuery {
    /// Magnet ou URI http(s) pointant un `.torrent`.
    pub uri: Option<String>,
    /// Chemin local d'un fichier `.torrent` sur le disque du daemon.
    pub torrent: Option<String>,
    /// Repertoire de destination (defaut : config du daemon).
    pub destination: Option<String>,
    /// Nombre de sauts anonymes (0 = telechargement direct).
    pub anon_hops: Option<u32>,
    /// Seeding anonyme — obligatoire si `anon_hops > 0` (Python).
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub safe_seeding: Option<bool>,
    /// Demarrer en pause.
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub paused: Option<bool>,
    /// Requete emise depuis un CLI.
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub cli: Option<bool>,
}

/// Deserialise un booleen optionnel souple (accepte true/false, "true"/"false", "1"/"0").
fn deserialize_optional_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct OptionalBoolVisitor;

    impl<'de> serde::de::Visitor<'de> for OptionalBoolVisitor {
        type Value = Option<bool>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter
                .write_str("un booleen (true/false) ou chaine (\"true\"/\"false\"/\"1\"/\"0\")")
        }

        fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            match v.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Ok(Some(true)),
                "false" | "0" | "no" => Ok(Some(false)),
                "" => Ok(None),
                other => Err(E::custom(format!("valeur booleenne invalide: {other}"))),
            }
        }

        fn visit_none<E>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_any(self)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_option(OptionalBoolVisitor)
}

/// Corps de `PUT /api/downloads`.
#[derive(Debug, Default, Deserialize)]
pub struct AddDownloadRequest {
    /// Magnet ou URI http(s) pointant un `.torrent`.
    pub uri: Option<String>,
    /// Chemin local d'un fichier `.torrent` sur le disque du daemon.
    pub torrent: Option<String>,
    /// Repertoire de destination (defaut : config du daemon).
    pub destination: Option<String>,
    /// Nombre de sauts anonymes (0 = telechargement direct).
    /// Exige `safe_seeding` et la stack IPv8 avec anonymat actif.
    pub anon_hops: Option<u32>,
    /// Seeding anonyme — obligatoire si `anon_hops > 0` (Python).
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub safe_seeding: Option<bool>,
    /// Demarrer en pause.
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub paused: Option<bool>,
    /// Requete emise depuis un CLI : les erreurs sont journalisees
    /// dans la file `clierrors` (parametre `cli` Python).
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub cli: Option<bool>,
}

/// Enregistre l'erreur dans la file CLI si `cli` est vrai, comme
/// `_add_err` Python, puis la retourne au client.
fn add_err(state: &AppState, msg: String, cli: bool) -> ApiError {
    if cli {
        state.push_cli_error(msg.clone());
    }
    ApiError::bad_request(msg)
}

/// `PUT /api/downloads` — ajoute un telechargement.
///
/// Parite avec Python (`tribler.core.libtorrent.restapi.downloads_endpoint`) :
/// 1. Si `Content-Type` est `applications/x-bittorrent` (ou `application/x-bittorrent` /
///    `application/octet-stream` ou charge bencode detectee), le corps contient les
///    octets bruts du fichier `.torrent` et les parametres sont lus depuis la query string.
/// 2. Sinon, le corps est interprete en JSON (`AddDownloadRequest`), enrichi par les
///    eventuels query params.
///
/// Fidele au Python : `anon_hops > 0` sans `safe_seeding` est refuse,
/// et le telechargement part sur la lane anonyme correspondante.
pub async fn add_download(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(query): axum::extract::Query<AddDownloadQuery>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    let is_torrent_mime =
        content_type.contains("bittorrent") || content_type.contains("octet-stream");

    // Detection heuristique : un fichier .torrent bencode commence toujours par 'd'
    // (dictionnaire racine bencode) et n'est pas un objet JSON (qui commence par '{').
    let is_raw_torrent = is_torrent_mime
        || (!body.is_empty()
            && body[0] == b'd'
            && !body.starts_with(b"{\"")
            && !body.starts_with(b"{"));

    if is_raw_torrent {
        let cli = query.cli.unwrap_or(false);
        let hops = query.anon_hops.unwrap_or(0);
        let safe_seeding = query.safe_seeding.unwrap_or(false);
        let paused = query.paused.unwrap_or(false);

        if hops > 0 && !safe_seeding {
            return Err(add_err(
                &state,
                "Cannot set anonymous download without safe seeding enabled".into(),
                cli,
            ));
        }

        if body.is_empty() {
            return Err(add_err(
                &state,
                "corrupt torrent file (empty body)".into(),
                cli,
            ));
        }

        let dl = state
            .session
            .add_torrent_bytes_anon(body.to_vec(), paused, hops, safe_seeding)
            .await
            .map_err(|e| match &e {
                tribler_core::CoreError::Format(_) => {
                    add_err(&state, "corrupt torrent file".into(), cli)
                }
                _ => add_err(&state, e.to_string(), cli),
            })?;

        return Ok(Json(serde_json::json!({
            "started": true,
            "infohash": dl.info_hash_hex(),
            "name": dl.name().unwrap_or_default(),
        })));
    }

    // Sinon : requete JSON classique.
    let req: AddDownloadRequest = if body.is_empty() {
        AddDownloadRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::bad_request(format!("invalid JSON: {e}")))?
    };

    let cli = req.cli.or(query.cli).unwrap_or(false);
    let hops = req.anon_hops.or(query.anon_hops).unwrap_or(0);
    let safe_seeding = req.safe_seeding.or(query.safe_seeding).unwrap_or(false);
    let paused = req.paused.or(query.paused).unwrap_or(false);
    let uri = req.uri.or(query.uri);
    let torrent = req.torrent.or(query.torrent);

    if hops > 0 && !safe_seeding {
        return Err(add_err(
            &state,
            "Cannot set anonymous download without safe seeding enabled".into(),
            cli,
        ));
    }

    // `ask_download_settings` Python : `uri` + `cli` + option active ->
    // `ask_add_download` au GUI au lieu d'ajouter directement.
    if let Some(uri) = &uri {
        let ask = state
            .daemon_config
            .lock()
            .unwrap()
            .libtorrent
            .ask_download_settings;
        if cli && ask {
            state
                .session
                .notifier()
                .notify(tribler_core::Notification::AskAddDownload { uri: uri.clone() });
            return Ok(Json(serde_json::json!({
                "started": false,
                "infohash": "",
            })));
        }
    }

    let dl = if let Some(uri) = &uri {
        state
            .session
            .add_download_anon(uri, paused, hops, safe_seeding)
            .await
            .map_err(|e| add_err(&state, e.to_string(), cli))?
    } else if let Some(path) = &torrent {
        let bytes = std::fs::read(path)
            .map_err(|e| add_err(&state, format!("lecture du .torrent: {e}"), cli))?;
        state
            .session
            .add_torrent_bytes_anon(bytes, paused, hops, safe_seeding)
            .await
            .map_err(|e| add_err(&state, e.to_string(), cli))?
    } else {
        return Err(add_err(&state, "uri parameter missing".into(), cli));
    };

    Ok(Json(serde_json::json!({
        "started": true,
        "infohash": dl.info_hash_hex(),
        "name": dl.name().unwrap_or_default(),
    })))
}

/// Corps de `DELETE /api/downloads/{infohash}`.
#[derive(Debug, Deserialize)]
pub struct RemoveDownloadRequest {
    /// Supprimer aussi les donnees sur disque.
    pub remove_data: Option<bool>,
}

/// `DELETE /api/downloads/{infohash}` — supprime un telechargement.
pub async fn delete_download(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<RemoveDownloadRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .session
        .remove(&infohash, req.remove_data.unwrap_or(false))
        .await?;
    Ok(Json(serde_json::json!({
        "removed": true,
        "infohash": infohash,
    })))
}

/// Corps de `PATCH /api/downloads/{infohash}` — tous les champs du
/// `UpdateDownloadRequest` Python (`downloads_endpoint.py`).
///
/// `auto_managed`, `file_priority` et `seeding_ratio_default` sont
/// type `serde_json::Value` pour reproduire les validations Python
/// (`isinstance(..., bool)`, unpack de la paire, veracite) plutot que
/// le rejet serde par defaut.
#[derive(Debug, Deserialize)]
pub struct UpdateDownloadRequest {
    /// `"resume"`, `"stop"`, `"recheck"` ou `"move_storage"`.
    pub state: Option<String>,
    /// Nouveau nombre de sauts anonymes — doit etre le seul parametre
    /// de la requete (comme en Python).
    pub anon_hops: Option<u32>,
    /// Indices des fichiers a telecharger (`set_selected_files`).
    pub selected_files: Option<Vec<i64>>,
    /// Paire `[file_index, priority]` (priorite 0..=7).
    pub file_priority: Option<serde_json::Value>,
    /// Limite d'upload en octets/s (0 ignore, comme le walrus Python).
    pub upload_limit: Option<i64>,
    /// Limite de download en octets/s.
    pub download_limit: Option<i64>,
    /// Ratio de seed individuel.
    pub seeding_ratio: Option<f64>,
    /// Verite → reinitialise le ratio individuel au defaut.
    pub seeding_ratio_default: Option<serde_json::Value>,
    /// `queue_up`/`queue_top`/`queue_down`/`queue_bottom`.
    pub queue_position: Option<String>,
    /// Flag auto-managed (doit etre un booleen, comme Python).
    pub auto_managed: Option<serde_json::Value>,
    /// `state=move_storage` : dossier de destination.
    pub dest_dir: Option<String>,
    /// `state=move_storage` : dossier des fichiers termines.
    pub completed_dir: Option<String>,
    /// Cles inconnues — comptabilisees dans la regle d'exclusivite
    /// `anon_hops` (Python compte `len(parameters)` brut).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl UpdateDownloadRequest {
    /// Nombre de parametres presents (equivalent `len(parameters)`).
    fn param_count(&self) -> usize {
        [
            self.state.is_some(),
            self.anon_hops.is_some(),
            self.selected_files.is_some(),
            self.file_priority.is_some(),
            self.upload_limit.is_some(),
            self.download_limit.is_some(),
            self.seeding_ratio.is_some(),
            self.seeding_ratio_default.is_some(),
            self.queue_position.is_some(),
            self.auto_managed.is_some(),
            self.dest_dir.is_some(),
            self.completed_dir.is_some(),
        ]
        .iter()
        .filter(|p| **p)
        .count()
            + self.extra.len()
    }
}

/// Veracite Python d'une valeur JSON (`if parameters.get(k)`).
fn truthy(v: &serde_json::Value) -> bool {
    use serde_json::Value;
    !matches!(v, Value::Null | Value::Bool(false))
        && !matches!(v, Value::Number(n) if n.as_i64() == Some(0))
        && !matches!(v, Value::String(s) if s.is_empty())
}

/// Traduit les erreurs de validation metier (`CoreError::State`,
/// messages alignes sur le Python : "index out of range", "Target
/// directory ...") en 400 — comme les `HTTP_BAD_REQUEST` du endpoint.
fn state_err(e: tribler_core::CoreError) -> ApiError {
    match e {
        tribler_core::CoreError::State(m) => ApiError::bad_request(m),
        tribler_core::CoreError::InvalidState(m) => ApiError::bad_request(m),
        other => ApiError::from(other),
    }
}

/// `PATCH /api/downloads/{infohash}` — modifie l'etat et les
/// reglages d'un telechargement (`update_download` Python).
pub async fn update_download(
    State(state): State<AppState>,
    Path(infohash): Path<String>,
    Json(req): Json<UpdateDownloadRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dl = state
        .session
        .find_download_hex(&infohash)
        .ok_or_else(|| ApiError::not_found(format!("this download does not exist: {infohash}")))?;
    let ih_hex = dl.info_hash_hex();

    // `anon_hops` doit etre le seul parametre (comptage brut des
    // cles du corps, cles inconnues comprises — `len(parameters)`).
    if req.anon_hops.is_some() && req.param_count() > 1 {
        return Err(ApiError::bad_request(
            "anon_hops must be the only parameter in this request",
        ));
    }
    if let Some(hops) = req.anon_hops {
        state
            .session
            .update_hops(&infohash, hops)
            .await
            .map_err(invalid_state_as_bad_request)?;
        return Ok(Json(serde_json::json!({
            "modified": true,
            "infohash": ih_hex,
        })));
    }

    if let Some(files) = &req.selected_files {
        state
            .session
            .set_selected_files(&infohash, files)
            .await
            .map_err(state_err)?;
    }

    if let Some(fp) = &req.file_priority {
        // `file_index, priority = parameters["file_priority"]` —
        // l'unpack Python leve sur toute valeur non paire (500
        // handled exception) : on renvoie le meme format.
        let pair = fp
            .as_array()
            .and_then(|a| {
                match (
                    a.first().and_then(|v| v.as_i64()),
                    a.get(1).and_then(|v| v.as_i64()),
                ) {
                    (Some(i), Some(p)) if a.len() == 2 => Some((i, p)),
                    _ => None,
                }
            })
            .ok_or_else(|| {
                ApiError::internal_handled(
                    "TypeError: file_priority must be [file_index, priority]",
                )
            })?;
        state
            .session
            .set_file_priority(&infohash, pair.0, pair.1)
            .map_err(state_err)?;
    }

    // Walrus Python : `if upload_limit := ...` — 0 est falsy, ignore.
    if req.upload_limit.is_some_and(|v| v != 0) || req.download_limit.is_some_and(|v| v != 0) {
        state
            .session
            .set_rate_limits(
                &infohash,
                req.upload_limit.filter(|v| *v != 0),
                req.download_limit.filter(|v| *v != 0),
            )
            .map_err(state_err)?;
    }

    if let Some(ratio) = req.seeding_ratio {
        state
            .session
            .set_seeding_ratio(&infohash, Some(ratio))
            .map_err(state_err)?;
    }
    if req.seeding_ratio_default.as_ref().is_some_and(truthy) {
        state
            .session
            .set_seeding_ratio(&infohash, None)
            .map_err(state_err)?;
    }

    if let Some(op) = req.queue_position.as_deref() {
        let op = match op {
            "queue_up" => tribler_core::QueueOp::Up,
            "queue_top" => tribler_core::QueueOp::Top,
            "queue_down" => tribler_core::QueueOp::Down,
            "queue_bottom" => tribler_core::QueueOp::Bottom,
            _ => {
                return Err(ApiError::bad_request("invalid value for queue_position"));
            }
        };
        state
            .session
            .move_in_queue(&infohash, op)
            .map_err(state_err)?;
    }

    if let Some(v) = &req.auto_managed {
        // `isinstance(parameters["auto_managed"], bool)` Python.
        let Some(enabled) = v.as_bool() else {
            return Err(ApiError::bad_request("invalid value for auto_managed"));
        };
        state
            .session
            .set_auto_managed(&infohash, enabled)
            .map_err(state_err)?;
    }

    let mut modified = true;
    if let Some(op) = req.state.as_deref() {
        match op {
            "resume" => {
                state.session.resume(&infohash).await?;
                state
                    .session
                    .set_stopped_flag(&infohash, false)
                    .map_err(state_err)?;
            }
            "stop" => {
                state.session.pause(&infohash).await?;
                state
                    .session
                    .set_stopped_flag(&infohash, true)
                    .map_err(state_err)?;
            }
            "recheck" => {
                state
                    .session
                    .recheck(&infohash)
                    .await
                    .map_err(invalid_state_as_bad_request)?;
            }
            "move_storage" => {
                // `parameters["dest_dir"]` Python : KeyError → 500
                // `handled` (`return_handled_exception`).
                let Some(dest) = req.dest_dir.as_deref() else {
                    return Err(ApiError::internal_handled("KeyError: 'dest_dir'"));
                };
                modified = state
                    .session
                    .move_storage(
                        &infohash,
                        std::path::Path::new(dest),
                        req.completed_dir.as_deref().map(std::path::Path::new),
                    )
                    .await
                    .map_err(state_err)?;
            }
            _ => {
                return Err(ApiError::bad_request("unknown state parameter"));
            }
        }
    }

    Ok(Json(serde_json::json!({
        "modified": modified,
        "infohash": ih_hex,
    })))
}

/// Les erreurs metier `InvalidState` des chemins anonymes (stack ipv8
/// inactive, lane indisponible) sont des erreurs de requete, pas des
/// 404 : les ressources introuvables sont testees explicitement avant.
fn invalid_state_as_bad_request(e: tribler_core::CoreError) -> ApiError {
    match e {
        tribler_core::CoreError::InvalidState(m) => ApiError::bad_request(m),
        other => ApiError::from(other),
    }
}
