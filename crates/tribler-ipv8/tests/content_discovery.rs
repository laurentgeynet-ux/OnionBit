//! Test loopback de `ContentDiscoveryCommunity` (etape 14) :
//! echange de santes (msg 3/4), version (101/102) et select distant
//! (201/202) entre deux noeuds — 100 % loopback.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_ipv8::content_discovery::{
    ContentDiscoveryCommunity, ContentProvider, HealthInfo, HEALTH_REQUEST_RANDOM,
};
use tribler_ipv8::endpoint::UdpEndpoint;
use tribler_ipv8::peer::Network;
use tribler_ipv8::UdpAddress;

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
    fn healths_for(&self, _request_type: u8) -> Vec<HealthInfo> {
        self.healths.clone()
    }
    fn process_health(&self, healths: &[HealthInfo]) -> Vec<[u8; 20]> {
        self.received
            .lock()
            .unwrap()
            .extend(healths.iter().cloned());
        Vec::new()
    }
    fn remote_select(&self, json: &[u8]) -> Vec<u8> {
        self.selects.lock().unwrap().push(json.to_vec());
        self.select_blob.clone()
    }
    fn process_select_response(&self, blob: &[u8]) {
        self.responses.lock().unwrap().push(blob.to_vec());
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
    let community = ContentDiscoveryCommunity::new(
        key,
        net.clone(),
        ep.clone(),
        provider,
        Some(Duration::from_secs(3600)),
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
        .with_env_filter("tribler_ipv8=trace")
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

/// Version request (101) -> response (102) : le pair distant repond
/// avec ses chaines (verifie via le provider de B sollicite).
#[tokio::test(flavor = "multi_thread")]
async fn version_request_answered() {
    struct V {
        calls: Arc<Mutex<u32>>,
    }
    impl ContentProvider for V {
        fn healths_for(&self, _t: u8) -> Vec<HealthInfo> {
            vec![]
        }
        fn process_health(&self, _h: &[HealthInfo]) -> Vec<[u8; 20]> {
            vec![]
        }
        fn remote_select(&self, _j: &[u8]) -> Vec<u8> {
            vec![]
        }
        fn process_select_response(&self, _b: &[u8]) {}
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
