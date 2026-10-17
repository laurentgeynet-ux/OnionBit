// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/files` — equivalent de
//! `tribler.core.restapi.file_endpoint` (navigateur de fichiers pour
//! les selecteurs de l'UI ; l'API n'ecoute que sur loopback).

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::error::ApiError;
use crate::state::AppState;

/// Separateur de la plateforme (`os.path.sep` Python).
const SEPARATOR: &str = if cfg!(windows) { "\\" } else { "/" };

/// ADR-0018 : la zone privee n'est pas navigable — les noms de
/// groupes `OBD`/`manifest.obm` n'apportent rien au selecteur et un
/// listing brut exposerait la topologie chiffree. `path` sous
/// `data/private` (ou le spec `@private/…`) → `403`.
fn refuse_private(state: &AppState, path: &std::path::Path) -> Result<(), ApiError> {
    let resolved = state
        .session
        .paths()
        .resolve_input(path)
        .unwrap_or_else(|_| path.to_path_buf());
    let resolved = resolved.canonicalize().unwrap_or(resolved);
    if state
        .session
        .paths()
        .to_portable(&resolved)
        .is_some_and(|s| s == "@private" || s.starts_with("@private/"))
    {
        return Err(ApiError::forbidden(
            "the private storage area is not browsable",
        ));
    }
    Ok(())
}

/// Entree d'un listing de repertoire.
fn entry(path: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "name": path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "path": path.canonicalize().unwrap_or_else(|_| path.to_path_buf()).display().to_string(),
        "dir": path.is_dir(),
    })
}

/// `GET /api/files/browse?path=...&files=1` — liste un repertoire
/// (sous-repertoires d'abord, `..` en tete ; sous Windows, `/`
/// liste les lecteurs logiques).
#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
    /// Repertoire a lister.
    pub path: Option<String>,
    /// Inclure les fichiers (`files=1`, sinon dossiers seuls).
    pub files: Option<String>,
}

pub async fn browse(
    State(state): State<AppState>,
    Query(q): Query<BrowseQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let path = q.path.unwrap_or_default();
    let show_files = q.files.as_deref() == Some("1");

    // Racine : sous Windows, liste des lecteurs.
    if path == "/" && cfg!(windows) {
        let mut drives = Vec::new();
        for letter in b'A'..=b'Z' {
            let d = format!("{}:\\", letter as char);
            if std::path::Path::new(&d).exists() {
                drives.push(serde_json::json!({
                    "name": d, "path": d, "dir": true,
                }));
            }
        }
        return Ok(Json(serde_json::json!({
            "current": "Root", "paths": drives, "separator": SEPARATOR,
        })));
    }

    // Spec portable `@public/…` / `@state/…` : resolu via le layout
    // (`@private/…` est refuse par `refuse_private` — la zone
    // chiffree n'est pas navigable). Sert la page « Zone publique »
    // qui navigue `path=@public`.
    let mut dir = if path.starts_with('@') {
        state
            .session
            .paths()
            .resolve_input(std::path::Path::new(&path))
            .map_err(|e| ApiError::bad_request(format!("invalid path spec {path}: {e}")))?
    } else {
        std::path::PathBuf::from(if path.is_empty() { "." } else { &path })
    };
    refuse_private(&state, &dir)?;
    dir = dir.canonicalize().unwrap_or(dir);
    while !dir.is_dir() {
        if !dir.pop() {
            break;
        }
    }
    // `canonicalize` a pu rentrer dans `data/private` par un chemin
    // public prefixe (junction) — re-verification post-resolution.
    refuse_private(&state, &dir)?;
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!(
            "No directory named {path} exists"
        )));
    }

    let mut results: Vec<serde_json::Value> = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| {
        ApiError::internal(format!("Directory {} not accessible: {e}", dir.display()))
    })?;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() && !show_files {
            continue;
        }
        results.push(entry(&p));
    }
    // Dossiers d'abord (tri Python : `not f["dir"]`).
    results.sort_by_key(|v| !v["dir"].as_bool().unwrap_or(false));

    let parent = dir.parent().map(|p| p.to_path_buf());
    let mut out = vec![serde_json::json!({
        "name": "..",
        "path": parent.map(|p| p.display().to_string()).unwrap_or_else(|| "/".into()),
        "dir": true,
    })];
    out.extend(results);

    Ok(Json(serde_json::json!({
        "current": dir.display().to_string(),
        "paths": out,
        "separator": SEPARATOR,
    })))
}

