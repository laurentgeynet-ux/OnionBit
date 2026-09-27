//! `tribler-daemon` — binaire principal du daemon Tribler-Rust-Torrent.
//!
//! Point d'entree du processus : charge la configuration, initialise le
//! logging (`tracing`), assemble les composants d'infrastructure
//! (`tribler-bittorrent`, `tribler-ipv8`, `tribler-tunnel`, `tribler-db`)
//! derriere les traits de `tribler-core`, puis demarre `tribler-api` pour
//! exposer le plan de controle local. Aucun client (CLI ou future UI
//! Flutter) ne parle a autre chose qu'a `tribler-api`.
//!
//! Etat : squelette (etape 0). Assemblage reel a l'etape 8
//! ("Premier daemon executable de bout en bout : BitTorrent + API + DB,
//! sans IPv8").

fn main() {
    println!(
        "tribler-daemon : squelette (etape 0). Voir docs/plans/roadmap.md pour l'etat d'avancement."
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn le_squelette_compile() {
        assert_eq!(2 + 2, 4);
    }
}
