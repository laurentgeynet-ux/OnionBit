//! Rejeu de paquets enregistres lors de l'echange reel
//! Rust <-> pyipv8 (`scripts/interop_ipv8.ps1`, artefacts dans
//! `target/interop/`). Ces fixtures sont des paquets filaires
//! **produits par le vrai pyipv8** (ou par notre noeud et verifies par
//! lui) — le test prouve que notre parseur/verifier Ed25519 les
//! accepte, de facon permanente dans la CI.
//!
//! Provenance (commit pyipv8, sens, msg_ids, regeneration) :
//! `tests/fixtures/README.md`.

use onionbit_ipv8::discovery::DISCOVERY_COMMUNITY_ID;
use onionbit_ipv8::packet::Packet;

/// Paquets produits par pyipv8 (TX du noeud Python, recus par Rust).
const PYIPV8_PACKETS: &str = include_str!("fixtures/pyipv8_discovery.hex");
/// Paquets produits par notre noeud Rust (verifies cote Python).
const RUST_PACKETS: &str = include_str!("fixtures/rust_discovery.hex");

fn parse_fixture(src: &str) -> Vec<Vec<u8>> {
    src.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| hex::decode(l.trim()).unwrap())
        .collect()
}

#[test]
fn rejoue_paquets_pyipv8() {
    let packets = parse_fixture(PYIPV8_PACKETS);
    assert!(packets.len() >= 4, "fixture incomplete");
    let mut seen_intro_req = false;
    let mut seen_intro_resp = false;
    let mut seen_sim_req = false;
    for p in &packets {
        let pkt = Packet::parse(
            p,
            Some(&DISCOVERY_COMMUNITY_ID),
            &onionbit_ipv8::packet::WIRE_DISCOVERY,
        )
        .expect("paquet pyipv8 rejete par le parseur Rust");
        assert!(pkt.signed, "paquet pyipv8 non signe ?");
        match pkt.msg_id {
            246 => seen_intro_req = true,
            245 => seen_intro_resp = true,
            1 => seen_sim_req = true,
            _ => {}
        }
    }
    assert!(seen_intro_req, "pas d'introduction-request pyipv8");
    assert!(seen_intro_resp, "pas d'introduction-response pyipv8");
    assert!(seen_sim_req, "pas de similarity-request pyipv8");
}

#[test]
fn rejoue_paquets_rust_verifies_par_pyipv8() {
    // Meme paquets que le vrai pyipv8 a deja acceptes en direct
    // (signature verifiee par verify_packets.py) — on re-verifie ici
    // que le parseur Rust les accepte aussi.
    let packets = parse_fixture(RUST_PACKETS);
    assert!(packets.len() >= 4, "fixture incomplete");
    for p in &packets {
        Packet::parse(
            p,
            Some(&DISCOVERY_COMMUNITY_ID),
            &onionbit_ipv8::packet::WIRE_DISCOVERY,
        )
        .expect("paquet Rust rejete par notre parseur");
    }
}