/// `GET /api/files/list?path=...&recursively=1` — liste les fichiers
/// d'un repertoire (recursif par defaut).
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Repertoire a lister.
    pub path: Option<String>,
    /// `0` = non recursif.
    pub recursively: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let path = q.path.unwrap_or_default();
    let recursive = q.recursively.as_deref() != Some("0");
    let dir = std::path::PathBuf::from(&path);
    refuse_private(&state, &dir)?;
    if !dir.exists() {
        return Err(ApiError::not_found(format!(
            "Directory {path} does not exist"
        )));
    }
    let mut results = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if recursive {
                    stack.push(p);
                }
            } else {
                results.push(serde_json::json!({
                    "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    "path": p.canonicalize().unwrap_or(p.clone()).display().to_string(),
                }));
            }
        }
    }
    Ok(Json(serde_json::json!({
        "current": path,
        "paths": results,
        "separator": SEPARATOR,
    })))
}

/// `GET /api/files/create?path=...` — cree un repertoire (selecteur
/// "nouveau dossier" de l'UI).
#[derive(Debug, Deserialize)]
pub struct CreateQuery {
    /// Repertoire a creer.
    pub path: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Query(q): Query<CreateQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let path = q
        .path
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::bad_request("path parameter missing"))?;
    let dir = std::path::PathBuf::from(&path);
    refuse_private(&state, &dir)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::bad_request(format!("Cannot create {path}: {e}")))?;
    Ok(Json(serde_json::json!({
        "created": true,
        "path": dir.canonicalize().unwrap_or(dir).display().to_string(),
    })))
}

// ----------------------------------------------------------------
// Mutations de l'explorateur « Zone publique » — confinees a
// `data/public` : contrairement a `browse`/`list`/`create` (navigateur
// generique de l'UI, en lecture), ces endpoints modifient le disque
// et n'acceptent donc que des cibles resolues sous `@public`.
// ----------------------------------------------------------------

/// `raw` (spec `@public/…` ou chemin brut) resolu sous la racine
/// publique. La comparaison `to_portable` est lexicale normalisee
/// (verbatim aplati, `..` reduits, casse Windows insensible) — valable
/// aussi pour une cible encore inexistante.
fn resolve_public(state: &AppState, raw: &str) -> Result<PathBuf, ApiError> {
    let p = state
        .session
        .paths()
        .resolve_input(Path::new(raw))
        .map_err(|e| ApiError::bad_request(format!("invalid path spec {raw}: {e}")))?;
    match state.session.paths().to_portable(&p) {
        Some(s) if s == "@public" || s.starts_with("@public/") => Ok(p),
        _ => Err(ApiError::forbidden(
            "explorer mutations are confined to @public",
        )),
    }
}

/// `resolve_public` + refus de la racine elle-meme : on ne supprime ni
/// ne deplace `data/public`.
fn require_public_child(state: &AppState, raw: &str) -> Result<PathBuf, ApiError> {
    let p = resolve_public(state, raw)?;
    if state.session.paths().to_portable(&p).as_deref() == Some("@public") {
        return Err(ApiError::forbidden("the public root cannot be modified"));
    }
    Ok(p)
}

/// Corps `{ "src": "…", "dst": "…" }` des transferts.
#[derive(Debug, Deserialize)]
pub struct TransferBody {
    /// Source — spec `@public/…` (`move`) ou n'importe quel chemin
    /// lisible du device hors `@private` (`copy` = import).
    pub src: String,
    /// Cible complete sous `@public/…` (dossier + nom), pas un
    /// repertoire parent.
    pub dst: String,
}

/// `dst` ne doit pas vivre a l'interieur de `src` (dossier copie ou
/// deplace dans son propre sous-arbre).
fn refuse_self_descent(src: &Path, dst: &Path) -> Result<(), ApiError> {
    let src = src.canonicalize().unwrap_or_else(|_| src.to_path_buf());
    // `dst` peut ne pas exister : on compare sur le parent existant.
    let mut probe = dst.to_path_buf();
    while !probe.exists() && probe.pop() {}
    let probe = probe.canonicalize().unwrap_or(probe);
    if probe.starts_with(&src) {
        return Err(ApiError::bad_request(
            "destination cannot be inside the source",
        ));
    }
    Ok(())
}

/// `dst` doit etre un chemin nouveau dans un parent existant.
fn check_fresh_target(dst: &Path) -> Result<(), ApiError> {
    if dst.exists() {
        return Err(ApiError::conflict(format!(
            "destination already exists: {}",
            dst.display()
        )));
    }
    if dst.parent().is_none_or(|p| !p.is_dir()) {
        return Err(ApiError::not_found("destination directory does not exist"));
    }
    Ok(())
}

