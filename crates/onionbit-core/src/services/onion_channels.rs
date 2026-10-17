// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Catalogue de canaux servi en `.onion` (ADR-0029 etape 126) —
//! endpoint HTTP minimal sur le port virtuel 80 du service
//! d'identite.
//!
//! - **Filaire** : `GET /channels/<pk64 hex>[?origin_id=N&first=F&last=L]`
//!   → `200` + corps = UNE frame LZ4 d'entrees `mdblob` signees —
//!   exactement le blob qu'un `SelectResponse` du plan OnionBit
//!   porte. Le client l'integre par `process_select_response` :
//!   la verification de signature (`public_key == channel_pk`)
//!   est independante du transport — un catalogue Tor n'est pas
//!   plus credible qu'un catalogue IPv8, et inversement.
//! - **Service** : un `ChannelCatalogHandler` souscrit au port
//!   virtuel 80 — port standard HTTP : le service ressemble a un
//!   site onion ordinaire (empreinte) et le catalogue est lisible
//!   par n'importe quel client HTTP/Tor. Les autres chemins
//!   repondent `404`, les methodes autres que `GET` `405`.
//! - **Portee** : servi des que le service `.onion` d'identite
//!   tourne et qu'une stack IPv8 existe (le createur sert son
//!   canal ; un abonne sert ce qu'il a synchronise — miroir
//!   opt-in par l'activation du service). La requete reste
//!   bornee (`first`/`last` plafonnes cote select, en-tete ≤
//!   `MAX_HEAD_BYTES`).

use std::sync::Arc;

/// Port virtuel `BEGIN` du catalogue — 80 : HTTP standard sur un
/// service `.onion` (indiscernable d'un site Tor ordinaire).
pub const ONION_CHANNEL_PORT: u16 = 80;

/// Plafond de la zone en-tete de requete lue depuis le flux —
/// notre endpoint n'a besoin que de la ligne de requete, mais on
/// tolere les en-tetes d'un client HTTP ordinaire.
#[cfg(feature = "hs")]
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// Requete de catalogue analysee depuis une ligne de requete
/// `GET /channels/<pk64 hex>?…`.
#[derive(Debug)]
pub struct ChannelQuery {
    /// `channel_pk` du canal demande (cle `channel_node` — 64 o).
    pub channel_pk: Vec<u8>,
    /// `origin_id` de la racine suivie (0 = tout le canal).
    pub origin_id: i64,
    /// `first`/`last` du select (bornes deja appliquees a la
    /// lecture — `first` ≤ 10 000, `last` ≤ `first + 100`).
    pub first: u64,
    pub last: u64,
}

/// `None` = `first`/`last` par defaut d'un sync (`1`/`100`).
impl ChannelQuery {
    /// La requete JSON equivalente a un `remote_select` de
    /// synchronisation (meme `metadata_type` que `channel_sync`).
    pub fn to_select_json(&self) -> serde_json::Value {
        serde_json::json!({
            "channel_pk": hex::encode(&self.channel_pk),
            "origin_id": self.origin_id,
            "metadata_type": [400, 220, 500],
            "first": self.first,
            "last": self.last,
        })
    }
}

