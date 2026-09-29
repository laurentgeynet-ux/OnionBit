//! Tests de l'etape 11 — framework de communities complet :
//! nouveau format d'introduction, adresses "walkable" introduites,
//! puncture-request -> puncture, horloge de Lamport.
//! 100 % loopback, aucun trafic sortant.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;

use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_ipv8::discovery::{DiscoveryCommunity, DISCOVERY_COMMUNITY_ID};
use tribler_ipv8::endpoint::UdpEndpoint;
use tribler_ipv8::packet::{prefix_of, Packet};
use tribler_ipv8::peer::{Network, Peer};
use tribler_ipv8::serializer::Writer;
use tribler_ipv8::UdpAddress;

/// Deadline genereuse pour les echanges loopback.
const WAIT: Duration = Duration::from_secs(5);
/// Poll loop period.
const POLL: Duration = Duration::from_millis(20);

/// Attend que `f` devienne vrai (avec deadline).
async fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while !f() && Instant::now() < deadline {
        tokio::time::sleep(POLL).await;
    }
    f()
}

/// (community, network, endpoint) sur loopback.
async fn node() -> (
    Arc<DiscoveryCommunity>,
    Arc<Network>,
    Arc<UdpEndpoint>,
    UdpAddress,
) {
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let addr = UdpAddress::from(ep.local_addr().unwrap());
    let net = Arc::new(Network::default());
    let key = LibNaClSecretKey::generate();
    let lan = UdpAddress::from("127.0.0.1:0".parse::<SocketAddr>().unwrap());
    let community = DiscoveryCommunity::new(key, net.clone(), ep.clone(), lan).await;
    let ep2 = ep.clone();
    tokio::spawn(async move {
        let _ = ep2.run().await;
    });
    (community, net, ep, addr)
}

/// L'introduction "new style" (msg 234 -> 233) verifie le pair et
/// propage le flag `new_style_intro`.
#[tokio::test(flavor = "multi_thread")]
async fn introduction_nouveau_style() {
    let (ca, net_a, _epa, _addr_a) = node().await;
    let (_cb, net_b, _epb, addr_b) = node().await;

    // Marque l'adresse de B comme "new style" cote A (introduite par
    // un pair quelconque).
    let intro_key = LibNaClSecretKey::generate();
    let introducer = Peer::new(intro_key.public_key().to_bin(), None).unwrap();
    net_a.discover_address(
        &introducer,
        addr_b.clone(),
        Some(DISCOVERY_COMMUNITY_ID),
        true,
    );
    assert!(net_a.is_new_style(&addr_b));

    ca.send_introduction_request(&addr_b).await.unwrap();

    // B doit avoir verifie A (signature du 234 OK).
    assert!(wait_for(|| !net_b.is_empty()).await, "B n'a pas verifie A");
    // A doit avoir verifie B (signature du 233 OK) avec new_style.
    assert!(
        wait_for(|| net_a
            .get_verified_by_address(&addr_b)
            .map(|p| p.new_style_intro)
            .unwrap_or(false))
        .await,
        "A n'a pas marque B comme new_style_intro"
    );
    let peer_b = net_a.get_verified_by_address(&addr_b).unwrap();
    assert!(peer_b.new_style_intro);
    // L'horloge de Lamport de B a avance (paquets signes recus).
    assert!(
        wait_for(|| _cb.global_time() > 0).await,
        "lamport de B n'a pas avance"
    );
}

/// `get_walkable_addresses` : une introduction-response contenant un
/// pair C rend l'adresse de C "walkable" cote A ; `get_new_introduction`
/// marche alors vers C sans bootstrap.
#[tokio::test(flavor = "multi_thread")]
async fn walkable_addresses_via_introduction() {
    let (ca, net_a, _epa, _addr_a) = node().await;
    let (_cb, net_b, _epb, addr_b) = node().await;
    let (cc, net_c, _epc, addr_c) = node().await;

    // B connait deja C : C envoie une introduction-request a B (les
    // ping/pong sont non signes en pyipv8 — ils ne peuvent pas
    // enregistrer un pair verifie).
    cc.send_introduction_request(&addr_b).await.unwrap();
    assert!(
        wait_for(|| net_b.peers_for_service(&DISCOVERY_COMMUNITY_ID).len() == 1).await,
        "B n'a pas enregistre C"
    );

    // A envoie une introduction-request (ancien style, loopback IPv4)
    // a B -> B introduit C.
    ca.send_introduction_request(&addr_b).await.unwrap();

    // L'adresse de C doit devenir walkable cote A.
    assert!(
        wait_for(|| !net_a
            .get_walkable_addresses(Some(&DISCOVERY_COMMUNITY_ID), false)
            .is_empty())
        .await,
        "A n'a pas appris d'adresse walkable"
    );
    let walkable = net_a.get_walkable_addresses(Some(&DISCOVERY_COMMUNITY_ID), false);
    assert!(
        walkable.iter().any(|a| a == &addr_c),
        "l'adresse de C n'est pas dans les walkables de A"
    );

    // `get_new_introduction` sans bootstrap doit marcher vers C.
    ca.get_new_introduction(&[]).await.unwrap();
    assert!(
        wait_for(|| !net_c.is_empty()).await,
        "C n'a pas recu de contact de A via la marche"
    );
}

