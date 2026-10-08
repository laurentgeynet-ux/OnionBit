// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cles applicatives de la messagerie (ADR-0011, « separation des
//! cles ») : HKDF-SHA256 sur le secret e2e avec un domaine distinct
//! de `key_generation` — les `hs_session_keys` de transport et les
//! cles de corps de trame ne se compromettent pas mutuellement.

use hkdf::Hkdf;
use onionbit_crypto::error::CryptoError;

/// Info HKDF de la couche messagerie — disjoint de
/// `HKDF_INFO_KEY_GENERATION` (`b"key_generation"`, transport).
pub const HKDF_INFO_MESSAGING: &[u8] = b"onionbit messaging v1";

/// Taille du materiel derive : 2 x 32 octets (cle par direction).
const KEY_MATERIAL_LEN: usize = 64;

/// Cles applicatives d'une session e2e, une par direction.
///
/// Le premier bloc du materiel derive sert a l'initiateur du lien
/// e2e (celui qui a envoye `create-e2e`) pour chiffrer ; le second
/// au repondant. Chaque cote connait son role et tient donc
/// `send`/`recv` dans le bon ordre.
#[derive(Debug, Clone)]
pub struct MessagingKeys {
    /// Cle de chiffrement des corps emis.
    pub send: [u8; 32],
    /// Cle de chiffrement des corps recus.
    pub recv: [u8; 32],
}

/// Derive les cles applicatives depuis le secret e2e
/// (`crypto_box_beforenm` du `link-e2e` — le meme materiel que
/// `generate_session_keys`, domaine different).
///
/// `initiator` = role local sur le circuit lie (vrai cote
/// `RP_DOWNLOADER`, faux cote `IP_SEEDER`).
pub fn derive_messaging_keys(
    e2e_shared_secret: &[u8],
    initiator: bool,
) -> Result<MessagingKeys, CryptoError> {
    let hkdf = Hkdf::<sha2::Sha256>::from_prk(e2e_shared_secret)
        .map_err(|e| CryptoError::KeyDerivation(format!("HKDF prk: {e}")))?;
    let mut out = [0u8; KEY_MATERIAL_LEN];
    hkdf.expand(HKDF_INFO_MESSAGING, &mut out)
        .map_err(|e| CryptoError::KeyDerivation(format!("HKDF expand: {e}")))?;
    let mut i2r = [0u8; 32];
    let mut r2i = [0u8; 32];
    i2r.copy_from_slice(&out[..32]);
    r2i.copy_from_slice(&out[32..]);
    Ok(if initiator {
        MessagingKeys {
            send: i2r,
            recv: r2i,
        }
    } else {
        MessagingKeys {
            send: r2i,
            recv: i2r,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_crypto::ipv8::session::{generate_session_keys, Direction, SessionKeys};

    /// Les deux roles derivent des cles miroir : `send` de l'un =
    /// `recv` de l'autre.
    #[test]
    fn derivation_miroir_des_roles() {
        let secret = [42u8; 32];
        let init = derive_messaging_keys(&secret, true).unwrap();
        let resp = derive_messaging_keys(&secret, false).unwrap();
        assert_eq!(init.send, resp.recv);
        assert_eq!(init.recv, resp.send);
        assert_ne!(init.send, init.recv);
    }

    /// Separation de domaine : les cles messagerie ne coincident
    /// avec aucune cle de transport `hs_session_keys` derivee du
    /// meme secret.
    #[test]
    fn domaine_separe_de_key_generation() {
        let secret = [7u8; 32];
        let app = derive_messaging_keys(&secret, true).unwrap();
        let transport: SessionKeys = generate_session_keys(&secret).unwrap();
        assert_ne!(app.send, transport.key_forward);
        assert_ne!(app.send, transport.key_backward);
        assert_ne!(app.recv, transport.key_forward);
        assert_ne!(app.recv, transport.key_backward);
        // Utilise Direction pour eviter un import mort.
        let _ = Direction::Forward;
    }
}