/// Copie recursive dossier/fichier (bloquant — `spawn_blocking`).
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<u64> {
    let mut n = 0u64;
    if src.is_dir() {
        std::fs::create_dir(dst)?;
        for e in std::fs::read_dir(src)?.flatten() {
            n += copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
    } else {
        std::fs::copy(src, dst)?;
        n = 1;
    }
    Ok(n)
}

/// `POST /api/files/mkdir` — cree un dossier sous `@public` (le parent
/// doit exister ; `409` si la cible existe deja).
#[derive(Debug, Deserialize)]
pub struct MkdirBody {
    /// Spec `@public/…/nouveau-dossier`.
    pub path: String,
}

pub async fn mkdir(
    State(state): State<AppState>,
    Json(body): Json<MkdirBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = resolve_public(&state, &body.path)?;
    if dir.exists() {
        return Err(ApiError::conflict("target already exists"));
    }
    std::fs::create_dir(&dir)
        .map_err(|e| ApiError::bad_request(format!("Cannot create {}: {e}", dir.display())))?;
    Ok(Json(serde_json::json!({ "created": true })))
}

/// `POST /api/files/delete` — supprime fichiers/dossiers sous
/// `@public` (recursif ; pas de corbeille — l'UI confirme avant).
#[derive(Debug, Deserialize)]
pub struct DeleteBody {
    /// Specs `@public/…` a supprimer (racine refusee).
    pub paths: Vec<String>,
}

pub async fn delete_entries(
    State(state): State<AppState>,
    Json(body): Json<DeleteBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.paths.is_empty() {
        return Err(ApiError::bad_request("paths parameter is empty"));
    }
    let mut resolved = Vec::with_capacity(body.paths.len());
    for raw in &body.paths {
        let p = require_public_child(&state, raw)?;
        if !p.exists() {
            return Err(ApiError::not_found(format!("no such entry: {raw}")));
        }
        resolved.push(p);
    }
    let deleted = tokio::task::spawn_blocking(move || {
        let mut n = 0u64;
        for p in &resolved {
            if p.is_dir() {
                std::fs::remove_dir_all(p)?;
            } else {
                std::fs::remove_file(p)?;
            }
            n += 1;
        }
        Ok::<u64, std::io::Error>(n)
    })
    .await
    .map_err(|e| ApiError::internal(format!("delete task failed: {e}")))?
    .map_err(|e| ApiError::internal(format!("delete failed: {e}")))?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

/// `POST /api/files/move` — deplace/renomme sous `@public` (rename
/// intra-volume ; `dst` = chemin complet de la cible).
pub async fn move_entry(
    State(state): State<AppState>,
    Json(body): Json<TransferBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let src = require_public_child(&state, &body.src)?;
    if !src.exists() {
        return Err(ApiError::not_found(format!("no such entry: {}", body.src)));
    }
    let dst = resolve_public(&state, &body.dst)?;
    refuse_self_descent(&src, &dst)?;
    check_fresh_target(&dst)?;
    std::fs::rename(&src, &dst).map_err(|e| ApiError::internal(format!("move failed: {e}")))?;
    Ok(Json(serde_json::json!({ "moved": true })))
}

/// `POST /api/files/copy` — copie `src` vers `dst` sous `@public`.
/// `src` peut etre un spec `@public/…` (copie interne) ou un chemin
/// local du device hors `@private` (import depuis l'explorateur OS) ;
/// `dst` reste confine a la zone publique. Dossiers copies
/// recursivement.
pub async fn copy_entry(
    State(state): State<AppState>,
    Json(body): Json<TransferBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let src = state
        .session
        .paths()
        .resolve_input(Path::new(&body.src))
        .map_err(|e| ApiError::bad_request(format!("invalid path spec {}: {e}", body.src)))?;
    refuse_private(&state, &src)?;
    if !src.exists() {
        return Err(ApiError::not_found(format!("no such entry: {}", body.src)));
    }
    let dst = resolve_public(&state, &body.dst)?;
    refuse_self_descent(&src, &dst)?;
    check_fresh_target(&dst)?;
    let copied = tokio::task::spawn_blocking(move || copy_tree(&src, &dst))
        .await
        .map_err(|e| ApiError::internal(format!("copy task failed: {e}")))?
        .map_err(|e| ApiError::internal(format!("copy failed: {e}")))?;
    Ok(Json(serde_json::json!({ "copied": copied })))
}
