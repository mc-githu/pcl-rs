//! 浏览器登录（OAuth 2.0 授权码 + PKCE + 回环重定向）。
//!
//! 与设备码相比更接近官方启动器的体验：拉起系统浏览器 → 用户登录 → 浏览器跳回
//! `http://127.0.0.1:<随机端口>/auth` → 启动器拿到 code 换 token。之后仍然走
//! XBL → XSTS → Minecraft Services 链路。
//!
//! 只监听回环地址，且用一次性 state + PKCE 校验，避免被本机其他进程劫持。

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use pcl_net::HttpClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::account::Account;
use crate::microsoft::{MicrosoftAuth, MsToken};

/// Microsoft 公共客户端授权端点（消费者账户）。
const AUTHORIZE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const SCOPE: &str = "XboxLive.signin offline_access";

/// 登录过程中的一次性参数。
pub struct BrowserLogin {
    pub authorize_url: String,
    pub redirect_uri: String,
    verifier: String,
    state: String,
    listener: TcpListener,
}

impl MicrosoftAuth {
    /// 绑定回环端口并构造授权 URL（调用方随后用 `open_browser` 打开它）。
    pub async fn begin_browser_login(&self) -> Result<BrowserLogin> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("绑定本地回环端口")?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}/auth");

        let verifier = random_urlsafe(48);
        let challenge = pkce_challenge(&verifier);
        let state = random_urlsafe(16);

        let mut params = url::form_urlencoded::Serializer::new(String::new());
        params.append_pair("client_id", self.client_id());
        params.append_pair("response_type", "code");
        params.append_pair("redirect_uri", &redirect_uri);
        params.append_pair("response_mode", "query");
        params.append_pair("scope", SCOPE);
        params.append_pair("state", &state);
        params.append_pair("code_challenge", &challenge);
        params.append_pair("code_challenge_method", "S256");
        params.append_pair("prompt", "select_account");
        let authorize_url = format!("{AUTHORIZE_URL}?{}", params.finish());

        Ok(BrowserLogin {
            authorize_url,
            redirect_uri,
            verifier,
            state,
            listener,
        })
    }

    /// 等待浏览器回调，用自己的 code 换 token，再完成 XBL/XSTS/MC 链路。
    pub async fn complete_browser_login(
        &self,
        session: BrowserLogin,
        timeout: Duration,
    ) -> Result<Account> {
        let code = tokio::time::timeout(timeout, wait_for_code(session.listener, &session.state))
            .await
            .map_err(|_| anyhow!("等待浏览器登录超时，请重试"))??;

        let token = self.exchange_code(&code, &session.verifier, &session.redirect_uri).await?;
        let ms: MsToken = token;
        self.authenticate(&ms).await
    }

    /// 授权码 → token。
    async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<MsToken> {
        let (status, body) = self
            .client()
            .post_form_value(
                crate::microsoft::TOKEN_URL,
                &[
                    ("client_id", self.client_id().to_string()),
                    ("scope", SCOPE.to_string()),
                    ("grant_type", "authorization_code".to_string()),
                    ("code", code.to_string()),
                    ("redirect_uri", redirect_uri.to_string()),
                    ("code_verifier", verifier.to_string()),
                ],
            )
            .await?;

        if !status.is_success() {
            bail!("授权码换取 token 失败 ({status}): {body}");
        }
        crate::microsoft::decode_token_public(&body)
    }
}

/// 接受一次浏览器回调，解析 `code`（并校验 `state`）。
async fn wait_for_code(listener: TcpListener, expected_state: &str) -> Result<String> {
    let (mut socket, _) = listener.accept().await.context("接受浏览器回调")?;

    let mut buffer = vec![0u8; 8 * 1024];
    let n = socket.read(&mut buffer).await.context("读取回调请求")?;
    let request = String::from_utf8_lossy(&buffer[..n]).to_string();

    // 例如：GET /auth?code=xxx&state=yyy HTTP/1.1
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();

    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }

    let body = if error.is_some() || code.is_none() {
        "<h2>登录未完成</h2><p>可以关闭此页面并回到 PCL-RS 重试。</p>"
    } else {
        "<h2>登录成功</h2><p>请回到 PCL-RS，窗口会自动继续。</p>"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;

    if let Some(err) = error {
        bail!("Microsoft 登录被取消或失败: {err}");
    }
    match state {
        Some(state) if state == expected_state => {}
        Some(_) => bail!("回调 state 不匹配，已拒绝本次登录"),
        None => bail!("回调缺少 state"),
    }
    code.ok_or_else(|| anyhow!("回调里没有授权码"))
}

/// 用系统默认浏览器打开 URL。
pub fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .context("打开系统浏览器")?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().context("打开系统浏览器")?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().context("打开系统浏览器")?;
    }
    Ok(())
}

fn random_urlsafe(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    // getrandom 失败时退化为时间种子：这里是 PKCE/state，不是长期密钥。
    if getrandom::getrandom(&mut bytes).is_err() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (now.rotate_left(i as u32) & 0xff) as u8;
        }
    }
    base64_url(&bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(verifier.as_bytes());
    base64_url(&digest)
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 提供给集成方（GUI/CLI）的便捷函数：开浏览器 → 等回调 → 换 token → 拿账户。
pub async fn login_with_browser(
    auth: &MicrosoftAuth,
    http: HttpClient,
) -> Result<Account> {
    let _ = &http;
    let session = auth.begin_browser_login().await?;
    open_browser(&session.authorize_url)?;
    auth.complete_browser_login(session, Duration::from_secs(300)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_base64url() {
        // RFC 7636 附录 B 的示例
        let challenge = pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn random_verifier_has_no_padding() {
        let v = random_urlsafe(48);
        assert!(!v.contains('='));
        assert!(!v.contains('+'));
        assert!(!v.contains('/'));
        assert_ne!(v, random_urlsafe(48));
    }
}
