// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Format filaire du transport furtif `stealth` (ADR-0017, etape
//! 50). Les primitives (Elligator2, X25519, HKDF, AEAD) vivent dans
//! [`onionbit_crypto::stealth`] ; ici : trames, sessions, filtres.
//!
//! Principe absolu : **rien de statique sur le fil**. Pas de magic,
//! pas de version, pas de cle en clair, pas de compteur lisible —
//! chaque datagramme est une suite d'octets uniformes pour
//! l'observateur :
//!
//! - `hs1` client→pont : `rep(X')(32) ‖ AEAD(k_hs1)〔 v ‖ ts ‖
//!   id_len ‖ id ‖ pad 〕` — l'AEAD joue le role du « MAC statique » :
//!   seul le detenteur du secret `b` du pont peut ouvrir. Le
//!   timestamp est dans le plaintext authentifie (anti-rejeu du
//!   premier datagramme).
//! - `hs2` pont→client : `rep(Y')(32) ‖ AEAD(k_hs2)〔 v ‖ ts ‖ pad
//!   〕` — cle sous `shared_ee` : impossible a forger sans `y`.
//! - `frame` : `field(8) ‖ AEAD(k_dir, nonce=compteur)〔 len ‖ inner
//!   ‖ pad 〕` — `field = compteur ⊕ mask` : le compteur sert de
//!   nonce AEAD (anti-rejeu par fenetre) sans jamais apparaitre en
//!   clair (un compteur nu est un signal sequentiel classifiable).
//!
//! **Rejet uniforme** : toute erreur d'ouverture est `Reject` — la
//! cause (MAC, timestamp, rejeu, saturation) n'affecte que le
//! diagnostic local, jamais le comportement observable.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use onionbit_crypto::stealth::{
    aead_open, aead_seal, hs1_key, hs2_key, representative_to_point, session_keys, x25519,
    HiddenEph, SessionKeys,
};

/// MTU du datagramme stealth (IPv6 garantie) — jamais de
/// fragmentation UDP : les routeurs censures la droppent en masse.
pub const STEALTH_MTU: usize = 1280;

/// Overhead filaire d'une trame : `field(8) + len(2) + tag AEAD(16)`.
const FRAME_OVERHEAD: usize = 8 + 2 + 16;

/// Overhead filaire d'un handshake : `rep(32) + tag AEAD(16)`.
const HS_OVERHEAD: usize = 32 + 16;

/// Parametres du transport stealth — aucun seuil en dur (AGENTS) ;
/// les valeurs par defaut suivent l'ADR-0017 §2/§3.
#[derive(Debug, Clone)]
pub struct StealthParams {
    /// Fenetre `±` de l'horodatage du handshake (anti-rejeu) —
    /// secondes. ADR : 90.
    pub hs_timestamp_skew_secs: u64,
    /// Padding additif max ajoute au plaintext (`inner +
    /// uniform(0, pad_max_extra)`), clampe a la MTU. ADR : 400.
    pub pad_max_extra: usize,
    /// Fenetre de rejeu des trames par session (bits). ADR : 256.
    pub replay_window: usize,
    /// Capacite max du filtre anti-rejeu `X'` (sature → silence).
    /// ADR : ~50 000.
    pub xprime_set_max: usize,
    /// MTU du datagramme produit. ADR : [`STEALTH_MTU`].
    pub mtu: usize,
}

impl Default for StealthParams {
    fn default() -> Self {
        Self {
            hs_timestamp_skew_secs: 90,
            pad_max_extra: 400,
            replay_window: 256,
            xprime_set_max: 50_000,
            mtu: STEALTH_MTU,
        }
    }
}

/// Erreur stealth. `Reject` porte une cause **pour le diagnostic
/// local uniquement** — elle ne doit jamais fuiter sur le fil ni
/// dans un log bruyant (compteur borne au plus).
#[derive(Debug, thiserror::Error)]
pub enum StealthError {
    /// Rejet silencieux — cause interne (`&'static str`), jamais
    /// serialisee.
    #[error("rejete: {0}")]
    Reject(&'static str),
    /// Le payload depasse le budget MTU — erreur cote emetteur,
    /// jamais emise.
    #[error("inner trop grand pour le budget MTU stealth")]
    TooLarge,
}

/// Alea pour le padding (source : `rand::rng`).
fn pad_len(max_extra: usize) -> usize {
    if max_extra == 0 {
        return 0;
    }
    let mut b = [0u8; 8];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut b);
    (u64::from_le_bytes(b) % (max_extra as u64 + 1)) as usize
}

