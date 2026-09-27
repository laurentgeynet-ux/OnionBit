//! `tribler-cli` — outil de pilotage en ligne de commande.
//!
//! Client du plan de controle expose par `tribler-daemon` via
//! `tribler-api` (HTTP/WebSocket sur `127.0.0.1`). Ne contient aucune
//! logique metier : traduit des sous-commandes CLI en appels REST, a
//! l'image de `mule-cli` dans le projet eMule-Rust.
//!
//! Etat : squelette (etape 0). Implementation a l'etape 7
//! ("CLI de pilotage minimal : status/list/add/remove").

fn main() {
    println!("tribler-cli : squelette (etape 0). Aucune commande implementee pour l'instant.");
}

#[cfg(test)]
mod tests {
    #[test]
    fn le_squelette_compile() {
        assert_eq!(2 + 2, 4);
    }
}
