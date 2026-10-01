// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers `/api/files` — equivalent de
//! `tribler.core.restapi.file_endpoint` (navigateur de fichiers pour
//! les selecteurs de l'UI ; l'API n'ecoute que sur loopback).

use axum::extract::Query;
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;

/// Separateur de la plateforme (`os.path.sep` Python).
const SEPARATOR: &str = if cfg!(windows) { "\\" } else { "/" };

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

pub async fn browse(Query(q): Query<BrowseQuery>) -> Result<Json<serde_json::Value>, ApiError> {
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

    // Remonter jusqu'a un repertoire existant (comportement Python).
    let mut dir = std::path::PathBuf::from(if path.is_empty() { "." } else { &path });
    dir = dir.canonicalize().unwrap_or(dir);
    while !dir.is_dir() {
        if !dir.pop() {
            break;
        }
    }
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

pub async fn list(Query(q): Query<ListQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let path = q.path.unwrap_or_default();
    let recursive = q.recursively.as_deref() != Some("0");
    let dir = std::path::PathBuf::from(&path);
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

pub async fn create(Query(q): Query<CreateQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let path = q
        .path
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::bad_request("path parameter missing"))?;
    let dir = std::path::PathBuf::from(&path);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::bad_request(format!("Cannot create {path}: {e}")))?;
    Ok(Json(serde_json::json!({
        "created": true,
        "path": dir.canonicalize().unwrap_or(dir).display().to_string(),
    })))
}