/// Octets aleatoires (padding — contenu librement variable).
fn rand_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut v);
    v
}

/// Filtre anti-rejeu des `X'` de handshake — deux fenetres
/// temporelles (courante + precedente, purge a chaque bascule) sur
/// `HashSet` borne : O(1), zero allocation non bornee. La cle est le
/// **point canonique** (pas le representant : ses 2 bits hauts sont
/// randomises — le point est la forme de dedup correcte).
pub struct XPrimeFilter {
    /// Fenetre courante.
    cur: HashSet<[u8; 32]>,
    /// Fenetre precedente (couvre le skew maximal residuel).
    prev: HashSet<[u8; 32]>,
    /// Debut de la fenetre courante.
    period_started: Instant,
    /// Duree d'une fenetre (= `hs_timestamp_skew_secs`).
    period: Duration,
    /// Borne `xprime_set_max` — sature → rejet silencieux.
    max: usize,
}

impl XPrimeFilter {
    /// `period_secs` : duree d'une fenetre — la retention couvre
    /// `[t, t+2*period)` ≥ fenetre de validite du timestamp.
    pub fn new(period_secs: u64, max: usize) -> Self {
        Self {
            cur: HashSet::new(),
            prev: HashSet::new(),
            period_started: Instant::now(),
            period: Duration::from_secs(period_secs.max(1)),
            max,
        }
    }

    /// Insere `point` s'il est inconnu et que le filtre n'est pas
    /// sature. `now` injecte l'horloge (tests). Retourne `false` sur
    /// rejeu OU saturation — le comportement externe est identique.
    pub fn insert_fresh(&mut self, point: &[u8; 32], now: Instant) -> bool {
        if now.duration_since(self.period_started) >= self.period {
            self.prev = std::mem::take(&mut self.cur);
            self.period_started = now;
        }
        if self.prev.contains(point) || self.cur.contains(point) {
            return false;
        }
        if self.cur.len() >= self.max {
            return false;
        }
        self.cur.insert(*point)
    }

    /// Nombre d'entrees actives (diagnostic/tests).
    #[cfg(test)]
    fn len(&self) -> usize {
        self.cur.len() + self.prev.len()
    }
}

/// Contexte client conserve entre `hs1_seal` et `hs2_open`.
pub struct Hs1ClientCtx {
    /// Representant emis.
    rep_x: [u8; 32],
    /// `X25519(x, B)` — cle d'authentification statique.
    static_dh: [u8; 32],
}

/// Resultat d'un `hs1` accepte cote pont.
pub struct Hs1Accepted {
    /// Representant recu (tel que filaire — entre au transcript).
    rep_x: [u8; 32],
    /// Point canonique decode.
    x_point: [u8; 32],
    /// Identite client chiffree vehiculee (`client_id`).
    pub client_id: Vec<u8>,
    /// `X25519(b, X')`.
    static_dh: [u8; 32],
}

/// Taille max de l'identite client dans `hs1` (borne le travail
/// d'allocation a l'ouverture).
pub const HS1_ID_MAX: usize = 256;

/// Version interne du handshake (dans le plaintext AEAD — jamais
/// visible en clair).
const HS_VERSION: u8 = 1;

