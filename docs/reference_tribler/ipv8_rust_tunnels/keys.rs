use openssl::asn1::Asn1Object;
use openssl::bn::{BigNum, BigNumContext};
use openssl::ec::{EcGroup, EcKey};
use openssl::ecdsa::EcdsaSig;
use openssl::hash::MessageDigest;
use openssl::pkey::{Id, PKey, Private, Public};
use openssl::sign::{Signer, Verifier};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyInt};

#[pyclass]
pub struct PublicKey {
    pub inner: PKey<Public>,
    pub crypt_pk: Option<Vec<u8>>,
}

#[pymethods]
impl PublicKey {
    #[new]
    fn new(keystring: &[u8]) -> PyResult<Self> {
        // LibNaCLPK: 10 byte prefix + 32 byte crypt_pk + 32 byte vk
        if keystring.starts_with(b"LibNaCLPK:") && keystring.len() >= 74 {
            let crypt_pk = keystring[10..42].to_vec();
            let vk_bytes = &keystring[42..74];

            let inner = PKey::public_key_from_raw_bytes(vk_bytes, Id::ED25519)
                .map_err(|e| PyValueError::new_err(format!("Failed to load Ed25519 PK: {}", e)))?;

            return Ok(Self {
                inner,
                crypt_pk: Some(crypt_pk),
            });
        }

        // OpenSSL PEM/DER
        let inner = if keystring.starts_with(b"-----") {
            PKey::public_key_from_pem(keystring)
        } else {
            PKey::public_key_from_der(keystring)
        }
        .map_err(|e| PyValueError::new_err(format!("Failed to load public key: {}", e)))?;

        Ok(Self {
            inner,
            crypt_pk: None,
        })
    }

    fn verify(&self, signature: &[u8], msg: &[u8]) -> bool {
        if self.inner.id() == Id::ED25519 {
            return Verifier::new_without_digest(&self.inner)
                .and_then(|mut verifier| verifier.verify_oneshot(signature, msg))
                .unwrap_or(false);
        }

        let mid = signature.len() / 2;
        if signature.is_empty() || signature.len() % 2 != 0 {
            return false;
        }

        let res: Result<bool, openssl::error::ErrorStack> = (|| {
            let r = BigNum::from_slice(&signature[..mid])?;
            let s = BigNum::from_slice(&signature[mid..])?;
            let sig = EcdsaSig::from_private_components(r, s)?;
            let ec = self.inner.ec_key()?;
            let digest = openssl::hash::hash(MessageDigest::sha1(), msg)?;

            Ok(sig.verify(&digest, &ec)?)
        })();

        res.unwrap_or(false)
    }

