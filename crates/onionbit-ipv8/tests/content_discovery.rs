// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test loopback de `ContentDiscoveryCommunity` (etape 14) :
//! echange de santes (msg 3/4), version (101/102) et select distant
//! (201/202) entre deux noeuds — 100 % loopback.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::content_discovery::{
    ContentDiscoveryCommunity, ContentDiscoverySettings, ContentProvider, HealthInfo,
    CONTENT_DISCOVERY_COMMUNITY_ID, HEALTH_REQUEST_RANDOM,
};
use onionbit_ipv8::discovery::DiscoveryCommunity;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::peer::Network;
use onionbit_ipv8::UdpAddress;

/// Deadline genereuse pour les echanges loopback.
const WAIT: Duration = Duration::from_secs(5);

/// `ContentProvider` enregistreur pour les tests.
struct MockProvider {
    /// Santes publiees en reponse aux requetes.
    healths: Vec<HealthInfo>,
    /// Santes recues (`process_health` -> infohashes nouveaux).
    received: Mutex<Vec<HealthInfo>>,
    /// Requetes select recues (json brut).
    selects: Mutex<Vec<Vec<u8>>>,
    /// Reponses select absorbees.
    responses: Mutex<Vec<Vec<u8>>>,
    /// Blob retourne par `remote_select`.
    select_blob: Vec<u8>,
}

impl ContentProvider for MockProvider {
    fn healths_for<'a>(
        &'a self,
        _request_type: u8,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<HealthInfo>> + Send + 'a>> {
        let healths = self.healths.clone();
        Box::pin(async move { healths })
    }
    fn process_health<'a>(
        &'a self,
        healths: &'a [HealthInfo],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<[u8; 20]>> + Send + 'a>> {
        let healths = healths.to_vec();
        Box::pin(async move {
            self.received.lock().unwrap().extend(healths);
            Vec::new()
        })
    }
    fn remote_select<'a>(
        &'a self,
        json: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<Vec<u8>>> + Send + 'a>> {
        let json = json.to_vec();
        let blob = self.select_blob.clone();
        Box::pin(async move {
            self.selects.lock().unwrap().push(json);
            vec![blob]
        })
    }
    fn process_select_response<'a>(
        &'a self,
        blob: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<serde_json::Value>> + Send + 'a>>
    {
        let blob = blob.to_vec();
        Box::pin(async move {
            self.responses.lock().unwrap().push(blob);
            Vec::new()
        })
    }
    fn version_info(&self) -> (String, String) {
        ("8.4.3-rust".into(), "test".into())
    }
}

/// (community, network, adresse) sur loopback.
async fn node(
    provider: Arc<dyn ContentProvider>,
) -> (Arc<ContentDiscoveryCommunity>, Arc<Network>, UdpAddress) {
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let addr = UdpAddress::from(ep.local_addr().unwrap());
    let net = Arc::new(Network::default());
    let key = LibNaClSecretKey::generate();
    // Gossip periodique desactive (intervalle tres long) : ces tests
    // isolent le chemin requete/reponse explicite (msg 3/4, 101/102,
    // 201/202) — le premier tick de `gossip_tick` se declenche
    // immediatement (tokio::time::interval) a un instant non
    // deterministe et peut emettre un `HealthPayload` legitime en
    // doublon des qu'un pair est verifie, rendant les egalites
    // strictes de compteur flaky.
    let discovery = DiscoveryCommunity::new(
        key.clone(),
        net.clone(),
        ep.clone(),
        UdpAddress::unspecified(),
    )
    .await;
    let community = ContentDiscoveryCommunity::new(
        key,
        net.clone(),
        ep.clone(),
        provider,
        ContentDiscoverySettings {
            gossip_interval: Duration::from_secs(3600),
            ..ContentDiscoverySettings::default()
        },
        discovery,
    )
    .await;
    tokio::spawn(async move {
        let _ = ep.run().await;
    });
    (community, net, addr)
}

async fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while !f() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f()
}

fn health(seed: u8) -> HealthInfo {
    HealthInfo {
        infohash: [seed; 20],
        seeders: 10,
        leechers: 2,
        last_check: 1_700_000_000,
        tracker: "udp://tracker.test:6969".into(),
    }
}