/// `hs1` client→pont : `rep(X') ‖ AEAD(k_hs1)〔v‖ts‖len‖id‖pad〕`.
///
/// Retourne le datagramme a envoyer et le contexte a conserver pour
/// `hs2_open`. `client_id` est une identite opaque choisie par
/// l'appelant (pk maitresse en v1) — borne [`HS1_ID_MAX`].
pub fn hs1_seal(
    x: &HiddenEph,
    bridge_pk: &[u8; 32],
    client_id: &[u8],
    now_ts: u64,
    params: &StealthParams,
) -> Result<(Vec<u8>, Hs1ClientCtx), StealthError> {
    if client_id.len() > HS1_ID_MAX {
        return Err(StealthError::TooLarge);
    }
    let rep_x = x.representative();
    // `X25519(x, B)` — `bridge_pk` est une u-coordonnee publique.
    let static_dh = x.diffie_hellman(bridge_pk);
    let k = hs1_key(bridge_pk, &rep_x, &static_dh);
    let mut pt = Vec::with_capacity(11 + client_id.len());
    pt.push(HS_VERSION);
    pt.extend_from_slice(&now_ts.to_be_bytes());
    pt.extend_from_slice(&(client_id.len() as u16).to_be_bytes());
    pt.extend_from_slice(client_id);
    // Padding additif : le handshake n'a pas de taille fixe
    // observable.
    pt.extend_from_slice(&rand_bytes(pad_len(
        params
            .pad_max_extra
            .min(params.mtu.saturating_sub(HS_OVERHEAD + pt.len())),
    )));
    let mut dgram = Vec::with_capacity(HS_OVERHEAD + pt.len());
    dgram.extend_from_slice(&rep_x);
    dgram.extend_from_slice(&aead_seal(&k, &[0u8; 12], &rep_x, &pt));
    Ok((dgram, Hs1ClientCtx { rep_x, static_dh }))
}

/// Ouverture `hs1` cote pont. Toute cause d'echec → `Reject`
/// (comportement externe identique). `filter` deduplique sur le
/// point `X'` — n'est alimente qu'apres authentification reussie :
/// un flot de garbage ne peut pas saturer le filtre.
pub fn hs1_open(
    bridge_sk: &[u8; 32],
    bridge_pk: &[u8; 32],
    dgram: &[u8],
    filter: &mut XPrimeFilter,
    now_ts: u64,
    params: &StealthParams,
) -> Result<Hs1Accepted, StealthError> {
    if dgram.len() < HS_OVERHEAD + 11 || dgram.len() > params.mtu {
        return Err(StealthError::Reject("borne"));
    }
    let rep_x: [u8; 32] = dgram[..32].try_into().expect("borne");
    let x_point = representative_to_point(&rep_x);
    let static_dh = x25519(bridge_sk, &x_point);
    let k = hs1_key(bridge_pk, &rep_x, &static_dh);
    let pt = aead_open(&k, &[0u8; 12], &rep_x, &dgram[32..])
        .map_err(|_| StealthError::Reject("auth"))?;
    if pt.len() < 11 || pt[0] != HS_VERSION {
        return Err(StealthError::Reject("forme"));
    }
    let ts = u64::from_be_bytes(pt[1..9].try_into().expect("borne"));
    if ts.abs_diff(now_ts) > params.hs_timestamp_skew_secs {
        return Err(StealthError::Reject("horodatage"));
    }
    let id_len = u16::from_be_bytes(pt[9..11].try_into().expect("borne")) as usize;
    if pt.len() < 11 + id_len {
        return Err(StealthError::Reject("borne id"));
    }
    if !filter.insert_fresh(&x_point, Instant::now()) {
        return Err(StealthError::Reject("rejeu X'"));
    }
    Ok(Hs1Accepted {
        rep_x,
        x_point,
        client_id: pt[11..11 + id_len].to_vec(),
        static_dh,
    })
}

/// `hs2` pont→client : `rep(Y') ‖ AEAD(k_hs2)〔v‖ts‖pad〕`. Retourne
/// le datagramme et la session etablie cote pont.
pub fn hs2_seal(
    y: &HiddenEph,
    hs1: &Hs1Accepted,
    now_ts: u64,
    params: &StealthParams,
) -> (Vec<u8>, StealthSession) {
    let rep_y = y.representative();
    let shared_ee = y.diffie_hellman(&hs1.x_point);
    let k2 = hs2_key(&hs1.rep_x, &rep_y, &shared_ee, &hs1.static_dh);
    let mut pt = Vec::with_capacity(9);
    pt.push(HS_VERSION);
    pt.extend_from_slice(&now_ts.to_be_bytes());
    pt.extend_from_slice(&rand_bytes(pad_len(
        params
            .pad_max_extra
            .min(params.mtu.saturating_sub(HS_OVERHEAD + pt.len())),
    )));
    let mut dgram = Vec::with_capacity(HS_OVERHEAD + pt.len());
    dgram.extend_from_slice(&rep_y);
    dgram.extend_from_slice(&aead_seal(&k2, &[0u8; 12], &rep_y, &pt));
    let ks = session_keys(&shared_ee, &hs1.static_dh, &hs1.rep_x, &rep_y);
    (dgram, StealthSession::bridge_side(ks, params))
}