    fn key_to_bin(&self) -> PyResult<Vec<u8>> {
        if self.inner.id() == Id::ED25519 {
            let vk = self
                .inner
                .raw_public_key()
                .map_err(|e| PyValueError::new_err(e.to_string()))?;
            // Use crypt_pk if available, otherwise default to vk
            let pk = self.crypt_pk.as_ref().unwrap_or(&vk);
            return Ok([b"LibNaCLPK:".as_slice(), pk, &vk].concat());
        }
        self.inner
            .public_key_to_der()
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    fn key_to_pem(&self) -> PyResult<Vec<u8>> {
        self.inner
            .public_key_to_pem()
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    fn get_signature_length(&self) -> usize {
        if self.inner.id() == Id::ED25519 {
            return self.inner.size();
        }

        self.inner
            .ec_key()
            .map(|ec| {
                let bits = ec.group().degree();
                let field_len = (bits + 7) / 8;
                (field_len * 2) as usize
            })
            .unwrap_or_else(|_| self.inner.size())
    }

    pub fn curve_name(&self) -> PyResult<&'static str> {
        match self.inner.id() {
            Id::ED25519 => Ok("curve25519"),
            Id::EC => self
                .inner
                .ec_key()
                .and_then(|ec| {
                    ec.group()
                        .curve_name()
                        .ok_or_else(openssl::error::ErrorStack::get)
                })
                .and_then(|nid| nid.short_name())
                .map_err(|e| PyValueError::new_err(e.to_string())),
            _ => Err(PyValueError::new_err("Unsupported key type")),
        }
    }
}

#[pyclass]
pub struct PrivateKey {
    pub inner: PKey<Private>,
    pub crypt_sk: Option<Vec<u8>>, // Curve25519
}

#[pymethods]
impl PrivateKey {
    #[new]
    fn new(keystring: &[u8]) -> PyResult<Self> {
        // LibNaCL DualSecret: LibNaCLSK: + crypt_sk (32) + signer_seed (32) = 74 bytes
        if keystring.starts_with(b"LibNaCLSK:") && keystring.len() >= 74 {
            let crypt_sk = keystring[10..42].to_vec();
            let signer_seed = &keystring[42..74];

            let inner = PKey::private_key_from_raw_bytes(signer_seed, Id::ED25519)
                .map_err(|e| PyValueError::new_err(format!("Failed to load Ed25519 SK: {}", e)))?;

            return Ok(Self {
                inner,
                crypt_sk: Some(crypt_sk),
            });
        }

        // OpenSSL PEM/DER
        let inner = if keystring.starts_with(b"-----") {
            PKey::private_key_from_pem(keystring)
        } else {
            PKey::private_key_from_der(keystring)
        }
        .map_err(|e| PyValueError::new_err(format!("Failed to load private key: {}", e)))?;

        Ok(Self {
            inner,
            crypt_sk: None,
        })
    }

