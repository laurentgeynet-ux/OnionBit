use openssl::derive::Deriver;
use openssl::hash::MessageDigest;
use openssl::md::Md;
use openssl::pkey::{Id, PKey};
use openssl::pkey_ctx::{HkdfMode, PkeyCtx};
use openssl::sign::Signer;
use openssl::symm::{decrypt_aead, encrypt_aead, Cipher};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use crate::keys::{PrivateKey, PublicKey};

macro_rules! rotl32 {
    ($x:expr, $n:expr) => {
        ($x << $n) | ($x >> (32 - $n))
    };
}

// Port of libnacl's crypto_box_beforenm.
#[pyfunction]
pub fn crypto_box_beforenm(pk: &[u8], sk: &[u8]) -> PyResult<Vec<u8>> {
    if pk.len() != 32 || sk.len() != 32 {
        return Err(PyValueError::new_err("Inputs must be 32 bytes"));
    }

    // Clamp sk (same as libnacl's crypto_scalarmult_curve25519)
    let mut clamped_sk = sk.to_vec();
    clamped_sk[0] &= 248;
    clamped_sk[31] &= 127;
    clamped_sk[31] |= 64;

    // X25519 DH of the clamped sk key with the peer's pk
    let s = (|| -> Result<Vec<u8>, openssl::error::ErrorStack> {
        let openssl_sk = PKey::private_key_from_raw_bytes(&clamped_sk, Id::X25519)?;
        let openssl_pk = PKey::public_key_from_raw_bytes(pk, Id::X25519)?;
        let mut deriver = Deriver::new(&openssl_sk)?;
        deriver.set_peer(&openssl_pk)?;
        deriver.derive_to_vec()
    })()
    .map_err(|e| PyValueError::new_err(format!("X25519 DH failed: {}", e)))?;

    // Perform hsalsa20 (same as libnacl's crypto_core_hsalsa20)
    // Using the salsa20 crate for this proved problematic.
    let load32_le = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());

    let mut x0: u32 = 0x61707865;
    let mut x5: u32 = 0x3320646e;
    let mut x10: u32 = 0x79622d32;
    let mut x15: u32 = 0x6b206574;
    let mut x1 = load32_le(&s, 0);
    let mut x2 = load32_le(&s, 4);
    let mut x3 = load32_le(&s, 8);
    let mut x4 = load32_le(&s, 12);
    let mut x11 = load32_le(&s, 16);
    let mut x12 = load32_le(&s, 20);
    let mut x13 = load32_le(&s, 24);
    let mut x14 = load32_le(&s, 28);
    let mut x6: u32 = 0;
    let mut x7: u32 = 0;
    let mut x8: u32 = 0;
    let mut x9: u32 = 0;

    for _ in (0..20).step_by(2) {
        x4 ^= rotl32!(x0.wrapping_add(x12), 7);
        x8 ^= rotl32!(x4.wrapping_add(x0), 9);
        x12 ^= rotl32!(x8.wrapping_add(x4), 13);
        x0 ^= rotl32!(x12.wrapping_add(x8), 18);
        x9 ^= rotl32!(x5.wrapping_add(x1), 7);
        x13 ^= rotl32!(x9.wrapping_add(x5), 9);
        x1 ^= rotl32!(x13.wrapping_add(x9), 13);
        x5 ^= rotl32!(x1.wrapping_add(x13), 18);
        x14 ^= rotl32!(x10.wrapping_add(x6), 7);
        x2 ^= rotl32!(x14.wrapping_add(x10), 9);
        x6 ^= rotl32!(x2.wrapping_add(x14), 13);
        x10 ^= rotl32!(x6.wrapping_add(x2), 18);
        x3 ^= rotl32!(x15.wrapping_add(x11), 7);
        x7 ^= rotl32!(x3.wrapping_add(x15), 9);
        x11 ^= rotl32!(x7.wrapping_add(x3), 13);
        x15 ^= rotl32!(x11.wrapping_add(x7), 18);
        x1 ^= rotl32!(x0.wrapping_add(x3), 7);
        x2 ^= rotl32!(x1.wrapping_add(x0), 9);
        x3 ^= rotl32!(x2.wrapping_add(x1), 13);
        x0 ^= rotl32!(x3.wrapping_add(x2), 18);
        x6 ^= rotl32!(x5.wrapping_add(x4), 7);
        x7 ^= rotl32!(x6.wrapping_add(x5), 9);
        x4 ^= rotl32!(x7.wrapping_add(x6), 13);
        x5 ^= rotl32!(x4.wrapping_add(x7), 18);
        x11 ^= rotl32!(x10.wrapping_add(x9), 7);
        x8 ^= rotl32!(x11.wrapping_add(x10), 9);
        x9 ^= rotl32!(x8.wrapping_add(x11), 13);
        x10 ^= rotl32!(x9.wrapping_add(x8), 18);
        x12 ^= rotl32!(x15.wrapping_add(x14), 7);
        x13 ^= rotl32!(x12.wrapping_add(x15), 9);
        x14 ^= rotl32!(x13.wrapping_add(x12), 13);
        x15 ^= rotl32!(x14.wrapping_add(x13), 18);
    }

    let mut out = vec![0u8; 32];
    out[0..4].copy_from_slice(&x0.to_le_bytes());
    out[4..8].copy_from_slice(&x5.to_le_bytes());
    out[8..12].copy_from_slice(&x10.to_le_bytes());
    out[12..16].copy_from_slice(&x15.to_le_bytes());
    out[16..20].copy_from_slice(&x6.to_le_bytes());
    out[20..24].copy_from_slice(&x7.to_le_bytes());
    out[24..28].copy_from_slice(&x8.to_le_bytes());
    out[28..32].copy_from_slice(&x9.to_le_bytes());

    Ok(out)
}

