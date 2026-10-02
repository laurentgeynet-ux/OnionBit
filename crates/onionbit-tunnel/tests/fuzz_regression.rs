// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Harnais de robustesse des surfaces de parsing exposees aux relais
//! et pairs hostiles (threat model, fuzzing P1) : chaque parser doit
//! retourner `Ok`/`Err` — JAMAIS paniquer, quel que soit l'entree.
//!
//! Deux regimes :
//!  - vecteurs de bord deterministes (longueurs autour des offsets du
//!    format : prefixe 22, flags 27/28, msg interne 29/30, tag AEAD 16)
//!  - exploration aleatoire proptest (stable : pas besoin de nightly
//!    ni de libfuzzer — les memes surfaces sont dans `fuzz/` pour le
//!    fuzzing couverture-guidee sur une machine equipee).
//!
//! Le format impose `len > OFF_INNER_MSG_ID` avant indexation : le
//! decrypt AEAD peut retrecir une cellule a 29 octets pile — le
//! scenario de crash que ce test epingle.

use onionbit_crypto::ipv8::session::Direction;
use onionbit_ipv8::packet::{prefix_of, Packet, WIRE_DEFAULT};
use onionbit_ipv8::serializer::Reader;
use onionbit_tunnel::cell::{self, Cell};
use onionbit_tunnel::hidden_services::unpack_dht_intro_point;
use onionbit_tunnel::payload::{self as tp, msg, Cellable};
use onionbit_tunnel::TUNNEL_COMMUNITY_ID;
use proptest::prelude::*;

