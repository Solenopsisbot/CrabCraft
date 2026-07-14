//! Login-handshake crypto: the Minecraft server hash and RSA encryption of the
//! shared secret.

use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use sha1::{Digest, Sha1};

use crate::AuthError;

/// A fresh random 16-byte AES shared secret.
pub fn random_shared_secret() -> [u8; 16] {
    let mut secret = [0u8; 16];
    rand::rng().fill_bytes(&mut secret);
    secret
}

/// The Minecraft "server hash": SHA-1 of `server_id || shared_secret ||
/// public_key`, rendered as a **signed**, leading-zero-trimmed hex string (the
/// quirky `BigInteger.toString(16)` form Mojang uses).
pub fn server_hash(server_id: &str, shared_secret: &[u8], public_key_der: &[u8]) -> String {
    let mut data = Vec::with_capacity(server_id.len() + shared_secret.len() + public_key_der.len());
    data.extend_from_slice(server_id.as_bytes());
    data.extend_from_slice(shared_secret);
    data.extend_from_slice(public_key_der);
    minecraft_digest(&data)
}

/// SHA-1 of `data` rendered in Minecraft's signed-hex form.
pub fn minecraft_digest(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    let hash: [u8; 20] = hasher.finalize().into();
    signed_hex(hash)
}

/// RSA PKCS#1 v1.5 encrypts `data` to the server's DER (SubjectPublicKeyInfo)
/// public key — used for both the shared secret and the verify token.
pub fn encrypt_to_server(public_key_der: &[u8], data: &[u8]) -> Result<Vec<u8>, AuthError> {
    let key = RsaPublicKey::from_public_key_der(public_key_der)?;
    // `rsa` 0.9 uses rand_core 0.6, while rand 0.9 uses rand_core 0.9. Use
    // the RNG re-exported by `rsa` so the cryptographic trait versions match.
    let mut rng = rsa::rand_core::OsRng;
    Ok(key.encrypt(&mut rng, Pkcs1v15Encrypt, data)?)
}

/// Interprets a 20-byte digest as a big-endian two's-complement integer and
/// formats it like Java's `BigInteger.toString(16)`.
fn signed_hex(mut hash: [u8; 20]) -> String {
    let negative = hash[0] & 0x80 != 0;
    if negative {
        // two's complement: invert all bytes, then add one (big-endian).
        let mut carry = true;
        for byte in hash.iter_mut().rev() {
            *byte = !*byte;
            if carry {
                let (v, c) = byte.overflowing_add(1);
                *byte = v;
                carry = c;
            }
        }
    }
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    let trimmed = hex.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    if negative {
        format!("-{trimmed}")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_hash_known_vectors() {
        // Canonical examples (Mojang/wiki.vg) for the signed-hex digest.
        assert_eq!(
            minecraft_digest(b"Notch"),
            "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48"
        );
        assert_eq!(
            minecraft_digest(b"jeb_"),
            "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1"
        );
        assert_eq!(
            minecraft_digest(b"simon"),
            "88e16a1019277b15d58faf0541e11910eb756f6"
        );
    }

    #[test]
    fn rsa_encrypt_roundtrips() {
        use rsa::pkcs8::EncodePublicKey;
        use rsa::RsaPrivateKey;

        let mut rng = rsa::rand_core::OsRng;
        let private = RsaPrivateKey::new(&mut rng, 1024).expect("keygen");
        let public = RsaPublicKey::from(&private);
        let der = public.to_public_key_der().expect("der");

        let secret = random_shared_secret();
        let encrypted = encrypt_to_server(der.as_bytes(), &secret).expect("encrypt");
        let decrypted = private
            .decrypt(Pkcs1v15Encrypt, &encrypted)
            .expect("decrypt");
        assert_eq!(decrypted, secret);
    }
}