#[pymethods]
impl PrivateKey {
    fn diffie_hellman(&self, peer_public_key: &[u8]) -> PyResult<Vec<u8>> {
        let sk_bytes = self
            .crypt_sk
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("No crypt_sk"))?;

        crypto_box_beforenm(peer_public_key, sk_bytes)
    }

    fn get_crypt_pk(&self) -> PyResult<Vec<u8>> {
        let sk_bytes = self
            .crypt_sk
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("No crypt_sk"))?;

        (|| -> Result<Vec<u8>, openssl::error::ErrorStack> {
            let my_sk = PKey::private_key_from_raw_bytes(sk_bytes, Id::X25519)?;
            my_sk.raw_public_key()
        })()
        .map_err(|e| PyValueError::new_err(e.to_string()))
    }
}

#[pymethods]
impl PublicKey {
    fn get_crypt_pk(&self) -> PyResult<Vec<u8>> {
        self.crypt_pk
            .as_ref()
            .cloned()
            .ok_or_else(|| PyValueError::new_err("No crypt_pk"))
    }
}

#[pyfunction]
pub fn crypto_auth(key: &[u8], message: &[u8]) -> PyResult<Vec<u8>> {
    let pkey = PKey::hmac(key).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let mut signer =
        Signer::new(MessageDigest::sha512(), &pkey).map_err(|e| PyValueError::new_err(e.to_string()))?;

    signer
        .update(message)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    let tag = signer
        .sign_to_vec()
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

    Ok(tag[..32].to_vec())
}

#[pyfunction]
pub fn crypto_auth_verify(tag: &[u8], key: &[u8], message: &[u8]) -> bool {
    if let Ok(calculated_tag) = crypto_auth(key, message) {
        return openssl::memcmp::eq(&calculated_tag, tag);
    }
    false
}

#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct SessionKeys {
    #[pyo3(get, set)]
    pub key_forward: Vec<u8>,
    #[pyo3(get, set)]
    pub key_backward: Vec<u8>,
    #[pyo3(get, set)]
    pub salt_forward: Vec<u8>,
    #[pyo3(get, set)]
    pub salt_backward: Vec<u8>,
    #[pyo3(get, set)]
    pub salt_explicit_forward: u64,
    #[pyo3(get, set)]
    pub salt_explicit_backward: u64,
}

#[pymethods]
impl SessionKeys {
    pub fn encrypt_str(&mut self, content: &[u8], direction: i32) -> PyResult<Vec<u8>> {
        let (key, salt, counter) = if direction == 0 {
            self.salt_explicit_forward += 1;
            (&self.key_forward, &self.salt_forward, self.salt_explicit_forward)
        } else {
            self.salt_explicit_backward += 1;
            (&self.key_backward, &self.salt_backward, self.salt_explicit_backward)
        };

        let counter_bytes = (counter as u64).to_be_bytes();

        let mut nonce = Vec::with_capacity(12);
        nonce.extend_from_slice(salt);
        nonce.extend_from_slice(&counter_bytes);

        let mut tag = [0u8; 16];
        let ciphertext =
            encrypt_aead(Cipher::chacha20_poly1305(), key, Some(&nonce), &[], content, &mut tag)
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

        // Format: 8 byte counter + ciphertext + 16 byte tag
        let mut out = Vec::with_capacity(8 + ciphertext.len() + 16);
        out.extend_from_slice(&counter_bytes);
        out.extend_from_slice(&ciphertext);
        out.extend_from_slice(&tag);
        Ok(out)
    }

    pub fn decrypt_str(&self, content: &[u8], direction: i32) -> PyResult<Vec<u8>> {
        if content.len() < 24 {
            return Err(PyValueError::new_err("Content too short"));
        }

        let (key, salt) = if direction == 0 {
            (&self.key_forward, &self.salt_forward)
        } else {
            (&self.key_backward, &self.salt_backward)
        };

        let (counter_bytes, rest) = content.split_at(8);
        let (ciphertext, tag) = rest.split_at(rest.len() - 16);

        let mut nonce = Vec::with_capacity(12);
        nonce.extend_from_slice(salt);
        nonce.extend_from_slice(counter_bytes);

        decrypt_aead(Cipher::chacha20_poly1305(), key, Some(&nonce), &[], ciphertext, tag)
            .map_err(|_| PyRuntimeError::new_err("Decryption failed"))
    }
}

#[pyfunction]
pub fn generate_session_keys(_py: Python<'_>, shared_secret: &[u8]) -> PyResult<SessionKeys> {
    let key = (|| -> Result<Vec<u8>, openssl::error::ErrorStack> {
        let mut ctx = PkeyCtx::new_id(Id::HKDF)?;
        ctx.derive_init()?;
        ctx.set_hkdf_mode(HkdfMode::EXPAND_ONLY)?;
        ctx.set_hkdf_key(shared_secret)?;
        ctx.set_hkdf_md(Md::sha256())?;
        ctx.add_hkdf_info(b"key_generation")?;

        let mut out = vec![0u8; 72];
        ctx.derive(Some(&mut out))?;
        Ok(out)
    })()
    .map_err(|e| PyValueError::new_err(e.to_string()))?;

    Ok(SessionKeys {
        key_backward: key[0..32].to_vec(),
        key_forward: key[32..64].to_vec(),

        salt_backward: key[64..68].to_vec(),
        salt_forward: key[68..72].to_vec(),

        salt_explicit_backward: 1,
        salt_explicit_forward: 1,
    })
}