/// Puncture-request **non signe** (msg 250) vers B avec `wan_walker`
/// = socket brut : B doit repondre par un puncture **signe** (msg 249)
/// vers cette adresse (`on_puncture_request` Python).
#[tokio::test(flavor = "multi_thread")]
async fn puncture_request_provoque_puncture() {
    // Socket brut jouant le role du demandeur de puncture.
    let target = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target_addr = UdpAddress::from(target.local_addr().unwrap());
    let lan_walker = UdpAddress::from("127.0.0.1:1".parse::<SocketAddr>().unwrap());

    let (_cb, _net_b, _epb, addr_b) = node().await;

    // Construit le puncture-request non signe (format ancien : ipv4).
    let mut w = Writer::new();
    w.ipv4(&lan_walker).unwrap();
    w.ipv4(&target_addr).unwrap();
    w.u16(7);
    let pkt = Packet::pack_unsigned(
        &DISCOVERY_COMMUNITY_ID,
        tribler_ipv8::payloads::msg::PUNCTURE_REQUEST,
        1,
        &w.into_bytes(),
    );

    let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender
        .send_to(&pkt, addr_b.to_socket_addr().unwrap())
        .await
        .unwrap();

    // Attend le puncture (msg 249) sur le socket `target`.
    let mut buf = vec![0u8; 2048];
    let (n, _src) = tokio::time::timeout(WAIT, target.recv_from(&mut buf))
        .await
        .expect("pas de puncture recu")
        .unwrap();
    let parsed = Packet::parse(&buf[..n], None, &tribler_ipv8::packet::WIRE_DISCOVERY).unwrap();
    assert_eq!(
        parsed.msg_id,
        tribler_ipv8::payloads::msg::PUNCTURE,
        "le paquet recu n'est pas un puncture"
    );
    assert!(parsed.signed, "le puncture doit etre signe");
    // Le prefixe est bien celui de la discovery community.
    assert_eq!(&buf[..22], &prefix_of(&DISCOVERY_COMMUNITY_ID));
}

/// Etape 26 — `StatisticsEndpoint` : compteurs par prefixe/`msg_id`
/// (rx + tx), `enable_community_statistics`, agregat `diff_time`.
#[tokio::test(flavor = "multi_thread")]
async fn endpoint_statistics_par_message() {
    let (_ca, _net_a, ep_a, addr_a) = node().await;
    let (_cb, _net_b, ep_b, _addr_b) = node().await;

    let prefix = prefix_of(&DISCOVERY_COMMUNITY_ID);
    ep_a.enable_community_statistics(prefix, true).await;
    ep_b.enable_community_statistics(prefix, true).await;

    // Prefixe non suivi -> aucun comptage sur l'endpoint C.
    let ep_c = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let ep_c2 = ep_c.clone();
    tokio::spawn(async move {
        let _ = ep_c2.run().await;
    });

    // A -> B : ping signe (msg 3). Le handler peut echouer — seuls
    // les compteurs de l'endpoint comptent ici.
    let mut w = Writer::new();
    w.u16(42);
    let key_a = LibNaClSecretKey::generate();
    let pkt = Packet::sign(
        &DISCOVERY_COMMUNITY_ID,
        tribler_ipv8::payloads::msg::PING,
        &key_a,
        1,
        &w.into_bytes(),
    );
    ep_a.send_to(&addr_a, &pkt) // self-send : compte tx sur A
        .await
        .unwrap();
    let ep_b_addr = {
        // B ecoute deja via sa community ; on lui envoie le meme paquet.
        _net_b.is_empty(); // silence le warning unused sur le tuple
        ep_b.local_addr().unwrap()
    };
    ep_a.send_to(&UdpAddress::from(ep_b_addr), &pkt)
        .await
        .unwrap();

    // tx sur A : 2 paquets msg 3.
    assert!(
        wait_for(|| {
            let ep = ep_a.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    ep.get_statistics(&prefix)
                        .await
                        .get(&3)
                        .map(|s| s.num_up >= 2)
                        .unwrap_or(false)
                })
            })
        })
        .await,
        "tx non compte sur A"
    );
    // rx sur A (self-send) et B : au moins 1 paquet msg 3.
    assert!(
        wait_for(|| {
            let ep = ep_b.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    ep.get_statistics(&prefix)
                        .await
                        .get(&3)
                        .map(|s| s.num_down >= 1)
                        .unwrap_or(false)
                })
            })
        })
        .await,
        "rx non compte sur B"
    );
    // Timestamps renseignes.
    let s3 = ep_a.get_statistics(&prefix).await;
    let st = s3.get(&3).unwrap();
    assert!(st.first_measured_up > 0.0 && st.last_measured_up >= st.first_measured_up);
    assert!(st.num_up >= 2 && st.bytes_up >= 2 * pkt.len() as u64);

    // Agregat : somme sur les msg_id + diff_time >= 0.
    let agg = ep_a.get_aggregate_statistics(&prefix).await;
    assert!(agg.num_up >= 2 && agg.bytes_up >= 2 * pkt.len() as u64);
    assert!(agg.diff_time >= 0.0);

    // Prefixe non suivi (C) : rien.
    let none = ep_c.get_statistics(&prefix).await;
    assert!(none.is_empty(), "prefixe non suivi ne doit pas compter");

    // Desactivation : l'entree est retiree (`statistics.pop` Python).
    ep_a.enable_community_statistics(prefix, false).await;
    assert!(ep_a.get_statistics(&prefix).await.is_empty());
    let agg = ep_a.get_aggregate_statistics(&prefix).await;
    assert_eq!(agg.num_up, 0);
}
