//! Microsoft account login via the OAuth 2.0 device-code flow.
//!
//! ```text
//! Microsoft OAuth  ->  Xbox Live (XBL)  ->  XSTS  ->  Minecraft Services
//! ```

use anyhow::{anyhow, bail, Context, Result};
use pcl_net::HttpClient;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::account::{Account, AccountKind};

pub const DEVICE_CODE_URL: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
pub const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
pub const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
pub const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
pub const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
pub const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

const SCOPE: &str = "XboxLive.signin offline_access";

/// Placeholder Azure application (public client) id.
///
/// PCL-RS does not ship a shared client id: every distributor registers its own
/// "mobile and desktop application" in Entra ID, enables the device code flow and
/// sets this constant (or the `PCL_MS_CLIENT_ID` environment variable).
pub const DEFAULT_CLIENT_ID: &str = "00000000-0000-0000-0000-000000000000";

/// What to show the user while they sign in.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeviceCodeFlow {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub interval: u64,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MsToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_at: u64,
}

pub struct MicrosoftAuth {
    client: HttpClient,
    client_id: String,
}

impl MicrosoftAuth {
    pub fn new(client: HttpClient) -> Self {
        let client_id = std::env::var("PCL_MS_CLIENT_ID")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string());
        Self::with_client_id(client, client_id)
    }

    pub fn with_client_id(client: HttpClient, client_id: impl Into<String>) -> Self {
        Self {
            client,
            client_id: client_id.into(),
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn client(&self) -> &HttpClient {
        &self.client
    }

    /// Step 1: ask Microsoft for a user code + verification URL.
    pub async fn request_device_code(&self) -> Result<DeviceCodeFlow> {
        let (status, body) = self
            .client
            .post_form_value(
                DEVICE_CODE_URL,
                &[
                    ("client_id", self.client_id.clone()),
                    ("scope", SCOPE.to_string()),
                ],
            )
            .await?;

        if !status.is_success() {
            bail!("device code request failed ({status}): {body}");
        }
        Ok(serde_json::from_value(body).context("decoding device code response")?)
    }

    /// Step 2: poll until the user finishes signing in.
    pub async fn poll_for_token<F>(&self, flow: &DeviceCodeFlow, mut on_wait: F) -> Result<MsToken>
    where
        F: FnMut(&DeviceCodeFlow),
    {
        let interval = Duration::from_secs(flow.interval.max(1).min(30));
        let deadline = SystemTime::now() + Duration::from_secs(flow.expires_in.max(60));

        loop {
            if SystemTime::now() > deadline {
                bail!("the sign-in code expired before it was used");
            }
            on_wait(flow);
            tokio::time::sleep(interval).await;

            let (status, body) = self
                .client
                .post_form_value(
                    TOKEN_URL,
                    &[
                        ("client_id", self.client_id.clone()),
                        ("device_code", flow.device_code.clone()),
                        (
                            "grant_type",
                            "urn:ietf:params:oauth:grant-type:device_code".to_string(),
                        ),
                    ],
                )
                .await?;

            if status.is_success() {
                return decode_ms_token(&body);
            }

            match body.get("error").and_then(|e| e.as_str()).unwrap_or("") {
                "authorization_pending" => continue,
                "slow_down" => {
                    tokio::time::sleep(interval).await;
                    continue;
                }
                "authorization_declined" => bail!("the sign-in request was declined"),
                "expired_token" | "code_expired" => {
                    bail!("the sign-in code expired; please start again")
                }
                other => bail!(
                    "Microsoft sign-in failed: {}",
                    body.get("error_description")
                        .and_then(|d| d.as_str())
                        .unwrap_or(other)
                ),
            }
        }
    }

    /// Refresh an expired token from an account's stored refresh token.
    pub async fn refresh(&self, account: &Account) -> Result<MsToken> {
        let refresh = account
            .refresh_token
            .as_deref()
            .ok_or_else(|| anyhow!("account has no refresh token; sign in again"))?;

        let (status, body) = self
            .client
            .post_form_value(
                TOKEN_URL,
                &[
                    ("client_id", self.client_id.clone()),
                    ("scope", SCOPE.to_string()),
                    ("grant_type", "refresh_token".to_string()),
                    ("refresh_token", refresh.to_string()),
                ],
            )
            .await?;

        if !status.is_success() {
            bail!("token refresh failed ({status}): {body}");
        }
        decode_ms_token(&body)
    }

    /// Step 3: exchange a Microsoft token for a Minecraft account.
    pub async fn authenticate(&self, token: &MsToken) -> Result<Account> {
        let (status, xbl) = self
            .client
            .post_json_value(
                XBL_URL,
                &serde_json::json!({
                    "Properties": {
                        "AuthMethod": "RPS",
                        "SiteName": "user.auth.xboxlive.com",
                        "RpsTicket": format!("d={}", token.access_token)
                    },
                    "RelyingParty": "http://auth.xboxlive.com",
                    "TokenType": "JWT"
                }),
            )
            .await?;
        if !status.is_success() {
            bail!("Xbox Live authentication failed ({status}): {xbl}");
        }
        let xbl_token = xbl
            .get("Token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("Xbox Live response has no Token: {xbl}"))?
            .to_string();

        let (status, xsts) = self
            .client
            .post_json_value(
                XSTS_URL,
                &serde_json::json!({
                    "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
                    "RelyingParty": "rp://api.minecraftservices.com/",
                    "TokenType": "JWT"
                }),
            )
            .await?;
        if !status.is_success() {
            bail!("{}", xsts_error_message(&xsts));
        }
        let xsts_token = xsts
            .get("Token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("XSTS response has no Token: {xsts}"))?;
        let uhs = xsts
            .pointer("/DisplayClaims/xui/0/uhs")
            .and_then(|u| u.as_str())
            .ok_or_else(|| anyhow!("XSTS response has no user hash: {xsts}"))?;

        let (status, mc) = self
            .client
            .post_json_value(
                MC_LOGIN_URL,
                &serde_json::json!({
                    "identityToken": format!("XBL3.0 x={uhs};{xsts_token}")
                }),
            )
            .await?;
        if !status.is_success() {
            bail!(
                "Minecraft login failed ({status}). This usually means the account does \
                 not own Minecraft: Java Edition. Response: {mc}"
            );
        }
        let mc_token = mc
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("Minecraft response has no access_token: {mc}"))?;
        let mc_expires_in = mc.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(86_400);

        let (status, profile) = self
            .client
            .get_json_value(MC_PROFILE_URL, Some(mc_token))
            .await?;
        if !status.is_success() {
            bail!(
                "could not read the Minecraft profile ({status}); the account may not own \
                 the game: {profile}"
            );
        }
        let name = profile
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| anyhow!("profile has no name: {profile}"))?
            .to_string();
        let uuid = profile
            .get("id")
            .and_then(|u| u.as_str())
            .ok_or_else(|| anyhow!("profile has no id: {profile}"))?
            .to_string();

        Ok(Account {
            kind: AccountKind::Microsoft,
            username: name,
            uuid,
            access_token: mc_token.to_string(),
            user_type: "msa".to_string(),
            xuid: xsts
                .pointer("/DisplayClaims/xui/0/xid")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string()),
            expires_at: Some(now_secs() + mc_expires_in),
            refresh_token: token.refresh_token.clone(),
        })
    }

    /// Full interactive login; `notify` receives the code to show the user.
    pub async fn login<N>(&self, notify: N) -> Result<Account>
    where
        N: FnMut(&DeviceCodeFlow),
    {
        let flow = self.request_device_code().await?;
        let token = self.poll_for_token(&flow, notify).await?;
        self.authenticate(&token).await
    }
}