/// Requete de sante (3) -> reponse (4) avec les santes du fournisseur.
#[tokio::test(flavor = "multi_thread")]
async fn health_request_response_roundtrip() {
    tracing_subscriber::fmt()
        .with_env_filter("onionbit_ipv8=trace")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    let pa = Arc::new(MockProvider {
        healths: vec![],
        received: Mutex::new(vec![]),
        selects: Mutex::new(vec![]),
        responses: Mutex::new(vec![]),
        select_blob: vec![],
    });
    let pb = Arc::new(MockProvider {
        healths: vec![health(0x11), health(0x22)],
        received: Mutex::new(vec![]),
        selects: Mutex::new(vec![]),
        responses: Mutex::new(vec![]),
        select_blob: vec![],
    });
    let (ca, _na, _aa) = node(pa.clone() as Arc<dyn ContentProvider>).await;
    let (_cb, _nb, addr_b) = node(pb.clone() as Arc<dyn ContentProvider>).await;

    eprintln!("DIAG addr_b={:?}", addr_b);
    ca.request_health(&addr_b, HEALTH_REQUEST_RANDOM)
        .await
        .unwrap();
    let ok = wait_for(|| pa.received.lock().unwrap().len() == 2).await;
    assert!(ok, "santes non recues");
    assert_eq!(pa.received.lock().unwrap()[0].seeders, 10);
}

/// Select distant (201) -> reponse (202) absorbee par le requeteur.
#[tokio::test(flavor = "multi_thread")]
async fn remote_select_roundtrip() {
    let pa = Arc::new(MockProvider {
        healths: vec![],
        received: Mutex::new(vec![]),
        selects: Mutex::new(vec![]),
        responses: Mutex::new(vec![]),
        select_blob: vec![],
    });
    let pb = Arc::new(MockProvider {
        healths: vec![],
        received: Mutex::new(vec![]),
        selects: Mutex::new(vec![]),
        responses: Mutex::new(vec![]),
        select_blob: b"mdblob-archive".to_vec(),
    });
    let (ca, _na, _aa) = node(pa.clone() as Arc<dyn ContentProvider>).await;
    let (_cb, _nb, addr_b) = node(pb.clone() as Arc<dyn ContentProvider>).await;

    let id = ca
        .send_remote_select(&addr_b, b"{\"infohash\":\"aa\"}".to_vec())
        .await
        .unwrap();
    let ok = wait_for(|| !pb.selects.lock().unwrap().is_empty()).await;
    assert!(ok, "select non recu par B");
    let ok = wait_for(|| !pa.responses.lock().unwrap().is_empty()).await;
    assert!(ok, "reponse select non recue par A");
    assert_eq!(pa.responses.lock().unwrap()[0], b"mdblob-archive");
    let _ = id;
}

/// `processing_callback` (`send_search_request` Python) : appelee
/// avec les objets nouveaux de la reponse — base de
/// `remote_query_results`.
#[tokio::test(flavor = "multi_thread")]
async fn remote_select_callback_invoked() {
    let pa = Arc::new(MockProvider {
        healths: vec![],
        received: Mutex::new(vec![]),
        selects: Mutex::new(vec![]),
        responses: Mutex::new(vec![]),
        select_blob: vec![],
    });
    let pb = Arc::new(MockProvider {
        healths: vec![],
        received: Mutex::new(vec![]),
        selects: Mutex::new(vec![]),
        responses: Mutex::new(vec![]),
        select_blob: b"payload".to_vec(),
    });
    let (ca, _na, _aa) = node(pa.clone() as Arc<dyn ContentProvider>).await;
    let (_cb, _nb, addr_b) = node(pb.clone() as Arc<dyn ContentProvider>).await;

    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_c = called.clone();
    ca.send_remote_select_cb(
        &addr_b,
        b"mid-b",
        b"{\"txt_filter\":\"x\"}".to_vec(),
        Arc::new(move |_mid, results| {
            assert!(results.is_empty());
            called_c.store(true, std::sync::atomic::Ordering::SeqCst);
        }),
    )
    .await
    .unwrap();
    let ok = wait_for(|| called.load(std::sync::atomic::Ordering::SeqCst)).await;
    assert!(ok, "processing_callback non invoque");
}