/// Ouverture `hs2` cote client → session etablie.
pub fn hs2_open(
    x: &HiddenEph,
    ctx: &Hs1ClientCtx,
    dgram: &[u8],
    now_ts: u64,
    params: &StealthParams,
) -> Result<StealthSession, StealthError> {
    if dgram.len() < HS_OVERHEAD + 9 || dgram.len() > params.mtu {
        return Err(StealthError::Reject("borne"));
    }
    let rep_y: [u8; 32] = dgram[..32].try_into().expect("borne");
    let y_point = representative_to_point(&rep_y);
    let shared_ee = x.diffie_hellman(&y_point);
    let k2 = hs2_key(&ctx.rep_x, &rep_y, &shared_ee, &ctx.static_dh);
    let pt = aead_open(&k2, &[0u8; 12], &rep_y, &dgram[32..])
        .map_err(|_| StealthError::Reject("auth"))?;
    if pt.len() < 9 || pt[0] != HS_VERSION {
        return Err(StealthError::Reject("forme"));
    }
    let ts = u64::from_be_bytes(pt[1..9].try_into().expect("borne"));
    if ts.abs_diff(now_ts) > params.hs_timestamp_skew_secs {
        return Err(StealthError::Reject("horodatage"));
    }
    let ks = session_keys(&shared_ee, &ctx.static_dh, &ctx.rep_x, &rep_y);
    Ok(StealthSession::client_side(ks, params))
}

/// Session stealth etablie : cles directionnelles, compteur sortant,
/// fenetre de rejeu entrante (bitmap glissante, marquee **apres**
/// verification AEAD — une trame forgee ne peut pas faire glisser la
/// fenetre sous les trames legitimes).
pub struct StealthSession {
    /// Cle AEAD emission.
    k_tx: [u8; 32],
    /// Masque du compteur filaire emission.
    mask_tx: u64,
    /// Compteur emission (nonce, jamais reutilise).
    tx_ctr: u64,
    /// Cle AEAD reception.
    k_rx: [u8; 32],
    /// Masque du compteur filaire reception.
    mask_rx: u64,
    /// Plus haut compteur verifie.
    rx_top: u64,
    /// Bitmap : bit `i` = compteur `rx_top - i` deja verifie.
    rx_map: Vec<u64>,
    /// Fenetre de rejeu (bits).
    window: usize,
    /// Budget MTU et padding.
    mtu: usize,
    /// `pad_max_extra`.
    pad_max: usize,
}

impl StealthSession {
    fn new(ks: SessionKeys, params: &StealthParams, client_side: bool) -> Self {
        let (k_tx, mask_tx, k_rx, mask_rx) = if client_side {
            (ks.c2b, ks.mask_c2b, ks.b2c, ks.mask_b2c)
        } else {
            (ks.b2c, ks.mask_b2c, ks.c2b, ks.mask_c2b)
        };
        Self {
            k_tx,
            mask_tx,
            tx_ctr: 0,
            k_rx,
            mask_rx,
            rx_top: 0,
            rx_map: vec![0u64; params.replay_window.div_ceil(64)],
            window: params.replay_window,
            mtu: params.mtu,
            pad_max: params.pad_max_extra,
        }
    }

    /// Session vue cote client (`c2b` en emission).
    fn client_side(ks: SessionKeys, params: &StealthParams) -> Self {
        Self::new(ks, params, true)
    }

    /// Session vue cote pont (`b2c` en emission).
    fn bridge_side(ks: SessionKeys, params: &StealthParams) -> Self {
        Self::new(ks, params, false)
    }

    /// Scelle un datagramme interne : `field ‖ AEAD〔len‖inner‖pad〕`
    /// ou `field = ctr ⊕ mask_tx`. `inner` ≤ `mtu - 26` sinon
    /// `TooLarge` (l'appelant decoupe — le transport ne fragmente
    /// jamais).
    pub fn seal_frame(&mut self, inner: &[u8]) -> Result<Vec<u8>, StealthError> {
        if inner.len() > self.mtu.saturating_sub(FRAME_OVERHEAD) {
            return Err(StealthError::TooLarge);
        }
        let ctr = self.tx_ctr;
        self.tx_ctr += 1;
        let field = (ctr ^ self.mask_tx).to_le_bytes();
        let budget_pad = self.mtu - FRAME_OVERHEAD - inner.len();
        let mut pt = Vec::with_capacity(2 + inner.len() + budget_pad.min(self.pad_max));
        pt.extend_from_slice(&(inner.len() as u16).to_be_bytes());
        pt.extend_from_slice(inner);
        pt.extend_from_slice(&rand_bytes(pad_len(budget_pad.min(self.pad_max))));
        let nonce: [u8; 12] = {
            let mut n = [0u8; 12];
            n[4..].copy_from_slice(&ctr.to_be_bytes());
            n
        };
        let mut out = Vec::with_capacity(8 + pt.len() + 16);
        out.extend_from_slice(&field);
        out.extend_from_slice(&aead_seal(&self.k_tx, &nonce, &field, &pt));
        Ok(out)
    }

