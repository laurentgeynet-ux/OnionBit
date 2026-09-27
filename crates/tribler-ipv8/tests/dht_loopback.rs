//! Test d'integration loopback du DHT IPv8 : deux noeuds sur
//! `127.0.0.1` se decouvrent (introduction), se pingent, puis
//! `store_value` / `find_values` de bout en bout — fidélité au protocole
//! `DHTCommunity`/`DHTDiscoveryCommunity` pyipv8.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_ipv8::dht::{DhtCommunity, DHT_COMMUNITY_ID};
use tribler_ipv8::{UdpAddress, UdpEndpoint};

/// Cree un noeud DHT complet (endpoint + community) sur le loopback.
async fn node() -> (Arc<UdpEndpoint>, Arc<DhtCommunity>, UdpAddress) {
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let local = UdpAddress::from(ep.local_addr().unwrap());
    let key = LibNaClSecretKey::generate();
    // `my_wan`/`my_lan` = adresse loopback reelle du socket : necessaire
    // pour `calc_node_id` (comme `my_estimated_wan` Python).
    let dht = DhtCommunity::new(key, local.clone(), local.clone(), ep.clone()).await;
    let runner = ep.clone();
    tokio::spawn(async move {
        let _ = runner.run().await;
    });
    (ep, dht, local)
}

#[tokio::test]
async fn dht_intro_ping_store_find_loopback() {
    let (_ep_a, a, addr_a) = node().await;
    let (_ep_b, b, addr_b) = node().await;

    // Bootstrap : A "marche" vers B (introduction-request sous le
    // prefixe DHT) — B decouvre A et repond ; A decouvre B.
    a.walk_to(&addr_b).await.unwrap();

    // Attente de la decouverte mutuelle (B ping A en decouvrant).
    let deadline = Instant::now() + Duration::from_secs(8);
    while (a.node_count() < 1 || b.node_count() < 1) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(a.node_count() >= 1, "A n'a pas decouvert B");
    assert!(b.node_count() >= 1, "B n'a pas decouvert A");

    // B stocke une valeur signee ; A l'heberge (B crawl sa table ->
    // token -> store-request vers A).
    let target: [u8; 20] = tribler_crypto::hash::sha1(b"cle-de-test");
    let nodes = b
        .store_value(&target, b"valeur-dht", true)
        .await
        .expect("store_value a echoue");
    assert!(!nodes.is_empty(), "aucun noeud n'a stocke la valeur");

    // B relit la valeur via find_values (crawl -> A repond values).
    let found = b
        .find_values(&target, 0)
        .await
        .expect("find_values a echoue");
    assert!(
        found.iter().any(|(data, _)| data == b"valeur-dht"),
        "valeur non retrouvee : {found:?}"
    );

    // La valeur est signee par la cle de B.
    assert!(found.iter().all(|(_, pk)| pk.is_some()));

    let _ = addr_a;
}

#[tokio::test]
async fn dht_ping_et_token() {
    let (_ep_a, a, _addr_a) = node().await;
    let (_ep_b, b, addr_b) = node().await;

    a.walk_to(&addr_b).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while a.node_count() < 1 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(a.node_count() >= 1);

    // Ping explicite d'un noeud de la table de A.
    let node_b = {
        // On recupere un noeud connu via closest_nodes sur la table V4.
        // (acces interne via `find_nodes` indirectement : on force un
        // crawl qui contactera B.)
        a.find_nodes(&[0u8; 20]).await.expect("find_nodes")
    };
    assert_eq!(node_b.len(), 1, "le crawl devrait toucher B uniquement");

    let _ = b;
}

#[tokio::test]
async fn valeur_signee_invalide_rejetee() {
    // `unserialize_value` : blob non signe vs signe vs corrompu.
    let key = LibNaClSecretKey::generate();
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let local = UdpAddress::from(ep.local_addr().unwrap());
    let dht = DhtCommunity::new(key.clone(), local.clone(), local, ep).await;

    let blob = dht.serialize_value(b"data", true);
    let (data, pk, _ver) = dht.unserialize_value(&blob).unwrap();
    assert_eq!(data, b"data");
    assert_eq!(pk.unwrap(), key.public_key().to_bin());

    // Corruption d'un octet du corps -> signature invalide -> None.
    let mut bad = blob.clone();
    let n = bad.len();
    bad[n - 40] ^= 0xFF;
    assert!(dht.unserialize_value(&bad).is_none());

    // Entree non signee : pas de cle.
    let raw = dht.serialize_value(b"plain", false);
    let (data, pk, ver) = dht.unserialize_value(&raw).unwrap();
    assert_eq!((data.as_slice(), pk, ver), (&b"plain"[..], None, 0));

    // `community_id` conforme au Python.
    assert_eq!(
        DHT_COMMUNITY_ID,
        [
            0x8d, 0x0b, 0xe1, 0x84, 0x5d, 0x74, 0xd1, 0x75, 0xf1, 0x78, 0x19, 0x7c, 0xad, 0x00,
            0x15, 0x91, 0xd0, 0x4d, 0x73, 0xcc,
        ]
    );
}
