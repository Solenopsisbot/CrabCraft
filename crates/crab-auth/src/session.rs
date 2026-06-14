//! The Mojang `sessionserver` join call.
//!
//! During the online-mode login handshake, after computing the server hash the
//! client tells Mojang it intends to join (proving it owns the account); the
//! server then verifies the same hash with Mojang. Returns `Ok(())` on the
//! expected `204 No Content`.

use uuid::Uuid;

use crate::AuthError;

const JOIN_URL: &str = "https://sessionserver.mojang.com/session/minecraft/join";

/// Notifies Mojang that the authenticated account is joining a server with the
/// given computed `server_hash`.
pub async fn join_server(
    access_token: &str,
    uuid: Uuid,
    server_hash: &str,
) -> Result<(), AuthError> {
    let http = reqwest::Client::new();
    let body = serde_json::json!({
        "accessToken": access_token,
        "selectedProfile": uuid.simple().to_string(),
        "serverId": server_hash,
    });
    let resp = http.post(JOIN_URL).json(&body).send().await?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(AuthError::Flow(format!(
            "sessionserver join failed: HTTP {}",
            resp.status()
        )))
    }
}
