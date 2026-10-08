// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Identite de conversation (`conv`, 16 octets — ADR-0019 §2).
//!
//! - **Directe** : deterministe —
//!   `SHA1("onionbit/conv/direct/v1" ‖ min(pk_a) ‖ max(pk_b))[:16]`.
//!   Calculable des deux cotes sans negociation : la migration des
//!   historiques v1 est mecanique et la reconnexion n'exige aucune
//!   trame d'identifiant.
//! - **Groupe** : aleatoire, cree par l'initiateur et propage aux
//!   invites par `gctl invite`.

use onionbit_crypto::hash::sha1;

/// Taille de l'identifiant de conversation.
pub const CONV_ID_LEN: usize = 16;

/// Identifiant de conversation (16 octets).
pub type ConvId = [u8; CONV_ID_LEN];

/// Domaine de derivation des conversations directes — disjoint du
/// domaine de swarm (`hash::messaging_hash`) et versionne : une
/// evolution de la grammaire change le domaine, jamais en place.
const CONV_DIRECT_DOMAIN: &[u8] = b"onionbit/conv/direct/v1";

/// `conv` directe des deux cles (ordre-insensible) : `min` et `max`
/// lexicographiques des blobs `pk` bruts — la meme valeur des deux
/// cotes du lien.
pub fn direct_conv(pk_a: &[u8], pk_b: &[u8]) -> ConvId {
    let (lo, hi) = if pk_a <= pk_b {
        (pk_a, pk_b)
    } else {
        (pk_b, pk_a)
    };
    let mut data = Vec::with_capacity(CONV_DIRECT_DOMAIN.len() + lo.len() + hi.len());
    data.extend_from_slice(CONV_DIRECT_DOMAIN);
    data.extend_from_slice(lo);
    data.extend_from_slice(hi);
    let h = sha1(&data);
    let mut conv = [0u8; CONV_ID_LEN];
    conv.copy_from_slice(&h[..CONV_ID_LEN]);
    conv
}

/// `conv` de groupe : aleatoire, emise par l'initiateur dans
/// `gctl invite` — jamais derivable, donc jamais devinable.
pub fn random_conv() -> ConvId {
    rand::random()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Determinisme et symetrie : les deux extremites calculent la
    /// meme `conv`, quel que soit l'ordre des cles.
    #[test]
    fn direct_conv_deterministe_et_symetrique() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        assert_eq!(direct_conv(&a, &b), direct_conv(&b, &a));
        assert_eq!(direct_conv(&a, &b), direct_conv(&a, &b));
    }

    /// Collision-freedom structurel : deux paires differentes → deux
    /// `conv` differentes (et un groupe aleatoire est distinct).
    #[test]
    fn direct_conv_distincte_par_paire() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let c = [3u8; 32];
        assert_ne!(direct_conv(&a, &b), direct_conv(&a, &c));
        assert_ne!(direct_conv(&a, &b), random_conv());
    }

    /// Vecteur interne : `SHA1(domaine ‖ min ‖ max)[:16]` tel
    /// qu'ecrit — le format ne peut pas bouger silencieusement
    /// (compat inter-demons OnionBit).
    #[test]
    fn direct_conv_vecteur() {
        let a = [9u8; 32];
        let b = [7u8; 32];
        // min = b (7 < 9).
        let mut expect = Vec::new();
        expect.extend_from_slice(b"onionbit/conv/direct/v1");
        expect.extend_from_slice(&b);
        expect.extend_from_slice(&a);
        let h = sha1(&expect);
        assert_eq!(direct_conv(&a, &b), h[..CONV_ID_LEN]);
    }
}