    fn key_to_bin(&mut self) -> PyResult<Vec<u8>> {
        if self.inner.id() == Id::ED25519 {
            let seed = self
                .inner
                .raw_private_key()
                .map_err(|e| PyValueError::new_err(e.to_string()))?;

            // Ensure crypt_sk exists.
            let sk = self.crypt_sk.get_or_insert_with(|| {
                PKey::generate_x25519()
                    .and_then(|k| k.raw_private_key())
                    .unwrap_or_default()
            });

            return Ok([b"LibNaCLSK:".as_slice(), sk, &seed].concat());
        }
        self.inner
            .private_key_to_der()
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    #[pyo3(name = "pub")]
    fn public_key(&self) -> PyResult<PublicKey> {
        let vk_inner = self
            .inner
            .public_key_to_der()
            .and_then(|der| PKey::public_key_from_der(&der))
            .map_err(|e| PyValueError::new_err(e.to_string()))?;

        let crypt_pk = self.crypt_sk.as_ref().and_then(|sk| {
            PKey::private_key_from_raw_bytes(sk, Id::X25519)
                .and_then(|k| k.raw_public_key())
                .ok()
        });

        Ok(PublicKey {
            inner: vk_inner,
            crypt_pk,
        })
    }

    fn key_to_pem(&self) -> PyResult<Vec<u8>> {
        if self.inner.id() == Id::ED25519 {
            return self
                .inner
                .private_key_to_pem_pkcs8()
                .map_err(|e| PyValueError::new_err(e.to_string()));
        }
        self.inner
            .ec_key()
            .and_then(|ec| ec.private_key_to_pem())
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    fn signature(&self, msg: &[u8]) -> PyResult<Vec<u8>> {
        if self.inner.id() == Id::ED25519 {
            return Signer::new_without_digest(&self.inner)
                .and_then(|mut signer| signer.sign_oneshot_to_vec(msg))
                .map_err(|e| PyValueError::new_err(format!("Ed25519 signature failed: {}", e)));
        }

        // NOTE: Legacy signature.
        let der_sig = Signer::new(MessageDigest::sha1(), &self.inner)
            .and_then(|mut s| s.sign_oneshot_to_vec(msg))
            .map_err(|e| PyValueError::new_err(e.to_string()))?;

        let sig = EcdsaSig::from_der(&der_sig).map_err(|e| PyValueError::new_err(e.to_string()))?;
        let len = self.get_signature_length() / 2;
        let mut raw = vec![0u8; len * 2];

        raw[..len].copy_from_slice(
            &sig.r()
                .to_vec_padded(len as i32)
                .map_err(|e| PyValueError::new_err(e.to_string()))?,
        );
        raw[len..].copy_from_slice(
            &sig.s()
                .to_vec_padded(len as i32)
                .map_err(|e| PyValueError::new_err(e.to_string()))?,
        );

        Ok(raw)
    }

    fn get_signature_length(&self) -> usize {
        if self.inner.id() == Id::ED25519 {
            return self.inner.size();
        }

        self.inner
            .ec_key()
            .map(|ec| {
                let bits = ec.group().degree();
                let field_len = (bits + 7) / 8;
                (field_len * 2) as usize
            })
            .unwrap_or_else(|_| self.inner.size())
    }

    #[staticmethod]
    fn generate(curve_name: &str) -> PyResult<Self> {
        if curve_name.to_lowercase() == "curve25519" {
            let signer = PKey::generate_ed25519().map_err(|e| PyValueError::new_err(e.to_string()))?;
            let crypt = PKey::generate_x25519().map_err(|e| PyValueError::new_err(e.to_string()))?;
            let crypt_sk_bytes = crypt
                .raw_private_key()
                .map_err(|e| PyValueError::new_err(e.to_string()))?;

            return Ok(PrivateKey {
                inner: signer,
                crypt_sk: Some(crypt_sk_bytes),
            });
        }

        // For legacy/non-ed25519 curves
        let inner = (|| {
            let nid = Asn1Object::from_str(curve_name)?.nid();
            let group = EcGroup::from_curve_name(nid)?;
            let ec_key = EcKey::generate(&group)?;
            PKey::from_ec_key(ec_key)
        })()
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

        Ok(PrivateKey {
            inner,
            crypt_sk: None,
        })
    }

    pub fn curve_name(&self) -> PyResult<&'static str> {
        match self.inner.id() {
            Id::ED25519 => Ok("curve25519"),
            Id::EC => self
                .inner
                .ec_key()
                .and_then(|ec| {
                    ec.group()
                        .curve_name()
                        .ok_or_else(openssl::error::ErrorStack::get)
                })
                .and_then(|nid| nid.short_name())
                .map_err(|e| PyValueError::new_err(e.to_string())),
            _ => Err(PyValueError::new_err("Unsupported key type")),
        }
    }
}

#[pyfunction]
pub fn generate_safe_prime(py: Python<'_>, bit_length: i32) -> PyResult<Py<PyAny>> {
    let mut prime = BigNum::new().map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

    prime
        .generate_prime(bit_length, true, None, None)
        .map_err(|e| PyRuntimeError::new_err(format!("Failed to generate safe prime: {}", e)))?;

    py.get_type::<PyInt>()
        .call_method1("from_bytes", (prime.to_vec(), "big"))
        .map(|obj| obj.into())
}

#[pyfunction]
pub fn generate_rsa_prime(py: Python<'_>, bit_length: u32) -> PyResult<Py<PyAny>> {
    let rsa = openssl::rsa::Rsa::generate(bit_length * 2)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

    let p_bytes = rsa
        .p()
        .ok_or_else(|| PyRuntimeError::new_err("RSA failed"))?
        .to_vec();

    py.get_type::<PyInt>()
        .call_method1("from_bytes", (p_bytes, "big"))
        .map(|obj| obj.into())
}

#[pyfunction]
pub fn is_prime(_py: Python<'_>, number: Bound<'_, PyInt>) -> PyResult<bool> {
    // Minimum number of bytes needed
    let byte_len = (number.call_method0("bit_length")?.extract::<usize>()? + 7) / 8;
    let bytes: Vec<u8> = number.call_method1("to_bytes", (byte_len, "big"))?.extract()?;

    let bn = BigNum::from_slice(&bytes).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    let mut ctx = BigNumContext::new().map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

    let is_p = bn
        .is_prime(64, &mut ctx)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

    Ok(is_p)
}