/// 供浏览器登录流程复用的 token 解析入口。
pub(crate) fn decode_token_public(body: &serde_json::Value) -> Result<MsToken> {
    decode_ms_token(body)
}

fn decode_ms_token(body: &serde_json::Value) -> Result<MsToken> {
    let access_token = body
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow!("OAuth response has no access_token: {body}"))?
        .to_string();
    let refresh_token = body
        .get("refresh_token")
        .and_then(|t| t.as_str())
        .map(|t| t.to_string());
    let expires_in = body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
    Ok(MsToken {
        access_token,
        refresh_token,
        expires_at: now_secs() + expires_in,
    })
}

/// Turn the cryptic XSTS `XErr` codes into actionable messages.
fn xsts_error_message(xsts: &serde_json::Value) -> String {
    let xerr = xsts.get("XErr").and_then(|v| v.as_u64()).unwrap_or(0);
    let advice = match xerr {
        2_148_916_233 => "该 Microsoft 账户没有 Xbox 档案，请先在 xbox.com 创建档案",
        2_148_916_235 => "该账户所在的地区/国家不受 Xbox Live 支持",
        2_148_916_238 => "该账户是儿童账户，需要家长在 Microsoft 家庭组中授权",
        2_148_916_239 => "该账户受到年龄限制，无法登录 Minecraft",
        _ => "请在 account.xbox.com 检查账户状态",
    };
    format!("Xbox Live (XSTS) 拒绝了登录（XErr {xerr}）：{advice}")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_token() {
        let body = serde_json::json!({
            "access_token": "abc", "refresh_token": "r", "expires_in": 3600
        });
        let t = decode_ms_token(&body).unwrap();
        assert_eq!(t.access_token, "abc");
        assert_eq!(t.refresh_token.as_deref(), Some("r"));
        assert!(t.expires_at > now_secs());
    }

    #[test]
    fn maps_known_xsts_errors() {
        let msg = xsts_error_message(&serde_json::json!({"XErr": 2_148_916_233u64}));
        assert!(msg.contains("2148916233"));
    }
}
