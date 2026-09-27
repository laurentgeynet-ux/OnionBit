//! `tribler-ipv8` — moteur overlay IPv8.
//!
//! Portage du coeur du protocole IPv8 (`pyipv8`) : c'est le composant le
//! **plus a risque** du projet (cf. `docs/plans/plan_faisabilite.md`,
//! section "Analyse de risques"), car aucune implementation Rust complete
//! n'existe a ce jour (`ipv8-rust-tunnels` ne couvre que le plan de
//! donnees des tunnels, pas le protocole overlay).
//!
//! Responsabilite unique :
//!
//! - encodage/decodage des messages IPv8 (format binaire `struct`-like de
//!   pyipv8, cf. `ipv8/messaging/`) ;
//! - decouverte de pairs (`ipv8/peerdiscovery/`) : bootstrap via serveurs
//!   connus, marche aleatoire, gestion du churn ;
//! - framework de "communities" (groupes logiques de pairs partageant un
//!   protocole applicatif, ex. `TunnelCommunity`, `DiscoveryCommunity`) ;
//! - DHT overlay IPv8 (distinct du DHT mainline BitTorrent BEP 5) ;
//! - signature/verification Ed25519 de chaque message.
//!
//! Ce crate ne connait ni le protocole BitTorrent (`tribler-bittorrent`)
//! ni les circuits d'anonymisation (`tribler-tunnel`, qui consomme ce
//! crate comme fondation).
//!
//! Etat : etape 9 — format filaire (serialiseur + paquets signes) et
//! `DiscoveryCommunity` minimale (ping/pong, similarity, introduction).
//! Les etapes 10-11 ajoutent le DHT overlay et le framework complet.
//!
//! Reference de verite protocolaire : `D:\Projet\Tribler_sources\tribler\
//! pyipv8` (voir `docs/reference_tribler/`).

pub mod address;
pub mod dht;
pub mod discovery;
pub mod endpoint;
pub mod error;
pub mod packet;
pub mod payloads;
pub mod peer;
pub mod serializer;

pub use address::UdpAddress;
pub use dht::{DhtCommunity, DHT_COMMUNITY_ID};
pub use discovery::{DiscoveryCommunity, DISCOVERY_COMMUNITY_ID};
pub use endpoint::UdpEndpoint;
pub use error::Ipv8Error;
pub use packet::{prefix_of, Packet, PREFIX_LEN, PROTOCOL_VERSION};
pub use peer::{Network, Peer};

/// Identifiant de community IPv8 (equivalent du `community_id`
/// pyipv8, 20 octets de hash de cle publique).
pub type CommunityId = [u8; 20];

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deux_noeuds_se_decouvrent_en_loopback() {
        // Deux endpoints UDP sur 127.0.0.1, deux communities de
        // decouverte : A envoie un ping a B, B repond pong ; A envoie
        // une introduction-request, B repond introduction-response.
        // Les deux se marquent mutuellement comme pairs verifies.
        let ep_a = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
        let ep_b = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
        let addr_a = ep_a.local_addr().unwrap();
        let addr_b = ep_b.local_addr().unwrap();

        let net_a = std::sync::Arc::new(Network::default());
        let net_b = std::sync::Arc::new(Network::default());
        let key_a = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let key_b = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();

        let ca = DiscoveryCommunity::new(key_a, net_a.clone(), ep_a.clone()).await;
        let _cb = DiscoveryCommunity::new(key_b, net_b.clone(), ep_b.clone()).await;

        let ra = tokio::spawn({
            let ep = ep_a.clone();
            async move { ep.run().await }
        });
        let _rb = tokio::spawn({
            let ep = ep_b.clone();
            async move { ep.run().await }
        });

        let dst = UdpAddress::from(addr_b);
        ca.send_ping(&dst).await.unwrap();

        // Attend que B ait verifie le pair A (et inversement via le pong).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while net_b.is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(net_b.len(), 1, "B n'a pas enregistre le pair A");

        while net_a.is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(net_a.len(), 1, "A n'a pas enregistre le pair B (pong)");

        let _ = (addr_a, ra);
    }

    #[test]
    fn paquet_signe_aller_retour() {
        let key = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let cid = DISCOVERY_COMMUNITY_ID;
        let payload = b"payload-de-test";
        let raw = Packet::sign(&cid, 3, &key, 42, payload);
        let pkt = Packet::parse(&raw, Some(&cid)).unwrap();
        assert_eq!(pkt.msg_id, 3);
        assert_eq!(pkt.global_time, 42);
        assert_eq!(pkt.payload, payload);
        assert_eq!(pkt.public_key_bin, key.public_key().to_bin());
    }

    #[test]
    fn paquet_signature_invalide_rejetee() {
        let key = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let cid = DISCOVERY_COMMUNITY_ID;
        let mut raw = Packet::sign(&cid, 3, &key, 42, b"x");
        // Corrompt un octet du payload.
        let n = raw.len();
        raw[n - 70] ^= 0xFF;
        assert!(matches!(
            Packet::parse(&raw, Some(&cid)),
            Err(Ipv8Error::InvalidSignature)
        ));
    }

    #[test]
    fn paquet_prefixe_etranger_rejete() {
        let key = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let cid = DISCOVERY_COMMUNITY_ID;
        let autre = [0xAAu8; 20];
        let raw = Packet::sign(&autre, 3, &key, 42, b"x");
        assert!(Packet::parse(&raw, Some(&cid)).is_err());
        // Sans filtre de prefixe, le paquet est accepte.
        assert!(Packet::parse(&raw, None).is_ok());
    }
}
