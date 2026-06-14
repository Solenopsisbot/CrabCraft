//! # crab-auth
//!
//! Everything needed to join an **online-mode** server:
//!
//! * [`crypt`] – the login-handshake crypto: the Minecraft server hash and RSA
//!   encryption of the shared secret. Verified with known-answer tests.
//! * [`msa`] – the Microsoft device-code OAuth → Xbox Live → XSTS → Minecraft
//!   services flow that yields a [`Session`].
//! * [`session`] – the Mojang `sessionserver` "join" call.
//!
//! The crypto is unit-tested. The network flows ([`msa`], [`session`]) are
//! implemented to spec but require a real Microsoft account and live endpoints,
//! so they are not exercised in CI.

pub mod crypt;
pub mod msa;
pub mod session;

pub use crypt::{encrypt_to_server, minecraft_digest, random_shared_secret, server_hash};
pub use msa::device_code_login;
pub use session::join_server;

use uuid::Uuid;

/// A logged-in Minecraft session.
#[derive(Clone, Debug)]
pub struct Session {
    /// Minecraft services access token (for the sessionserver join).
    pub access_token: String,
    /// The account's profile UUID.
    pub uuid: Uuid,
    /// The account's profile (player) name.
    pub username: String,
}

/// Errors from authentication and session setup.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("RSA error: {0}")]
    Rsa(#[from] rsa::Error),
    #[error("public key parse error: {0}")]
    Spki(#[from] rsa::pkcs8::spki::Error),
    #[error("auth flow error: {0}")]
    Flow(String),
    #[error("authorization timed out")]
    Timeout,
}