/// `RandomWalk` : une `introduction-request` sous le prefixe de la
/// community peuple l'overlay des deux cotes — le demandeur marque le
/// repondant via sa `introduction-response`, le repondant marque le
/// demandeur a la reception de la requete. Sans cette marche,
/// `peers_for_service` reste vide et la recherche distante est muette.
#[tokio::test(flavor = "multi_thread")]
async fn walk_decouvre_les_pairs_de_l_overlay() {
    let empty = || {
        Arc::new(MockProvider {
            healths: vec![],
            received: Mutex::new(vec![]),
            selects: Mutex::new(vec![]),
            responses: Mutex::new(vec![]),
            select_blob: vec![],
        }) as Arc<dyn ContentProvider>
    };
    let (ca, na, addr_a) = node(empty()).await;
    let (_cb, nb, addr_b) = node(empty()).await;

    // Equivalent d'un `step()` vers un pair de bootstrap connu.
    ca.walk_to(&addr_b).await.unwrap();
    let na2 = na.clone();
    let nb2 = nb.clone();
    let ok = wait_for(move || {
        !na2.peers_for_service(&CONTENT_DISCOVERY_COMMUNITY_ID)
            .is_empty()
            && !nb2
                .peers_for_service(&CONTENT_DISCOVERY_COMMUNITY_ID)
                .is_empty()
    })
    .await;
    assert!(ok, "la marche n'a pas peuple l'overlay content-discovery");

    // `step()` lui-meme : bootstrap = l'adresse de A, l'overlay de B
    // reste peuple apres une etape (A deja connu -> cible atteinte ?
    // non : B n'a que A, donc B marcherait — ici on verifie juste que
    // step n'erre pas et que A reste joignable sous le service).
    let peers = na.peers_for_service(&CONTENT_DISCOVERY_COMMUNITY_ID);
    assert_eq!(peers.len(), 1);
    assert!(peers[0].address.as_ref() == Some(&addr_b));
    let _ = addr_a;
}

/// Version request (101) -> response (102) : le pair distant repond
/// avec ses chaines (verifie via le provider de B sollicite).
#[tokio::test(flavor = "multi_thread")]
async fn version_request_answered() {
    struct V {
        calls: Arc<Mutex<u32>>,
    }
    impl ContentProvider for V {
        fn healths_for<'a>(
            &'a self,
            _t: u8,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<HealthInfo>> + Send + 'a>>
        {
            Box::pin(async move { vec![] })
        }
        fn process_health<'a>(
            &'a self,
            _h: &'a [HealthInfo],
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<[u8; 20]>> + Send + 'a>>
        {
            Box::pin(async move { vec![] })
        }
        fn remote_select<'a>(
            &'a self,
            _j: &'a [u8],
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<Vec<u8>>> + Send + 'a>>
        {
            Box::pin(async move { vec![] })
        }
        fn process_select_response<'a>(
            &'a self,
            _b: &'a [u8],
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<serde_json::Value>> + Send + 'a>>
        {
            Box::pin(async move { Vec::new() })
        }
        fn version_info(&self) -> (String, String) {
            *self.calls.lock().unwrap() += 1;
            ("8.4.3-rust".into(), "win64".into())
        }
    }
    let calls = Arc::new(Mutex::new(0u32));
    let pa = Arc::new(MockProvider {
        healths: vec![],
        received: Mutex::new(vec![]),
        selects: Mutex::new(vec![]),
        responses: Mutex::new(vec![]),
        select_blob: vec![],
    });
    let (ca, _na, _aa) = node(pa.clone() as Arc<dyn ContentProvider>).await;
    let (_cb, _nb, addr_b) = node(Arc::new(V {
        calls: calls.clone(),
    }) as Arc<dyn ContentProvider>)
    .await;

    ca.send_version_request(&addr_b).await.unwrap();
    let ok = wait_for(|| *calls.lock().unwrap() > 0).await;
    assert!(ok, "version_info jamais appele sur B");
}
