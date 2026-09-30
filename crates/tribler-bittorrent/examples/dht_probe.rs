//! Sonde DHT mainline brute : reproduit le `bind [::]` + bootstrap
//! par defaut de la session principale pour voir l'erreur interne
//! que `warn!("error in bootstrap: {error:#}")` masque.
//!
//! Usage : `cargo run -p tribler-bittorrent --example dht_probe`

use std::time::Duration;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "debug,librqbit_dht=trace,librqbit_dualstack_sockets=trace".into()
            }),
        )
        .init();

    let dht = librqbit::dht::DhtBuilder::new().await.expect("dht new");
    println!("dht creee, ecoute sur {:?}", dht.listen_addr());
    for _ in 0..12 {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let s = dht.stats();
        println!(
            "outstanding={} peers_v4={} peers_v6={}",
            s.outstanding_requests, s.routing_table_size, s.routing_table_size_v6
        );
        if s.routing_table_size > 0 {
            println!("DHT PEUPLEE — bootstrap fonctionnel");
            return;
        }
    }
    println!("DHT VIDE apres 120s — bootstrap en echec");
}