    /// Ouvre une trame : deobfuscation du compteur, borne de fenetre,
    /// verification AEAD, **puis** marquage (une forge ne fait pas
    /// glisser la fenetre). Tout echec → `Reject`.
    pub fn open_frame(&mut self, dgram: &[u8]) -> Result<Vec<u8>, StealthError> {
        if dgram.len() < 8 + 2 + 16 || dgram.len() > self.mtu {
            return Err(StealthError::Reject("borne"));
        }
        let field: [u8; 8] = dgram[..8].try_into().expect("borne");
        let ctr = u64::from_le_bytes(field) ^ self.mask_rx;
        // Hors fenetre basse : vieux compteur → rejet avant AEAD
        // (borne CPU : une trame trop ancienne ne vaut pas un open).
        let too_old =
            self.rx_top > 0 && ctr < self.rx_top && (self.rx_top - ctr) as usize >= self.window;
        if too_old {
            return Err(StealthError::Reject("compteur ancien"));
        }
        if ctr <= self.rx_top && self.bit_set(self.rx_top - ctr) {
            return Err(StealthError::Reject("rejeu trame"));
        }
        let nonce: [u8; 12] = {
            let mut n = [0u8; 12];
            n[4..].copy_from_slice(&ctr.to_be_bytes());
            n
        };
        let pt = aead_open(&self.k_rx, &nonce, &field, &dgram[8..])
            .map_err(|_| StealthError::Reject("auth"))?;
        if pt.len() < 2 {
            return Err(StealthError::Reject("forme"));
        }
        let inner_len = u16::from_be_bytes(pt[..2].try_into().expect("borne")) as usize;
        if pt.len() < 2 + inner_len {
            return Err(StealthError::Reject("borne inner"));
        }
        // Verifiee : marquage / glissement.
        if ctr > self.rx_top {
            self.slide(ctr - self.rx_top);
            self.rx_top = ctr;
        }
        self.set_bit(self.rx_top - ctr);
        Ok(pt[2..2 + inner_len].to_vec())
    }

    /// Bit `d` (distance depuis `rx_top`) deja marque ?
    fn bit_set(&self, d: u64) -> bool {
        let i = d as usize;
        i < self.window && self.rx_map[i / 64] & (1u64 << (i % 64)) != 0
    }

    /// Marque le bit `d`.
    fn set_bit(&mut self, d: u64) {
        let i = d as usize;
        if i < self.window {
            self.rx_map[i / 64] |= 1u64 << (i % 64);
        }
    }

