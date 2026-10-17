// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Codec du lien messagerie sur service `.onion` (ADR-0029, etape
//! 124 — arbitrage Q1 : « trame TCP fine portant `obmsg` inchange »,
//! aucun endpoint REST).
//!
//! Le plan Tor change le **cable**, pas la charge utile : les octets
//! produits par [`crate::frame::Frame::seal`] circulent tels quels
//! dans le flux — signature Ed25519 et corps AEAD identiques au plan
//! IPv8 (defense en profondeur : le message reste chiffre e2e meme si
//! le circuit Tor etait compromis).
//!
//! Forme filaire :
//!
//! ```text
//! client → service  preambule : "OBMS" ‖ u8(1) ‖ pk_bin(74 o)
//!                   [crypt_pk declaree — permet au repondant de
//!                    deriver `MessagingKeys` avant la 1re trame ;
//!                    identite seulement suggeree : la signature de
//!                    trame reste la preuve]
//! les deux sens     trame :     u32 BE len ‖ octets `Frame::seal`
//!                   [len ≤ max_frame_len, repete tant que le lien vit]
//! ```
//!
//! Le port virtuel [`ONION_MSG_PORT`] est le `BEGIN` demande sur le
//! service — une convention interne OnionBit (le descripteur ne
//! publie pas de proxy : les `StreamRequest` sont acceptes
//! directement, sans listener TCP local exposé).

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::MessagingError;

/// Port virtuel du lien messagerie sur le `.onion` d'identite —
/// valeur conventionnelle OnionBit (pas un standard) : elle
/// distingue les `StreamRequest` messagerie des futurs usages du
/// service (catalogue HTTP etape 126 → port 80, coffre 127).
pub const ONION_MSG_PORT: u16 = 17713;

/// Magic du preambule de lien — versionne le protocole de transport
/// (distinct de `v` des trames, qui reste au niveau [`crate::frame`]).
const MAGIC: &[u8; 4] = b"OBMS";

/// Version du protocole de lien v1.
const PROTO_V1: u8 = 1;

/// Taille canonique de `pk_bin` (`"LibNaCLPK:"` + 64 octets de
/// materiel — crypt X25519 puis Ed25519).
pub const PK_BIN_LEN: usize = 74;

/// Taille du preambule client → service : magic + version + `pk_bin`.
pub const PREAMBLE_LEN: usize = MAGIC.len() + 1 + PK_BIN_LEN;

/// Prefixede longueur borne par `max_frame_len` (u32 BE).
const LEN_PREFIX_LEN: usize = 4;

/// Encode le preambule `"OBMS" ‖ 0x01 ‖ pk_bin` — refuse une cle
/// dont la taille n'est pas canonique (le repondant lit une forme
/// fixe : jamais de champ de longueur a faire confiance).
pub fn encode_preamble(pk_bin: &[u8]) -> Result<Vec<u8>, MessagingError> {
    if pk_bin.len() != PK_BIN_LEN {
        return Err(MessagingError::Malformed("preambule : pk_bin != 74 octets"));
    }
    let mut out = Vec::with_capacity(PREAMBLE_LEN);
    out.extend_from_slice(MAGIC);
    out.push(PROTO_V1);
    out.extend_from_slice(pk_bin);
    Ok(out)
}

/// Decode un preambule lu : magic + version verifiees, renvoie le
/// `pk_bin` embarque (hint de derivation — l'identite reste a
/// prouver par signature de trame).
pub fn decode_preamble(buf: &[u8]) -> Result<&[u8], MessagingError> {
    if buf.len() != PREAMBLE_LEN {
        return Err(MessagingError::Malformed("preambule : taille incorrecte"));
    }
    if buf[..MAGIC.len()] != *MAGIC {
        return Err(MessagingError::Malformed("preambule : magic absent"));
    }
    if buf[MAGIC.len()] != PROTO_V1 {
        return Err(MessagingError::UnknownVersion(buf[MAGIC.len()] as i64));
    }
    Ok(&buf[MAGIC.len() + 1..])
}

/// Ecrit le preambule complet (borne temporelle a la charge de
/// l'appelant — cf. `MessagingConfig::onion_io_timeout`).
pub async fn write_preamble<W: AsyncWrite + Unpin>(
    w: &mut W,
    pk_bin: &[u8],
) -> Result<(), MessagingError> {
    let buf = encode_preamble(pk_bin)?;
    w.write_all(&buf)
        .await
        .map_err(|e| MessagingError::LinkIo(format!("preambule : {e}")))?;
    w.flush()
        .await
        .map_err(|e| MessagingError::LinkIo(format!("flush preambule : {e}")))
}

