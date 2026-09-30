//! Utilitaire de banc : cree un petit fichier + son `.torrent`.
//!
//! Usage : `mk_torrent <dossier_sortie> <taille_octets> [nonce]`
//! Ecrit `<dossier>/donnee.bin` (contenu pseudo-aleatoire ; `nonce`
//! distinct => infohash distinct — indispensable aux tests live pour
//! que la cle DHT du swarm ne recycle pas d'annonces de points
//! d'introduction perimees d'un run precedent) et
//! `<dossier>/test.torrent`, puis affiche `infohash=<hex>` sur stdout.

use std::path::PathBuf;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(
        args.next()
            .expect("usage: mk_torrent <dir> <bytes> [nonce]"),
    );
    let bytes: u64 = args
        .next()
        .expect("usage: mk_torrent <dir> <bytes> [nonce]")
        .parse()
        .expect("taille invalide");
    let nonce: u64 = args
        .next()
        .map(|s| s.parse().expect("nonce invalide"))
        .unwrap_or(0);
    std::fs::create_dir_all(&dir).expect("creation du dossier");

    // Contenu deterministe module le nonce (verification possible des
    // deux cotes : le `.torrent` embarque les vrais hash de pieces).
    let nb = nonce.to_le_bytes();
    let payload: Vec<u8> = (0..bytes)
        .map(|i| ((i % 251) as u8) ^ nb[(i % 8) as usize])
        .collect();
    std::fs::write(dir.join("donnee.bin"), &payload).expect("ecriture payload");

    let torrent = librqbit::create_torrent(
        &dir.join("donnee.bin"),
        librqbit::CreateTorrentOptions {
            piece_length: Some(16384),
            ..Default::default()
        },
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .expect("create_torrent");
    std::fs::write(dir.join("test.torrent"), torrent.as_bytes().unwrap())
        .expect("ecriture torrent");
    println!("infohash={}", torrent.info_hash().as_string());
}
