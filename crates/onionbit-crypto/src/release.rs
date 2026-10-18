// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Signature des releases `OBUP` (ADR-0032) — racine de confiance
//! unique, partagee entre `onionbit-daemon` (import) et
//! `onionbit-launcher` (swap).
//!
//! La clef privee de release est **hors-ligne** (machine de build) ;
//! seule la clef publique est embarquee ici. Signature domainee
//! `onionbit/update-manifest/v1` — un blob `OBUP` ne peut pas etre
//! reutilise comme signature d'un autre protocole OnionBit.

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};

/// Domaine de signature du manifest de release (convention
/// `onionbit/<usage>/v<maj>` — isole `OBUP` de `onion-bind` & co).
pub const UPDATE_DOMAIN: &[u8] = b"onionbit/update-manifest/v1";

/// Clef publique de release embarquee.
///
/// **PLACEHOLDER** : `[0;32]` tant qu'aucune release n'est signee —
/// une clef nulle ne verifie jamais de signature, donc tout `OBUP`
/// est refuse par un binaire non appareille (fail-closed). A la
/// premiere release : generer le keypair hors-ligne
/// (`onionbit-cli gen-release-key`), coller la clef publique ici,
/// garder la privee hors du repo. `key_id` = 8 premiers octets de
/// `SHA-256(pubkey)`.
pub const RELEASE_PUBKEY: [u8; 32] = [0; 32];

/// `key_id` d'une clef de release : `SHA-256(pk)[..8]` en u64 LE —
/// identifiant compact pour la rotation `next_key` (plusieurs clefs
/// peuvent coexister : embarquee + `state/release_key.next`).
pub fn release_key_id(pk: &[u8; 32]) -> u64 {
    use sha2::Digest;
    let h = sha2::Sha256::digest(pk);
    u64::from_le_bytes(h[..8].try_into().expect("8 octets"))
}

/// Corps signe : `domaine || 0 || manifest` — le separateur nul
/// empeche toute ambiguite de concatenation.
fn signed_body(manifest: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(UPDATE_DOMAIN.len() + 1 + manifest.len());
    msg.extend_from_slice(UPDATE_DOMAIN);
    msg.push(0);
    msg.extend_from_slice(manifest);
    msg
}

/// Signe le manifest bencode brut avec la clef de release.
pub fn sign_update_manifest(sk: &SigningKey, manifest: &[u8]) -> [u8; 64] {
    sk.sign(&signed_body(manifest)).to_bytes()
}

/// Verifie la signature d'un manifest `OBUP` sous `pk`.
/// `verify_strict` (et non `verify`) : les clefs faibles — point
/// identite `[0;32]` inclus — et les signatures non-canoniques sont
/// rejetees. Sans ca, le placeholder `[0;32]` serait une *vraie*
/// clef dont la signature `(0,0)` verifie — fail-open, pas closed.
pub fn verify_update_manifest(pk_bytes: &[u8; 32], manifest: &[u8], sig: &[u8; 64]) -> bool {
    let Ok(pk) = VerifyingKey::from_bytes(pk_bytes) else {
        return false;
    };
    pk.verify_strict(
        &signed_body(manifest),
        &ed25519_dalek::Signature::from_bytes(sig),
    )
    .is_ok()
}

/// Ensemble de clefs acceptees : embarquee + eventuelle clef
/// « suivante » persiste (`state/release_key.next`, 32 octets bruts)
/// apres une rotation `next_key` signee.
pub fn accepted_keys(state_dir: &std::path::Path) -> Vec<[u8; 32]> {
    let mut keys = vec![RELEASE_PUBKEY];
    if let Ok(next) = std::fs::read(state_dir.join("release_key.next")) {
        if let Ok(k) = <&[u8; 32]>::try_from(next.as_slice()) {
            keys.push(*k);
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_id_stable() {
        let pk = [0x42; 32];
        assert_eq!(release_key_id(&pk), release_key_id(&pk));
        assert_ne!(release_key_id(&pk), release_key_id(&[0x43; 32]));
    }

    #[test]
    fn clef_placeholder_ne_verifie_rien() {
        // [0;32] n'est pas un point Ed25519 valide pour une vraie
        // signature → toute verification contre la clef embarquee
        // par defaut echoue (fail-closed avant la 1re release).
        assert!(!verify_update_manifest(
            &RELEASE_PUBKEY,
            b"d1:ai1ee",
            &[0; 64]
        ));
    }

    #[test]
    fn domaine_isole_des_autres_usages() {
        let sk = SigningKey::from_bytes(&[7; 32]);
        let pk = sk.verifying_key().to_bytes();
        let sig = sign_update_manifest(&sk, b"manifest");
        assert!(verify_update_manifest(&pk, b"manifest", &sig));
        // Un manifest modifie ou un autre domaine ne verifie pas.
        assert!(!verify_update_manifest(&pk, b"manifest2", &sig));
    }
}