/// Lit le preambule du client (exactement [`PREAMBLE_LEN`] octets)
/// sous `timeout` — un pair muet n'immobilise pas la tache. Renvoie
/// le `pk_bin` embarque ou `Err` (magic/version/taille).
pub async fn read_preamble<R: AsyncRead + Unpin>(
    r: &mut R,
    timeout: Duration,
) -> Result<Vec<u8>, MessagingError> {
    let mut buf = vec![0u8; PREAMBLE_LEN];
    let read = tokio::time::timeout(timeout, r.read_exact(&mut buf))
        .await
        .map_err(|_| MessagingError::LinkIo("preambule : delai depasse".into()))?
        .map_err(|e| MessagingError::LinkIo(format!("preambule : {e}")))?;
    let _ = read;
    Ok(decode_preamble(&buf)?.to_vec())
}

/// Ecrit une trame filaire (`Frame::seal`) prefixee de sa longueur
/// `u32` big-endian.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    wire: &[u8],
) -> Result<(), MessagingError> {
    let len = u32::try_from(wire.len())
        .map_err(|_| MessagingError::FrameTooLarge(wire.len(), u32::MAX as usize))?;
    w.write_all(&len.to_be_bytes())
        .await
        .map_err(|e| MessagingError::LinkIo(format!("prefixe : {e}")))?;
    w.write_all(wire)
        .await
        .map_err(|e| MessagingError::LinkIo(format!("corps : {e}")))?;
    Ok(())
}

/// Lit une trame prefixee : `Ok(None)` sur fin de flux propre avant
/// tout octet (lien ferme par le pair), `Err` sur troncation,
/// longueur > `max_len` ou I/O. La borne est lue **avant**
/// l'allocation — jamais de buffer de la taille declaree par le pair.
pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
    max_len: usize,
) -> Result<Option<Vec<u8>>, MessagingError> {
    let mut prefix = [0u8; LEN_PREFIX_LEN];
    match r.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(MessagingError::LinkIo(format!("prefixe : {e}"))),
    }
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 || len > max_len {
        return Err(MessagingError::FrameTooLarge(len, max_len));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .await
        .map_err(|e| MessagingError::LinkIo(format!("corps : {e}")))?;
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk_bin() -> Vec<u8> {
        // `LibNaCLPK:` (10 o) + 64 octets de cle fictive.
        let mut v = b"LibNaCLPK:".to_vec();
        v.extend_from_slice(&[0xAB; 64]);
        v
    }

    /// Le preambule code/decode le `pk_bin` canonique.
    #[test]
    fn preambule_round_trip() {
        let pk = pk_bin();
        let buf = encode_preamble(&pk).unwrap();
        assert_eq!(buf.len(), PREAMBLE_LEN);
        assert_eq!(&buf[..4], b"OBMS");
        assert_eq!(decode_preamble(&buf).unwrap(), pk.as_slice());
    }

    /// Preambules malformes : magic, version et taille rejetes.
    #[test]
    fn preambule_rejette_les_formes_invalides() {
        let pk = pk_bin();
        assert!(encode_preamble(&pk[..70]).is_err());
        let mut buf = encode_preamble(&pk).unwrap();
        buf[0] = b'X';
        assert!(decode_preamble(&buf).is_err());
        let mut buf = encode_preamble(&pk).unwrap();
        buf[4] = 0x02; // version inconnue
        assert!(matches!(
            decode_preamble(&buf),
            Err(MessagingError::UnknownVersion(2))
        ));
        assert!(decode_preamble(&buf[..buf.len() - 1]).is_err());
    }

    /// Trames prefixees en serie sur un duplex : ordre et contenu.
    #[tokio::test]
    async fn trames_prefixees_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_preamble(&mut a, &pk_bin()).await.unwrap();
        write_frame(&mut a, b"premier").await.unwrap();
        write_frame(&mut a, &[7u8; 900]).await.unwrap();
        drop(a);
        let pk = read_preamble(&mut b, Duration::from_secs(5)).await.unwrap();
        assert_eq!(pk, pk_bin());
        assert_eq!(read_frame(&mut b, 1024).await.unwrap().unwrap(), b"premier");
        assert_eq!(
            read_frame(&mut b, 1024).await.unwrap().unwrap(),
            vec![7u8; 900]
        );
        // Fin de flux propre = None (pas d'erreur).
        assert!(read_frame(&mut b, 1024).await.unwrap().is_none());
    }

    /// Une longueur hostile est refusee avant toute allocation ;
    /// la troncation et le vide (`len=0`) echouent aussi.
    #[tokio::test]
    async fn trame_prefixee_bornee() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&4096u32.to_be_bytes()).await.unwrap();
        assert!(matches!(
            read_frame(&mut b, 1024).await,
            Err(MessagingError::FrameTooLarge(4096, 1024))
        ));
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&0u32.to_be_bytes()).await.unwrap();
        assert!(read_frame(&mut b, 1024).await.is_err());
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&8u32.to_be_bytes()).await.unwrap();
        a.write_all(b"abc").await.unwrap();
        drop(a);
        assert!(read_frame(&mut b, 1024).await.is_err());
    }
}