/// Passe `data` dans tous les parsers accessibles a un pair hostile.
/// Invariant : aucune panic — les erreurs sont le resultat attendu.
fn exercise_all(data: &[u8]) {
    // Enveloppe IPv8 signee/non signee.
    let _ = Packet::parse(data, None, &WIRE_DEFAULT);
    let _ = Packet::parse(data, Some(&TUNNEL_COMMUNITY_ID), &WIRE_DEFAULT);

    // Cellule tunnel : parse, flags, transform et crypto de couche.
    let _ = Cell::parse(data);
    let _ = cell::check_cell_flags(data, 8);
    let _ = cell::check_cell_flags(data, 0);
    let _ = cell::decrypt_cell(data, Direction::Forward, &[]);
    let _ = cell::decrypt_cell(data, Direction::Backward, &[]);
    let mut no_keys = [];
    let _ = cell::encrypt_cell(data, Direction::Forward, &mut no_keys);
    let _ = Cell::swap_circuit_id(data, 0xDEAD);

    // Payloads e2e non signes (dispatch socket brute) : le corps est
    // lu apres `prefix + msg_id` — on fuzz le corps directement.
    let mut r = Reader::new(data);
    let _ = tp::CreateE2E::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::CreatedE2E::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::PeersRequest::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::PeersResponse::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::LinkE2E::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::LinkedE2E::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::EstablishIntro::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::IntroEstablished::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::EstablishRendezvous::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::RendezvousEstablished::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::RendezvousInfo::unpack_framed(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::Data::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::Create::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::Created::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::Extend::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::Extended::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::Destroy::unpack(&mut r);
    let mut r = Reader::new(data);
    let _ = tp::TunnelPing::unpack(&mut r);

    // Valeur DHT des points d'introduction.
    let _ = unpack_dht_intro_point(data);

    // Datagramme uTP (surface exit : datagrammes recus de
    // l'exterieur sont reencapsules — le header est parse avant tout).
    let _ = librqbit_utp::raw::UtpHeader::deserialize(data);
}

/// Datagramme complet `prefix + msg_id + corps` : exercice du
/// dispatch non signe `on_packet_from_circuit` (msg ids 13/14/17/18).
fn exercise_unsigned_dispatch(msg_id: u8, body: &[u8]) {
    let prefix = prefix_of(&TUNNEL_COMMUNITY_ID);
    let mut pkt = Vec::with_capacity(prefix.len() + 1 + body.len());
    pkt.extend_from_slice(&prefix);
    pkt.push(msg_id);
    pkt.extend_from_slice(body);

    // Parse d'enveloppe (echoue -> fallback dispatch non signe).
    let _ = Packet::parse(&pkt, Some(&TUNNEL_COMMUNITY_ID), &WIRE_DEFAULT);
    let mut r = Reader::new(body);
    match msg_id {
        msg::CREATE_E2E => {
            let _ = tp::CreateE2E::unpack(&mut r);
        }
        msg::CREATED_E2E => {
            let _ = tp::CreatedE2E::unpack(&mut r);
        }
        msg::PEERS_REQUEST => {
            let _ = tp::PeersRequest::unpack(&mut r);
        }
        msg::PEERS_RESPONSE => {
            let _ = tp::PeersResponse::unpack(&mut r);
        }
        _ => {}
    }
}

/// Vecteurs deterministes : frontieres du format (offsets de flags et
/// de msg interne, taille du tag AEAD), entrees minimales, mega-
/// compteurs de listes imbriquees.
#[test]
fn frontieres_format_ne_paniquent_pas() {
    // Longueurs autour des offsets : 22 (prefixe), 23 (msg), 27/28
    // (flags), 29 (inner msg id), 30 (corps), 44/45 (corps = 1 tag
    // AEAD pile -> decrypt a vide : `check_cell_flags` sur 29 octets).
    for len in [
        0usize, 1, 21, 22, 23, 24, 27, 28, 29, 30, 31, 32, 44, 45, 46, 63, 64, 255, 256,
    ] {
        exercise_all(&vec![0u8; len]);
        exercise_all(&vec![0xFF; len]);
    }

    // Compteurs de listes adversaires : `PeersResponse` annonce 255
    // elements mais le corps est tronque — `take(size)` borne.
    let mut w = onionbit_ipv8::serializer::Writer::new();
    w.u32(1); // circuit_id
    w.u16(0); // identifier
    w.bytes(&[0xAA; 20]); // info_hash
    w.u8(255); // count annonce, aucun element ne suit
    exercise_all(&w.into_bytes());

    // `varlen_h` geant non suivi de donnees.
    let mut w = onionbit_ipv8::serializer::Writer::new();
    w.u16(1); // identifier
    w.bytes(&[0xBB; 20]); // info_hash
    w.u16(u16::MAX); // node_public_key varlen = 65535, tronque
    exercise_all(&w.into_bytes());
}

/// Regression explicite du crash trouve a l'audit : une cellule qui
/// dechiffre a pile 29 octets (sans `inner_msg_id`) ne doit pas
/// faire paniquer `check_cell_flags`.
#[test]
fn cellule_decryptee_sans_msg_interne_ne_panique_pas() {
    let vingt_neuf = vec![0u8; 29];
    assert!(cell::check_cell_flags(&vingt_neuf, 8).is_err());
    assert!(cell::decrypt_cell(&vingt_neuf, Direction::Forward, &[]).is_err());
    assert!(Cell::parse(&vingt_neuf).is_err());
}

/// Epingle DNS (threat model) : `UdpAddress::Domain` ne se resout
/// JAMAIS en `SocketAddr` — l'endpoint ne peut donc pas emettre vers
/// un nom (ni le resoudre localement) ; la resolution a lieu cote
/// exit (`send_tcp_request` sur `TcpStream::connect((host, port))`).
/// Si un jour `to_socket_addr` resolvait les domaines, le chemin UDP
/// fuiterait en DNS clair — ce test le detecterait.
#[test]
fn adresse_domaine_ne_se_resout_pas_cote_client() {
    let domain = onionbit_ipv8::UdpAddress::Domain("tracker.example".into(), 6969);
    assert!(domain.to_socket_addr().is_none());
}

proptest! {
    /// Octets arbitraires : tous les parsers, aucune panic.
    #[test]
    fn parsers_jamais_de_panic(data in proptest::collection::vec(any::<u8>(), 0..=2048)) {
        exercise_all(&data);
    }

    /// Marteau la frontiere 27..=46 (flags + inner msg id + premier
    /// tag AEAD) — la zone ou les offsets decouverts sont coupes.
    #[test]
    fn frontiere_cellule_ne_panique_pas(data in proptest::collection::vec(any::<u8>(), 27..=46)) {
        exercise_all(&data);
    }

    /// Dispatch non signe : `prefix + msg_id hostile + corps`.
    #[test]
    fn dispatch_non_signe_ne_panique_pas(
        msg_id in any::<u8>(),
        body in proptest::collection::vec(any::<u8>(), 0..=512),
    ) {
        exercise_unsigned_dispatch(msg_id, &body);
    }
}
