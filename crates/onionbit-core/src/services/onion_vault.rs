// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Coffre servi en `.onion` (ADR-0029 etape 127) — endpoint HTTP
//! minimal sur le service `.onion` **coffre** dedie (adresse
//! `tor-hs-vault/v1`, distincte de l'identite) en restricted-
//! discovery : seul un appareil de la meme identite peut meme
//! decouvrir le service.
//!
//! - **Filaire** : deux chemins `GET` — `/vault` rend le blob
//!   `OBV1` courant (contacts + liaisons `.onion`, deja chiffres
//!   pour l'identite), `/manifest` rend `manifest.obm` brut (le
//!   codec OBM porte deja magic/version/AEAD — le serveur ne fait
//!   que lire le fichier). Reponses bornees : `404` hors de ces
//!   chemins, `405` hors `GET`, `204` si la ressource n'existe pas
//!   encore.
//! - **Confidentialite** : le contenu servi est deja chiffre —
//!   l'autorisation client est une barriere de *decouverte* (DoS),
//!   pas le secret (un blob OBV1/manifeste vole reste illisible
//!   sans la cle maitresse). Aucun secret n'est jamais servi en
//!   clair, aucun n'apparait dans les logs.
//! - **Repli** : le pull `.onion` cote client est un chemin
//!   supplementaire — les ponts `pull_store` IPv8 restent le
//!   repli quand le service maison est hors ligne (jamais de
//!   point dur unique).

use std::sync::Arc;

/// Port virtuel `BEGIN` du coffre — 80 : HTTP standard, espace de
/// ports independant du service d'identite (service dedie, etape
/// 127).
pub const ONION_VAULT_PORT: u16 = 80;

/// Producteur de blob du coffre — rend `None` quand la ressource
/// n'existe pas (pas de manifeste prive, coffre non exportable).
/// Fonction-object : le handler reste testable sans services.
pub type VaultBlobFn = Arc<dyn Fn() -> Option<Vec<u8>> + Send + Sync>;

/// Chemins servis : `/vault` (blob `OBV1` du coffre d'amis) et
/// `/manifest` (`manifest.obm` de la zone privee). `Err(status)` =
/// reponse a renvoyer.
pub fn parse_vault_request(head: &str) -> Result<VaultPath, u16> {
    let line = head.lines().next().ok_or(400u16)?.trim_end();
    let mut parts = line.split_whitespace();
    let (method, target) = match (parts.next(), parts.next()) {
        (Some(m), Some(t)) => (m, t),
        _ => return Err(400),
    };
    let path = target.split('?').next().unwrap_or(target);
    let which = match path.trim_end_matches('/') {
        "/vault" => VaultPath::Vault,
        "/manifest" => VaultPath::Manifest,
        _ => return Err(404),
    };
    if method != "GET" {
        return Err(405);
    }
    Ok(which)
}

/// Ressource demandee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultPath {
    /// `OBV1` — coffre contacts/liaisons `.onion`.
    Vault,
    /// `manifest.obm` — catalogue chiffre de la zone privee.
    Manifest,
}

/// Sert une connexion du port coffre : requete bornee (meme
/// lecture d'en-tete que le catalogue), reponse = le blob courant
/// de la source. `204 No Content` quand la ressource n'existe pas.
#[cfg(feature = "hs")]
pub async fn serve_vault_stream<S>(mut stream: S, vault: VaultBlobFn, manifest: VaultBlobFn)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    use tokio::io::AsyncWriteExt;

    use super::onion_channels::{http_response, read_request_head};

    let Some(head) = read_request_head(&mut stream).await else {
        return;
    };
    let head_str = String::from_utf8_lossy(&head);
    let source = match parse_vault_request(&head_str) {
        Ok(VaultPath::Vault) => vault,
        Ok(VaultPath::Manifest) => manifest,
        Err(status) => {
            let reason = match status {
                404 => "Not Found",
                405 => "Method Not Allowed",
                _ => "Bad Request",
            };
            let _ = stream.write_all(&http_response(status, reason, &[])).await;
            return;
        }
    };
    match source() {
        Some(body) => {
            let _ = stream.write_all(&http_response(200, "OK", &body)).await;
        }
        None => {
            let _ = stream
                .write_all(&http_response(204, "No Content", &[]))
                .await;
        }
    }
}

/// Handler du port virtuel coffre (etape 127) — un flux par
/// requete dans sa propre tache.
#[cfg(feature = "hs")]
pub struct VaultOnionHandler {
    vault: VaultBlobFn,
    manifest: VaultBlobFn,
}

#[cfg(feature = "hs")]
impl VaultOnionHandler {
    pub fn new(vault: VaultBlobFn, manifest: VaultBlobFn) -> Self {
        Self { vault, manifest }
    }
}

#[cfg(feature = "hs")]
impl onionbit_tor::OnionLinkHandler for VaultOnionHandler {
    fn on_stream(&self, stream: Box<dyn onionbit_tor::OnionIo>) {
        let (vault, manifest) = (self.vault.clone(), self.manifest.clone());
        tokio::spawn(async move {
            serve_vault_stream(stream, vault, manifest).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vault_chemins() {
        assert_eq!(
            parse_vault_request("GET /vault HTTP/1.1\r\n\r\n").unwrap(),
            VaultPath::Vault
        );
        assert_eq!(
            parse_vault_request("GET /manifest?x=1 HTTP/1.1\r\n\r\n").unwrap(),
            VaultPath::Manifest
        );
        assert_eq!(
            parse_vault_request("GET /autre HTTP/1.1\r\n\r\n").unwrap_err(),
            404
        );
        assert_eq!(
            parse_vault_request("POST /vault HTTP/1.1\r\n\r\n").unwrap_err(),
            405
        );
        assert_eq!(parse_vault_request("garbage\r\n\r\n").unwrap_err(), 400);
    }

    #[cfg(feature = "hs")]
    #[tokio::test]
    async fn serve_vault_round_trip() {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let vault: VaultBlobFn = Arc::new(|| Some(b"OBV1\x01test".to_vec()));
        let manifest: VaultBlobFn = Arc::new(|| None);
        tokio::spawn(serve_vault_stream(server, vault, manifest));
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        client
            .write_all(b"GET /vault HTTP/1.1\r\nHost: x.onion\r\n\r\n")
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "reponse : {text}");
        assert!(resp.windows(9).any(|w| w == b"OBV1\x01test"));
    }

    #[cfg(feature = "hs")]
    #[tokio::test]
    async fn serve_manifest_absent_204() {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let none: VaultBlobFn = Arc::new(|| None);
        tokio::spawn(serve_vault_stream(server, none.clone(), none));
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        client
            .write_all(b"GET /manifest HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        assert!(String::from_utf8_lossy(&resp).starts_with("HTTP/1.1 204"));
    }
}
