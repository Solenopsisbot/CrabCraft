//! AES-128-CFB8, the stream cipher Minecraft uses for the encrypted connection.
//!
//! After the login encryption handshake, every byte in each direction is
//! AES-128 in 8-bit cipher-feedback mode, with **both** the key and the initial
//! shift register set to the shared secret. CFB8 is processed one byte at a
//! time, so encryptor and decryptor each keep an independent 16-byte register.
//!
//! We build CFB8 by hand on top of the `aes` block cipher (we do not implement
//! AES itself). The implementation is checked against the NIST SP800-38A CFB8
//! test vector plus encrypt/decrypt round-trips.

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;

/// One direction of an AES-128-CFB8 stream.
pub struct Aes128Cfb8 {
    cipher: Aes128,
    register: [u8; 16],
}

impl Aes128Cfb8 {
    /// Minecraft cipher: key and IV are both the shared secret.
    pub fn new(secret: &[u8; 16]) -> Self {
        Self::with_iv(secret, secret)
    }

    /// General constructor (used by tests / non-Minecraft IVs).
    pub fn with_iv(key: &[u8; 16], iv: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(GenericArray::from_slice(key)),
            register: *iv,
        }
    }

    #[inline]
    fn keystream_byte(&self) -> u8 {
        let mut block = GenericArray::clone_from_slice(&self.register);
        self.cipher.encrypt_block(&mut block);
        block[0]
    }

    #[inline]
    fn advance(&mut self, ciphertext_byte: u8) {
        self.register.copy_within(1.., 0);
        self.register[15] = ciphertext_byte;
    }

    /// Encrypts `data` in place, advancing the stream.
    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data {
            let c = *byte ^ self.keystream_byte();
            self.advance(c);
            *byte = c;
        }
    }

    /// Decrypts `data` in place, advancing the stream.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        for byte in data {
            let c = *byte;
            *byte = c ^ self.keystream_byte();
            self.advance(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nist_cfb8_aes128_vector() {
        // NIST SP800-38A, F.3.7 (CFB8-AES128.Encrypt).
        let key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let iv: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        // NIST plaintext 6b c1 be e2 -> CFB8 ciphertext 3b 79 42 4c
        let mut data = [0x6b, 0xc1, 0xbe, 0xe2];
        Aes128Cfb8::with_iv(&key, &iv).encrypt(&mut data);
        assert_eq!(data, [0x3b, 0x79, 0x42, 0x4c]);
    }

    #[test]
    fn roundtrip_longer_than_block() {
        let secret = [7u8; 16];
        let original: Vec<u8> = (0..100u8).collect();
        let mut buf = original.clone();
        Aes128Cfb8::new(&secret).encrypt(&mut buf);
        assert_ne!(buf, original, "ciphertext should differ");
        Aes128Cfb8::new(&secret).decrypt(&mut buf);
        assert_eq!(buf, original, "decrypt must invert encrypt");
    }

    #[test]
    fn split_processing_matches_whole() {
        // Processing in chunks must equal processing all at once (stream state).
        let secret = [0x11u8; 16];
        let data: Vec<u8> = (0..50u8).collect();

        let mut whole = data.clone();
        Aes128Cfb8::new(&secret).encrypt(&mut whole);

        let mut chunked = data.clone();
        let mut cipher = Aes128Cfb8::new(&secret);
        let (a, b) = chunked.split_at_mut(13);
        cipher.encrypt(a);
        cipher.encrypt(b);
        assert_eq!(whole, chunked);
    }
}