    /// Fait glisser la fenetre de `shift` positions vers le haut :
    /// le compteur a distance `d` devient distance `d + shift`.
    fn slide(&mut self, shift: u64) {
        let shift = shift as usize;
        if shift >= self.window {
            self.rx_map.iter_mut().for_each(|w| *w = 0);
            return;
        }
        let ws = shift / 64;
        let bs = shift % 64;
        for i in (0..self.rx_map.len()).rev() {
            let hi = if i >= ws { self.rx_map[i - ws] } else { 0 };
            let lo = if bs > 0 && i > ws {
                self.rx_map[i - ws - 1]
            } else {
                0
            };
            self.rx_map[i] = (hi << bs) | if bs > 0 { lo >> (64 - bs) } else { 0 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> StealthParams {
        StealthParams::default()
    }

    /// Pont statique : cle X25519 (la `bridge_pk` du lien d'invitation) —
    /// emprunte la cle `crypt` d'une identite `LibNaClSecretKey`.
    fn bridge() -> (onionbit_crypto::ipv8::keys::LibNaClSecretKey, [u8; 32]) {
        let sk = onionbit_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let pk = *sk.public_key().crypt_x25519().as_bytes();
        (sk, pk)
    }

    #[test]
    fn handshake_puis_trames_aller_retour() {
        let (b_sk, b_pk) = bridge();
        let p = params();
        let now = 1_760_000_000u64;
        let x = HiddenEph::generate();
        let (d1, ctx) = hs1_seal(&x, &b_pk, b"client-pk", now, &p).unwrap();
        let mut filt = XPrimeFilter::new(p.hs_timestamp_skew_secs, p.xprime_set_max);
        let acc = hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &d1,
            &mut filt,
            now,
            &p,
        )
        .unwrap();
        assert_eq!(acc.client_id, b"client-pk");
        let y = HiddenEph::generate();
        let (d2, mut s_bridge) = hs2_seal(&y, &acc, now, &p);
        let mut s_client = hs2_open(&x, &ctx, &d2, now, &p).unwrap();
        // Trames dans les deux sens.
        for i in 0..300u64 {
            let f = s_client.seal_frame(&[i as u8; 100]).unwrap();
            assert_eq!(s_bridge.open_frame(&f).unwrap(), vec![i as u8; 100]);
        }
        let f = s_bridge.seal_frame(b"reponse").unwrap();
        assert_eq!(s_client.open_frame(&f).unwrap(), b"reponse");
    }

    #[test]
    fn hs1_garbage_mauvaise_cle_rejeu_silencieux() {
        let (b_sk, b_pk) = bridge();
        let (_, other_pk) = bridge();
        let p = params();
        let now = 1_760_000_000u64;
        let x = HiddenEph::generate();
        let (d1, _ctx) = hs1_seal(&x, &b_pk, b"id", now, &p).unwrap();
        let mut filt = XPrimeFilter::new(p.hs_timestamp_skew_secs, p.xprime_set_max);
        // Garbage pur.
        assert!(hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &[9u8; 200],
            &mut filt,
            now,
            &p
        )
        .is_err());
        assert!(hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &d1[..20],
            &mut filt,
            now,
            &p
        )
        .is_err());
        // Bonne forme, mauvais pont.
        assert!(hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &other_pk,
            &d1,
            &mut filt,
            now,
            &p
        )
        .is_err());
        // Premier passage OK, le rejeu du meme datagramme → rejet
        // (filtre X').
        hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &d1,
            &mut filt,
            now,
            &p,
        )
        .unwrap();
        assert!(hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &d1,
            &mut filt,
            now,
            &p
        )
        .is_err());
        assert_eq!(filt.len(), 1);
    }

    #[test]
    fn hs1_horodatage_hors_fenetre() {
        let (b_sk, b_pk) = bridge();
        let p = params();
        let now = 1_760_000_000u64;
        let x = HiddenEph::generate();
        // emis avec une horloge en avance de 2*skew
        let (d1, _c) =
            hs1_seal(&x, &b_pk, b"id", now + 2 * p.hs_timestamp_skew_secs + 5, &p).unwrap();
        let mut filt = XPrimeFilter::new(p.hs_timestamp_skew_secs, p.xprime_set_max);
        assert!(hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &d1,
            &mut filt,
            now,
            &p
        )
        .is_err());
    }

    #[test]
    fn trame_rejeu_desordre_et_fenetre() {
        let (b_sk, b_pk) = bridge();
        let p = params();
        let now = 1_760_000_000u64;
        let x = HiddenEph::generate();
        let (d1, ctx) = hs1_seal(&x, &b_pk, b"id", now, &p).unwrap();
        let mut filt = XPrimeFilter::new(p.hs_timestamp_skew_secs, p.xprime_set_max);
        let acc = hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &d1,
            &mut filt,
            now,
            &p,
        )
        .unwrap();
        let y = HiddenEph::generate();
        let (d2, mut sb) = hs2_seal(&y, &acc, now, &p);
        let mut sc = hs2_open(&x, &ctx, &d2, now, &p).unwrap();

        // Desordre dans la fenetre.
        let f0 = sc.seal_frame(b"a").unwrap();
        let f1 = sc.seal_frame(b"b").unwrap();
        let f2 = sc.seal_frame(b"c").unwrap();
        assert_eq!(sb.open_frame(&f2).unwrap(), b"c");
        assert_eq!(sb.open_frame(&f0).unwrap(), b"a");
        assert_eq!(sb.open_frame(&f1).unwrap(), b"b");
        // Rejeu exact → rejet.
        assert!(sb.open_frame(&f2).is_err());
        // Compteur hors fenetre basse → rejet.
        sb.rx_top = 10_000;
        assert!(sb.open_frame(&f0).is_err());
        // Trame corrompue → rejet.
        let mut bad = sb_rx(&mut sc);
        bad[10] ^= 0x55;
        assert!(sb.open_frame(&bad).is_err());
    }

    fn sb_rx(sc: &mut StealthSession) -> Vec<u8> {
        sc.seal_frame(b"x").unwrap()
    }

    #[test]
    fn troncature_toutes_bornes_rejet_uniforme() {
        // Oracle hostile : pour CHAQUE longueur de troncature, hs1,
        // hs2 et trame doivent etre rejetes (aucun prefixe valide ne
        // doit produire d'acceptation ni de panic).
        let (b_sk, b_pk) = bridge();
        let p = params();
        let now = 1_760_000_000u64;
        let x = HiddenEph::generate();
        let (d1, ctx) = hs1_seal(&x, &b_pk, b"id", now, &p).unwrap();
        let mut filt = XPrimeFilter::new(p.hs_timestamp_skew_secs, p.xprime_set_max);
        for cut in 0..d1.len() {
            assert!(
                hs1_open(
                    b_sk.crypt_x25519().as_bytes(),
                    &b_pk,
                    &d1[..cut],
                    &mut filt,
                    now,
                    &p
                )
                .is_err(),
                "hs1 accepte a la troncature {cut}"
            );
        }
        let acc = hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &d1,
            &mut filt,
            now,
            &p,
        )
        .unwrap();
        let y = HiddenEph::generate();
        let (d2, mut sb) = hs2_seal(&y, &acc, now, &p);
        for cut in 0..d2.len() {
            assert!(
                hs2_open(&x, &ctx, &d2[..cut], now, &p).is_err(),
                "hs2 accepte a la troncature {cut}"
            );
        }
        let mut sc = hs2_open(&x, &ctx, &d2, now, &p).unwrap();
        let f = sc.seal_frame(b"payload-de-test").unwrap();
        for cut in 0..f.len() {
            assert!(
                sb.open_frame(&f[..cut]).is_err(),
                "trame acceptee a la troncature {cut}"
            );
        }
        // Bit-flip exhaustif sur la trame (f debutant a ctr=0 ici :
        // seuls les flips du champ masque menent a un autre compteur
        // — tous doivent echouer l'AEAD ou la fenetre).
        for i in 0..f.len() * 8 {
            let mut bad = f.clone();
            bad[i / 8] ^= 1 << (i % 8);
            assert!(sb.open_frame(&bad).is_err(), "flip bit {i} accepte");
        }
        // La trame intacte passe encore (le filtre n'a pas ete
        // empoisonne par les forges).
        assert_eq!(sb.open_frame(&f).unwrap(), b"payload-de-test");
    }

    #[test]
    fn fenetre_bords_exactement() {
        // Compteur a distance exactement `window` sous le sommet →
        // rejet ; a `window - 1` → accepte ; glissement au-dela de la
        // fenetre vide la map.
        let (b_sk, b_pk) = bridge();
        let mut p = params();
        p.replay_window = 64;
        let now = 1_760_000_000u64;
        let x = HiddenEph::generate();
        let (d1, ctx) = hs1_seal(&x, &b_pk, b"id", now, &p).unwrap();
        let mut filt = XPrimeFilter::new(p.hs_timestamp_skew_secs, p.xprime_set_max);
        let acc = hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &d1,
            &mut filt,
            now,
            &p,
        )
        .unwrap();
        let y = HiddenEph::generate();
        let (d2, mut sb) = hs2_seal(&y, &acc, now, &p);
        let mut sc = hs2_open(&x, &ctx, &d2, now, &p).unwrap();

        // Emet ctr 0..=64 : le receveur saute directement a 64 —
        // ctr 0 est alors a distance 64 == window → rejet.
        let frames: Vec<Vec<u8>> = (0..=64).map(|_| sc.seal_frame(b"z").unwrap()).collect();
        assert!(sb.open_frame(&frames[64]).is_ok());
        assert!(
            sb.open_frame(&frames[0]).is_err(),
            "ctr 0 a distance == window"
        );
        // ctr 1..=63 encore dans la fenetre.
        assert!(sb.open_frame(&frames[63]).is_ok());
        assert!(sb.open_frame(&frames[1]).is_ok());
        // Rejeu du sommet courant.
        assert!(sb.open_frame(&frames[64]).is_err());
    }

    #[test]
    fn separation_de_domaine_obf_stealth() {
        // Un blob OBF (domaine `onionbit/ext-obf/v1`) soumis comme
        // trame stealth → rejet ; et reciproquement une trame stealth
        // n'est pas lisible comme enveloppe OBF.
        use crate::ext::obf;
        use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let obf_blob = obf::seal(&b.public_key(), &a, 1, b"obf", obf::OBF_PAD_BUCKET).unwrap();

        let (b_sk, b_pk) = bridge();
        let p = params();
        let now = 1_760_000_000u64;
        let x = HiddenEph::generate();
        let (d1, ctx) = hs1_seal(&x, &b_pk, b"id", now, &p).unwrap();
        let mut filt = XPrimeFilter::new(p.hs_timestamp_skew_secs, p.xprime_set_max);
        let acc = hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &d1,
            &mut filt,
            now,
            &p,
        )
        .unwrap();
        let y = HiddenEph::generate();
        let (d2, mut sb) = hs2_seal(&y, &acc, now, &p);
        let mut sc = hs2_open(&x, &ctx, &d2, now, &p).unwrap();
        let st_frame = sc.seal_frame(b"stealth").unwrap();

        // OBF → stealth : rejet.
        assert!(sb.open_frame(&obf_blob).is_err());
        // stealth → OBF : erreur d'ouverture (AEAD/domaine).
        assert!(obf::open(&a.public_key(), &b, &st_frame).is_err());
    }

    #[test]
    fn xprime_filter_deux_fenetres_et_saturation() {
        let mut f = XPrimeFilter::new(60, 4);
        let t0 = Instant::now();
        let p = [7u8; 32];
        assert!(f.insert_fresh(&p, t0));
        assert!(!f.insert_fresh(&p, t0));
        // Survit a la bascule (fenetre precedente).
        assert!(!f.insert_fresh(&p, t0 + Duration::from_secs(61)));
        // Expire apres deux fenetres.
        assert!(f.insert_fresh(&p, t0 + Duration::from_secs(122)));
        // Saturation : rejet silencieux.
        let mut f2 = XPrimeFilter::new(60, 2);
        assert!(f2.insert_fresh(&[1u8; 32], t0));
        assert!(f2.insert_fresh(&[2u8; 32], t0));
        assert!(!f2.insert_fresh(&[3u8; 32], t0));
    }

    #[test]
    fn filtre_uniformite_tailles_et_marqueurs() {
        // Aucun octet nul/constante entre deux handshakes et deux
        // trames : positions a comparaison naive.
        let (b_sk, b_pk) = bridge();
        let p = params();
        let now = 1_760_000_000u64;
        let xa = HiddenEph::generate();
        let xb = HiddenEph::generate();
        let (da, ca) = hs1_seal(&xa, &b_pk, b"meme-id", now, &p).unwrap();
        let (db, cb) = hs1_seal(&xb, &b_pk, b"meme-id", now, &p).unwrap();
        let mut filt = XPrimeFilter::new(p.hs_timestamp_skew_secs, p.xprime_set_max);
        let acc_a = hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &da,
            &mut filt,
            now,
            &p,
        )
        .unwrap();
        let acc_b = hs1_open(
            b_sk.crypt_x25519().as_bytes(),
            &b_pk,
            &db,
            &mut filt,
            now,
            &p,
        )
        .unwrap();
        let ya = HiddenEph::generate();
        let yb = HiddenEph::generate();
        let (_d2a, mut sba) = hs2_seal(&ya, &acc_a, now, &p);
        let (_d2b, mut sbb) = hs2_seal(&yb, &acc_b, now, &p);
        let _ = (ca, cb);
        let fa = sba_seal(&mut sba);
        let fb = sba_seal(&mut sbb);
        // Prefixes (field+debut ct) differents entre sessions.
        assert_ne!(&fa[..16], &fb[..16]);
    }

    fn sba_seal(s: &mut StealthSession) -> Vec<u8> {
        s.seal_frame(b"payload test").unwrap()
    }
}
