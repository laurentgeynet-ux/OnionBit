//! Cellule de circuit (`CellPayload` pyipv8) : format filaire des
//! messages de tunnel.
//!
//! Une cellule sur le fil (cf. `CellPayload.to_bin`) :
//!
//! ```text
//! prefix(22) | msg_id=0 (1) | circuit_id u32BE (4) | plaintext (1) |
//! relay_early (1) | message (msg_id interne + payload)
//! ```
//!
//! L'absence de signature : les cellules ne sont **pas** des paquets
//! IPv8 signes (le `msg_id` a l'offset 22 vaut 0, ce qui les distingue
//! des paquets normaux). Le chiffrement par couche s'applique a
//! `message` (offset 29+) sauf pour `NO_CRYPTO_PACKETS`.

use tribler_crypto::ipv8::session::SessionKeys;
use tribler_ipv8::Ipv8Error;

/// `msg_id` des cellules (toujours 0 — `CellPayload.msg_id`).
pub const CELL_MSG_ID: u8 = 0;
/// Position du flag `plaintext`.
const OFF_PLAINTEXT: usize = 27;
/// Position du flag `relay_early`.
const OFF_RELAY_EARLY: usize = 28;
/// Position du `msg_id` interne.
const OFF_INNER_MSG_ID: usize = 29;
/// Position du corps du message interne.
const OFF_MSG: usize = 30;

/// `NO_CRYPTO_PACKETS` pyipv8 + Rust tunnels : create(2), created(3),
/// http-request(31), http-response(33).
pub const NO_CRYPTO_PACKETS: &[u8] = &[2, 3, 31, 33];

/// `true` si `packet` ressemble a une cellule pour ce prefixe
/// (`is_cell` des tunnels Rust : prefixe + `packet[22] == 0`).
pub fn is_cell(prefix: &[u8; 22], packet: &[u8]) -> bool {
    packet.len() > OFF_MSG && packet.starts_with(prefix) && packet[22] == 0
}

/// Cellule decodee.
#[derive(Debug)]
pub struct Cell {
    /// Identifiant de circuit.
    pub circuit_id: u32,
    /// `msg_id` du message interne.
    pub inner_msg_id: u8,
    /// Corps du message interne (avant crypto de couche).
    pub message: Vec<u8>,
    /// Flag plaintext (pas de crypto de couche).
    pub plaintext: bool,
    /// Flag relay-early.
    pub relay_early: bool,
}

impl Cell {
    /// `CellPayload.from_bin`.
    pub fn parse(cell: &[u8]) -> Result<Self, Ipv8Error> {
        if cell.len() <= OFF_MSG {
            return Err(Ipv8Error::Truncated {
                need: OFF_MSG + 1,
                have: cell.len(),
            });
        }
        let circuit_id = u32::from_be_bytes(cell[23..27].try_into().unwrap());
        let plaintext = cell[OFF_PLAINTEXT] != 0;
        let relay_early = cell[OFF_RELAY_EARLY] != 0;
        let inner_msg_id = cell[OFF_INNER_MSG_ID];
        let message = cell[OFF_MSG..].to_vec();
        Ok(Self {
            circuit_id,
            inner_msg_id,
            message,
            plaintext,
            relay_early,
        })
    }

    /// `to_bin` Python — construit la cellule sur le fil.
    pub fn to_wire(
        prefix: &[u8; 22],
        circuit_id: u32,
        inner_msg_id: u8,
        message: &[u8],
        plaintext: bool,
        relay_early: bool,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(OFF_MSG + message.len());
        out.extend_from_slice(prefix);
        out.push(CELL_MSG_ID);
        out.extend_from_slice(&circuit_id.to_be_bytes());
        out.push(plaintext as u8);
        out.push(relay_early as u8);
        out.push(inner_msg_id);
        out.extend_from_slice(message);
        out
    }

    /// `unwrap` Python : reconstruit `prefix + msg_id + circuit_id +
    /// payload` (le format "decapsule" lu par les handlers).
    pub fn unwrap(&self, prefix: &[u8; 22]) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(22 + 1 + 4 + self.message.len());
        out.extend_from_slice(prefix);
        out.push(self.inner_msg_id);
        out.extend_from_slice(&self.circuit_id.to_be_bytes());
        out.extend_from_slice(&self.message);
        out
    }

    /// Remplace le `circuit_id` de la cellule brute
    /// (`swap_circuit_id` Rust tunnels).
    pub fn swap_circuit_id(cell: &[u8], circuit_id: u32) -> Vec<u8> {
        let mut out = cell.to_vec();
        out[23..27].copy_from_slice(&circuit_id.to_be_bytes());
        out
    }
}

/// `check_cell_flags` des tunnels Rust.
pub fn check_cell_flags(cell: &[u8], max_relay_early: u8) -> Result<(), Ipv8Error> {
    // relay_early non nul uniquement pour extend (msg 4).
    if (cell[OFF_RELAY_EARLY] == 0 && cell[OFF_INNER_MSG_ID] == 4) || max_relay_early == 0 {
        return Err(Ipv8Error::Malformed(
            "flag relay_early absent ou inattendu",
        ));
    }
    if cell[OFF_PLAINTEXT] != 0 && !NO_CRYPTO_PACKETS.contains(&cell[OFF_INNER_MSG_ID]) {
        return Err(Ipv8Error::Malformed(
            "seuls create/created peuvent etre en clair",
        ));
    }
    Ok(())
}

/// `encrypt_cell` : chiffre le corps du message par couches (hop le
/// plus eloigne en dernier — `iter_mut().rev()` des tunnels Rust).
/// No-op si le flag plaintext est pose.
pub fn encrypt_cell(
    cell: &[u8],
    direction: tribler_crypto::ipv8::session::Direction,
    keys_list: &mut [SessionKeys],
) -> Result<Vec<u8>, Ipv8Error> {
    if cell[OFF_PLAINTEXT] != 0 {
        return Ok(cell.to_vec());
    }
    let mut message = cell[OFF_INNER_MSG_ID..].to_vec();
    for keys in keys_list.iter_mut().rev() {
        message = keys.encrypt_str(&message, direction)?;
    }
    let mut out = cell[..OFF_INNER_MSG_ID].to_vec();
    out.extend_from_slice(&message);
    Ok(out)
}

/// `decrypt_cell` : dechiffre dans l'ordre des hops.
pub fn decrypt_cell(
    cell: &[u8],
    direction: tribler_crypto::ipv8::session::Direction,
    keys_list: &[SessionKeys],
) -> Result<Vec<u8>, Ipv8Error> {
    if cell[OFF_PLAINTEXT] != 0 {
        return Ok(cell.to_vec());
    }
    let mut message = cell[OFF_INNER_MSG_ID..].to_vec();
    for keys in keys_list {
        message = keys.decrypt_str(&message, direction)?;
    }
    let mut out = cell[..OFF_INNER_MSG_ID].to_vec();
    out.extend_from_slice(&message);
    Ok(out)
}
