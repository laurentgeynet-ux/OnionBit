//! Politique des noeuds de sortie — port de `DataChecker` et
//! `TunnelExitSocket.is_allowed` (pyipv8 `messaging/anonymization/
//! exit_socket.py`).
//!
//! Un noeud de sortie ne doit relayer QUE du trafic reconnaissable
//! comme BitTorrent (uTP, tracker UDP, DHT bencode) ou IPv8, et
//! uniquement si le flag de sortie correspondant est annonce. C'est
//! ce qui empeche un circuit d'etre detourne en proxy UDP generique.

/// `PEER_FLAG_RELAY` : le pair accepte de relayer.
pub const PEER_FLAG_RELAY: i32 = 1;
/// `PEER_FLAG_EXIT_BT` : le pair accepte de sortir du trafic BT.
pub const PEER_FLAG_EXIT_BT: i32 = 2;
/// `PEER_FLAG_EXIT_IPV8`.
pub const PEER_FLAG_EXIT_IPV8: i32 = 4;
/// `PEER_FLAG_SPEED_TEST`.
pub const PEER_FLAG_SPEED_TEST: i32 = 8;
/// `PEER_FLAG_EXIT_HTTP` (`ipv8-rust-tunnels` `PeerFlag::ExitHttp`) :
/// le pair accepte de sortir du trafic HTTP (requetes tracker).
pub const PEER_FLAG_EXIT_HTTP: i32 = 32768;

/// `DataChecker.could_be_utp` : en-tete uTP BEP-29 plausible
/// (>=20 octets, type 0..4, version 1, extension 0..3).
pub fn could_be_utp(data: &[u8]) -> bool {
    if data.len() < 20 {
        return false;
    }
    let b1 = data[0];
    let b2 = data[1];
    (b1 >> 4) <= 4 && (b1 & 0x0F) == 1 && b2 <= 3
}

/// `DataChecker.could_be_udp_tracker` : champ `action` 0..3 en
/// position 0 ou 8 (protocole tracker UDP).
pub fn could_be_udp_tracker(data: &[u8]) -> bool {
    let at = |off: usize| -> Option<u32> {
        data.get(off..off + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    (data.len() >= 8 && at(0).is_some_and(|a| a <= 3))
        || (data.len() >= 12 && at(8).is_some_and(|a| a <= 3))
}

/// `DataChecker.could_be_dht` : dictionnaire bencode `d…e`.
pub fn could_be_dht(data: &[u8]) -> bool {
    data.len() > 1 && data[0] == b'd' && *data.last().unwrap() == b'e'
}

/// `DataChecker.could_be_bt` : uTP, tracker UDP ou DHT.
pub fn could_be_bt(data: &[u8]) -> bool {
    could_be_utp(data) || could_be_udp_tracker(data) || could_be_dht(data)
}

/// `DataChecker.could_be_ipv8` : prefixe IPv8 plausible (>=23 octets,
/// magic 0x00, version 0x01/0x02).
pub fn could_be_ipv8(data: &[u8]) -> bool {
    data.len() >= 23 && data[0] == 0x00 && (data[1] == 0x01 || data[1] == 0x02)
}

/// `TunnelExitSocket.is_allowed` : une donnee ne sort que si elle
/// ressemble a du BT **et** `PEER_FLAG_EXIT_BT` est annonce, ou a de
/// l'IPv8 **et** `PEER_FLAG_EXIT_IPV8` est annonce, ou a de l'IPv8
/// portant le prefixe de NOTRE community (`data[:22] == prefix`).
pub fn is_exit_data_allowed(data: &[u8], peer_flags: i32, community_prefix: &[u8]) -> bool {
    let is_bt = could_be_bt(data);
    let is_ipv8 = could_be_ipv8(data);
    (is_bt && peer_flags & PEER_FLAG_EXIT_BT != 0)
        || (is_ipv8 && peer_flags & PEER_FLAG_EXIT_IPV8 != 0)
        || (is_ipv8 && data.len() >= 22 && data[..22] == community_prefix[..22])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Datagramme uTP ST_SYN valide (type 4, version 1, ext 0).
    fn utp() -> Vec<u8> {
        let mut p = vec![0x41, 0x00];
        p.extend_from_slice(&[0; 18]);
        p
    }

    #[test]
    fn classification_fidele_au_python() {
        assert!(could_be_utp(&utp()));
        assert!(!could_be_utp(&[0; 10])); // trop court
        assert!(!could_be_utp(&{
            let mut p = utp();
            p[0] = 0x42;
            p
        })); // version != 1
        assert!(could_be_dht(b"d1:ai1ee"));
        assert!(!could_be_dht(b"xturndata"));
        assert!(could_be_udp_tracker(&[0, 0, 0, 2, 0, 0, 0, 0]));
        let mut ipv8 = vec![0u8; 30];
        ipv8[1] = 0x02;
        assert!(could_be_ipv8(&ipv8));
    }

    #[test]
    fn is_allowed_exige_le_flag_correspondant() {
        let prefix = [0u8; 22];
        // uTP accepte seulement avec EXIT_BT.
        assert!(!is_exit_data_allowed(&utp(), PEER_FLAG_RELAY, &prefix));
        assert!(is_exit_data_allowed(&utp(), PEER_FLAG_EXIT_BT, &prefix));
        // IPv8 du meme prefixe : toujours accepte.
        let mut mine = vec![0u8; 30];
        mine[1] = 0x02;
        assert!(is_exit_data_allowed(&mine, 0, &mine[..22]));
        assert!(!is_exit_data_allowed(&mine, 0, &prefix));
        // Donnee quelconque : refusee.
        assert!(!is_exit_data_allowed(
            b"hello world",
            PEER_FLAG_EXIT_BT,
            &prefix
        ));
    }
}
