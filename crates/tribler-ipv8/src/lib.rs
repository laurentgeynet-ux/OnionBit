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
pub mod content_discovery;
pub mod dht;
pub mod discovery;
pub mod endpoint;
pub mod error;
pub mod overlays;
pub mod packet;
pub mod payloads;
pub mod peer;
pub mod serializer;

pub use address::UdpAddress;
pub use content_discovery::CONTENT_DISCOVERY_COMMUNITY_ID;
pub use dht::{DhtCommunity, DHT_COMMUNITY_ID};
pub use discovery::{DiscoveryCommunity, DISCOVERY_COMMUNITY_ID};
pub use endpoint::{AggregateStats, NetworkStat, UdpEndpoint};
pub use error::Ipv8Error;
pub use overlays::{OverlayInfo, OverlayPeer, OverlayStrategy};
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
        // decouverte : A envoie une introduction-request a B, B repond
        // introduction-response (les ping/pong sont non signes en
        // pyipv8 — ils ne peuvent pas verifier un pair).
        // Les deux se marquent mutuellement comme pairs verifies.
        let ep_a = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
        let ep_b = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
        let addr_a = ep_a.local_addr().unwrap();
        let addr_b = ep_b.local_addr().unwrap();

        let net_a = std::sync::Arc::new(Network::default());
        let net_b = std::sync::Arc::new(Network::default());
        let key_a = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let key_b = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();

        let lan = UdpAddress::from("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap());
        let ca = DiscoveryCommunity::new(key_a, net_a.clone(), ep_a.clone(), lan.clone()).await;
        let _cb = DiscoveryCommunity::new(key_b, net_b.clone(), ep_b.clone(), lan).await;

        let ra = tokio::spawn({
            let ep = ep_a.clone();
            async move { ep.run().await }
        });
        let _rb = tokio::spawn({
            let ep = ep_b.clone();
            async move { ep.run().await }
        });

        let dst = UdpAddress::from(addr_b);
        ca.walk_to(&dst).await.unwrap();

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
        // `234` (intro request new-style) porte `dist` comme en pyipv8.
        let raw = Packet::sign(&cid, 234, &key, 42, payload);
        let pkt = Packet::parse(&raw, Some(&cid), &crate::packet::WIRE_DISCOVERY).unwrap();
        assert_eq!(pkt.msg_id, 234);
        assert_eq!(pkt.global_time, 42);
        assert_eq!(pkt.payload, payload);
        assert_eq!(pkt.public_key_bin, key.public_key().to_bin());
    }

    #[test]
    fn paquet_signe_sans_dist_aller_retour() {
        let key = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let cid = DISCOVERY_COMMUNITY_ID;
        let payload = b"payload-de-test";
        // Messages `ez_send` (DHT, cellules…) : `auth + payload`, pas de
        // `GlobalTimeDistributionPayload`. `msg_id` 3 est signe sans
        // dist hors `DiscoveryCommunity` (en discovery c'est un ping
        // non signe — cf. `WIRE_DISCOVERY`).
        let raw = Packet::sign_no_dist(&cid, 3, &key, payload);
        let pkt = Packet::parse(&raw, Some(&cid), &crate::packet::WIRE_DEFAULT).unwrap();
        assert_eq!(pkt.msg_id, 3);
        assert_eq!(pkt.global_time, 0);
        assert_eq!(pkt.payload, payload);
        assert_eq!(pkt.public_key_bin, key.public_key().to_bin());
    }

    #[test]
    fn paquet_signature_invalide_rejetee() {
        let key = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let cid = DISCOVERY_COMMUNITY_ID;
        let mut raw = Packet::sign(&cid, 2, &key, 42, b"x");
        // Corrompt un octet du payload.
        let n = raw.len();
        raw[n - 70] ^= 0xFF;
        assert!(matches!(
            Packet::parse(&raw, Some(&cid), &crate::packet::WIRE_DISCOVERY),
            Err(Ipv8Error::InvalidSignature)
        ));
    }

    #[tokio::test]
    async fn endpoint_dual_stack_envoie_et_recoit_en_v6() {
        // `DispatcherEndpoint` pyipv8 : le socket v6 partage les
        // listeners v4, l'envoi choisit le socket par famille.
        let Ok(ep) = UdpEndpoint::bind_dual("127.0.0.1:0", Some("[::1]:0")).await else {
            // Pas de pile IPv6 sur cette machine : test inapplicable.
            return;
        };
        let v6 = ep.local_addr_v6().unwrap().unwrap();
        assert!(v6.is_ipv6());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<usize>(1);
        let mut prefix = [0xEEu8; PREFIX_LEN];
        prefix[0] = 0; // prefixe arbitraire hors communities reelles
        ep.add_raw_prefix_listener(
            prefix,
            std::sync::Arc::new(move |_src, data| {
                let _ = tx.try_send(data.len());
                Ok(())
            }),
        )
        .await;
        let run = tokio::spawn({
            let ep = ep.clone();
            async move {
                let _ = ep.run().await;
            }
        });

        let mut data = Vec::from(&prefix[..]);
        data.extend_from_slice(b"ping-v6");
        ep.send_to(&UdpAddress::from(v6), &data).await.unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
        assert_eq!(got.expect("aucun datagramme v6 recu"), Some(data.len()));
        run.abort();
    }

    #[test]
    fn paquet_prefixe_etranger_rejete() {
        let key = tribler_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let cid = DISCOVERY_COMMUNITY_ID;
        let autre = [0xAAu8; 20];
        let raw = Packet::sign(&autre, 2, &key, 42, b"x");
        assert!(Packet::parse(&raw, Some(&cid), &crate::packet::WIRE_DEFAULT).is_err());
        // Sans filtre de prefixe, le paquet est accepte.
        assert!(Packet::parse(&raw, None, &crate::packet::WIRE_DEFAULT).is_ok());
    }
}
