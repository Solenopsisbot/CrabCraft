//! Microsoft device-code login → Minecraft session.
//!
//! Flow: device code → poll for MS token → Xbox Live → XSTS → Minecraft
//! services token → profile. Implemented to spec; requires a real account and
//! live Microsoft/Mojang endpoints, so it is not tested in CI.

use std::time::Duration;

use serde::Deserialize;
use uuid::Uuid;

use crate::{AuthError, Session};

// Public client id used by the vanilla launcher for MSA device-code login.
const CLIENT_ID: &str = "00000000402b5328";
const SCOPE: &str = "XboxLive.signin offline_access";
const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct XboxResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: DisplayClaims,
}

#[derive(Deserialize)]
struct DisplayClaims {
    xui: Vec<Xui>,
}

#[derive(Deserialize)]
struct Xui {
    uhs: String,
}

#[derive(Deserialize)]
struct McLogin {
    access_token: String,
}

#[derive(Deserialize)]
struct McProfile {
    id: String,
    name: String,
}

/// Runs the full device-code login. Prints the verification URL + user code,
/// then polls until the user authorizes (or it times out).
pub async fn device_code_login() -> Result<Session, AuthError> {
    let http = reqwest::Client::new();

    let dc: DeviceCode = http
        .post(DEVICE_CODE_URL)
        .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    println!(
        "\n  Sign in: open {} and enter code {}\n",
        dc.verification_uri, dc.user_code
    );

    let ms_token = poll_for_token(&http, &dc).await?;
    let xbl = xbox_authenticate(&http, &ms_token).await?;
    let xsts = xsts_authorize(&http, &xbl.token).await?;
    let uhs = xsts
        .display_claims
        .xui
        .first()
        .map(|x| x.uhs.clone())
        .ok_or_else(|| AuthError::Flow("missing Xbox user hash".into()))?;
    let mc_token = minecraft_login(&http, &uhs, &xsts.token).await?;
    let profile = minecraft_profile(&http, &mc_token).await?;
    let uuid = Uuid::parse_str(&profile.id)
        .map_err(|e| AuthError::Flow(format!("bad profile uuid: {e}")))?;

    Ok(Session {
        access_token: mc_token,
        uuid,
        username: profile.name,
    })
}

async fn poll_for_token(http: &reqwest::Client, dc: &DeviceCode) -> Result<String, AuthError> {
    for _ in 0..180 {
        tokio::time::sleep(Duration::from_secs(dc.interval.max(1))).await;
        let resp: TokenResponse = http
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", CLIENT_ID),
                ("device_code", dc.device_code.as_str()),
            ])
            .send()
            .await?
            .json()
            .await?;
        if let Some(token) = resp.access_token {
            return Ok(token);
        }
        match resp.error.as_deref() {
            Some("authorization_pending") | None => continue,
            Some(other) => return Err(AuthError::Flow(format!("device code error: {other}"))),
        }
    }
    Err(AuthError::Timeout)
}

async fn xbox_authenticate(
    http: &reqwest::Client,
    ms_token: &str,
) -> Result<XboxResponse, AuthError> {
    let body = serde_json::json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={ms_token}"),
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT",
    });
    Ok(http
        .post(XBL_URL)
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn xsts_authorize(
    http: &reqwest::Client,
    xbl_token: &str,
) -> Result<XboxResponse, AuthError> {
    let body = serde_json::json!({
        "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
    });
    Ok(http
        .post(XSTS_URL)
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn minecraft_login(
    http: &reqwest::Client,
    uhs: &str,
    xsts_token: &str,
) -> Result<String, AuthError> {
    let body = serde_json::json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") });
    let resp: McLogin = http
        .post(MC_LOGIN_URL)
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(resp.access_token)
}

async fn minecraft_profile(http: &reqwest::Client, mc_token: &str) -> Result<McProfile, AuthError> {
    Ok(http
        .get(MC_PROFILE_URL)
        .bearer_auth(mc_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