/// Source du catalogue — produit la frame LZ4 d'entrees `mdblob`
/// d'une requete (en pratique `Ipv8Stack::channel_catalog_frame`).
/// Fonction-object : le handler HTTP reste testable sans stack.
pub type CatalogFn = Arc<
    dyn Fn(
            ChannelQuery,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<u8>> + Send + 'static>>
        + Send
        + Sync,
>;

/// Analyse la zone en-tete d'une requete HTTP/1.x vers
/// `/channels/<pk64 hex>`. `Err(status)` = reponse a renvoyer
/// (`400` requete malformee, `404` chemin inconnu, `405` methode).
pub fn parse_channel_request(head: &str) -> Result<ChannelQuery, u16> {
    let line = head.lines().next().ok_or(400u16)?.trim_end();
    let mut parts = line.split_whitespace();
    let (method, target) = match (parts.next(), parts.next()) {
        (Some(m), Some(t)) => (m, t),
        _ => return Err(400),
    };
    let (path, raw_query) = target.split_once('?').unwrap_or((target, ""));
    let pk_hex = path.strip_prefix("/channels/").ok_or(404u16)?;
    // Requete reconnue mais mauvaise methode → 405 (pas 404 :
    // le endpoint existe).
    if method != "GET" {
        return Err(405);
    }
    let channel_pk = hex::decode(pk_hex.trim_end_matches('/')).map_err(|_| 400u16)?;
    if channel_pk.len() != 64 {
        return Err(400);
    }
    let mut query = ChannelQuery {
        channel_pk,
        origin_id: 0,
        first: 1,
        last: 100,
    };
    for kv in raw_query.split('&') {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        match k {
            "origin_id" => query.origin_id = v.parse().map_err(|_| 400u16)?,
            "first" => query.first = v.parse().map_err(|_| 400u16)?,
            "last" => query.last = v.parse().map_err(|_| 400u16)?,
            _ => {}
        }
    }
    Ok(query)
}

/// Corps d'une reponse HTTP/1.1 a corps ferme (`Connection:
/// close`) — le format servi par l'endpoint.
pub fn http_response(status: u16, reason: &str, body: &[u8]) -> Vec<u8> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

/// Lit la zone en-tete d'une requete HTTP/1.x (jusqu'a
/// `\r\n\r\n`, ≤ `MAX_HEAD_BYTES`) — partagee entre l'endpoint
/// catalogue (etape 126) et le coffre (etape 127). `None` = flux
/// clos ou plafond atteint : la connexion est abandonnee sans
/// reponse (le pair a raccroche ou debord).
#[cfg(feature = "hs")]
pub(crate) async fn read_request_head<S>(stream: &mut S) -> Option<Vec<u8>>
where
    S: tokio::io::AsyncRead + Unpin + Send,
{
    use tokio::io::AsyncReadExt;

    let mut head = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) => return None,
            Ok(n) => {
                head.extend_from_slice(&buf[..n]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    return Some(head);
                }
                if head.len() > MAX_HEAD_BYTES {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
}

/// Sert une connexion du port virtuel catalogue : lit la zone
/// en-tete (≤ `MAX_HEAD_BYTES`), produit la frame LZ4 du select
/// et repond. Les erreurs de flux sont silencieuses (le pair a
/// raccroche).
#[cfg(feature = "hs")]
pub async fn serve_catalog_stream<S>(mut stream: S, catalog: CatalogFn)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    use tokio::io::AsyncWriteExt;

    let Some(head) = read_request_head(&mut stream).await else {
        return;
    };
    let head_str = String::from_utf8_lossy(&head);
    let body = match parse_channel_request(&head_str) {
        Ok(query) => catalog(query).await,
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
    let _ = stream.write_all(&http_response(200, "OK", &body)).await;
}

/// Handler du port virtuel 80 (ADR-0029 etape 126) — un flux par
/// requete, traite dans sa propre tache.
#[cfg(feature = "hs")]
pub struct ChannelCatalogHandler {
    catalog: CatalogFn,
}

#[cfg(feature = "hs")]
impl ChannelCatalogHandler {
    pub fn new(catalog: CatalogFn) -> Self {
        Self { catalog }
    }
}

#[cfg(feature = "hs")]
impl onionbit_tor::OnionLinkHandler for ChannelCatalogHandler {
    fn on_stream(&self, stream: Box<dyn onionbit_tor::OnionIo>) {
        let catalog = self.catalog.clone();
        tokio::spawn(async move {
            serve_catalog_stream(stream, catalog).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_requete_canal() {
        let q = parse_channel_request(
            "GET /channels/aa55?origin_id=3&first=10&last=50 HTTP/1.1\r\nHost: x\r\n\r\n",
        )
        .unwrap_err();
        assert_eq!(q, 400); // pk hex trop courte → requete malformee

        let pk = "ab".repeat(64);
        let q = parse_channel_request(&format!(
            "GET /channels/{pk}?origin_id=3&first=10&last=50 HTTP/1.1\r\n\r\n"
        ))
        .expect("requete valide");
        assert_eq!(q.channel_pk.len(), 64);
        assert_eq!(q.origin_id, 3);
        assert_eq!(q.first, 10);
        assert_eq!(q.last, 50);
    }

    #[test]
    fn parse_rejets() {
        let pk = "ab".repeat(64);
        // Chemin inconnu → 404.
        assert_eq!(
            parse_channel_request("GET /autre HTTP/1.1\r\n\r\n").unwrap_err(),
            404
        );
        // Mauvaise methode sur un endpoint connu → 405.
        assert_eq!(
            parse_channel_request(&format!("POST /channels/{pk} HTTP/1.1\r\n\r\n")).unwrap_err(),
            405
        );
        // Parametre non numerique → 400.
        assert_eq!(
            parse_channel_request(&format!("GET /channels/{pk}?first=abc HTTP/1.1\r\n\r\n"))
                .unwrap_err(),
            400
        );
    }

    #[cfg(feature = "hs")]
    #[tokio::test]
    async fn serve_flux_round_trip() {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let catalog: CatalogFn = Arc::new(|q| {
            let body = q.to_select_json().to_string().into_bytes();
            Box::pin(async move { body })
        });
        tokio::spawn(serve_catalog_stream(server, catalog));
        let pk = "cd".repeat(64);
        let req = format!("GET /channels/{pk}?first=1&last=5 HTTP/1.1\r\nHost: x.onion\r\n\r\n");
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        client.write_all(req.as_bytes()).await.unwrap();
        client.shutdown().await.unwrap();
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "reponse : {text}");
        assert!(text.contains("\"first\":1"), "corps : {text}");
    }

    #[cfg(feature = "hs")]
    #[tokio::test]
    async fn serve_404_chemin_inconnu() {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let catalog: CatalogFn = Arc::new(|_| Box::pin(async { Vec::new() }));
        tokio::spawn(serve_catalog_stream(server, catalog));
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        client
            .write_all(b"GET /autre HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        assert!(String::from_utf8_lossy(&resp).starts_with("HTTP/1.1 404"));
    }
}
