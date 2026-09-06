//! 账户服务：正版 MCBE（Microsoft 账户）设备码登录。
//!
//! 流程：MSA 设备码授权 → 换取 MSA access_token → Xbox Live 用户认证 → XSTS
//!（RelyingParty `http://xboxlive.com`，MCBE 客户端使用）。
//!
//! 凭证安全：refresh_token / access_token 经系统密钥环加密存储
//!（Windows Credential Manager / macOS Keychain / Linux Secret Service），
//! 数据库仅保存账户公开信息（gamertag / xuid）。
//!
//! 事件：`account.login.state`（登录流程状态）、`account.changed`（账户变化）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::database::DatabaseService;

/// Microsoft 应用客户端 ID。
/// 默认使用公开的 Minecraft: Bedrock Android 客户端 ID（社区设备码流程通用），
/// 正式发布前应替换为自注册的 Azure 应用 ID。
const MS_CLIENT_ID: &str = "0000000048183522";
const MSA_DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const MSA_TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBOX_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const KEYRING_SERVICE: &str = "copper-golem";

/// 账户公开信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AccountInfo {
    pub id: String,
    pub gamertag: String,
    pub xuid: Option<String>,
}

/// 设备码信息（前端展示授权链接与用户码）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DeviceCodeInfo {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub message: String,
    pub expires_in_sec: u64,
}

/// 登录流程状态（经 `account.login.state` 广播）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginState {
    Waiting,
    Done,
    Failed,
}

/// 账户服务。
pub struct AccountService {
    db: Arc<DatabaseService>,
    events: Arc<EventBus>,
    client: Client,
    runtime: tokio::runtime::Handle,
    /// 当前登录进行中的设备码（避免重复发起；轮询任务结束即清空）。
    pending_device_code: Arc<Mutex<Option<String>>>,
}

impl AccountService {
    pub fn new(
        db: Arc<DatabaseService>,
        events: Arc<EventBus>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");
        Self {
            db,
            events,
            client,
            runtime,
            pending_device_code: Arc::new(Mutex::new(None)),
        }
    }

    /// 当前登录账户（数据库单行）。
    pub fn current(&self) -> Option<AccountInfo> {
        self.db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT id, gamertag, xuid FROM core_account ORDER BY updated_at DESC LIMIT 1",
                    [],
                    |r| {
                        Ok(AccountInfo {
                            id: r.get(0)?,
                            gamertag: r.get(1)?,
                            xuid: r.get(2)?,
                        })
                    },
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(KernelError::Database(other)),
                })
            })
            .unwrap_or(None)
    }

    /// 发起设备码登录：返回授权信息并后台轮询授权结果。
    pub async fn begin_login(&self) -> Result<DeviceCodeInfo, KernelError> {
        if self.pending_device_code.lock().is_some() {
            return Err(KernelError::Account("已有进行中的登录流程".into()));
        }
        let params = [
            ("client_id", MS_CLIENT_ID),
            ("scope", "XboxLive.signin offline_access"),
        ];
        let resp: Value = self
            .client
            .post(MSA_DEVICE_CODE_URL)
            .form(&params)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let device_code = resp
            .get("device_code")
            .and_then(Value::as_str)
            .ok_or_else(|| KernelError::Account("设备码响应缺少 device_code".into()))?
            .to_string();
        let info = DeviceCodeInfo {
            device_code: device_code.clone(),
            user_code: resp
                .get("user_code")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            verification_uri: resp
                .get("verification_uri")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            message: resp
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            expires_in_sec: resp.get("expires_in").and_then(Value::as_u64).unwrap_or(900),
        };

        *self.pending_device_code.lock() = Some(device_code.clone());
        self.publish_login_state(LoginState::Waiting, None);

        let (db, events, client) = self.clone_handles();
        let pending = self.pending_device_code.clone();
        let interval = resp.get("interval").and_then(Value::as_u64).unwrap_or(5);
        let expires = info.expires_in_sec;
        self.runtime.spawn(async move {
            poll_login_completion(
                (db, events, client),
                pending,
                device_code,
                interval,
                expires,
            )
            .await;
        });

        // 打开浏览器引导授权。
        let _ = tauri_plugin_opener::open_url(info.verification_uri.clone(), None::<String>);

        Ok(info)
    }

    /// 退出登录：清除密钥环与数据库记录。
    pub fn logout(&self) -> Result<(), KernelError> {
        if let Some(account) = self.current() {
            if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &account.id) {
                let _ = entry.delete_credential();
            }
        }
        self.db.with_conn(|conn| {
            conn.execute("DELETE FROM core_account", [])?;
            Ok(())
        })?;
        self.events.publish("account.changed", json!({ "account": null }));
        Ok(())
    }

    /// 手动刷新当前账户令牌（失败时前端提示重新登录）。
    pub async fn refresh(&self) -> Result<(), KernelError> {
        let account = self
            .current()
            .ok_or_else(|| KernelError::Account("未登录".into()))?;
        self.do_refresh(&account).await
    }

    /// 供游戏启动使用：取当前账户的 MSA + XSTS 凭证（自动刷新）。
    pub async fn credentials(&self) -> Result<Value, KernelError> {
        let account = self
            .current()
            .ok_or_else(|| KernelError::Account("未登录".into()))?;
        let ms_token = self.load_ms_token(&account)?;
        let xsts = self.exchange_xsts(&ms_token).await?;
        Ok(json!({
            "gamertag": account.gamertag,
            "xuid": account.xuid,
            "xsts_token": xsts,
        }))
    }

    // ---- 内部实现 ----

    fn clone_handles(&self) -> (Arc<DatabaseService>, Arc<EventBus>, Client) {
        (self.db.clone(), self.events.clone(), self.client.clone())
    }

    fn publish_login_state(&self, state: LoginState, reason: Option<&str>) {
        self.events.publish(
            "account.login.state",
            json!({ "state": state, "reason": reason }),
        );
    }

    fn load_ms_token(&self, account: &AccountInfo) -> Result<String, KernelError> {
        let entry = keyring::Entry::new(&format!("{KEYRING_SERVICE}:access"), &account.id)?;
        entry.get_password().map_err(|_| {
            KernelError::Account("本地凭证缺失，请重新登录".into())
        })
    }

    async fn do_refresh(&self, account: &AccountInfo) -> Result<(), KernelError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &account.id)?;
        let refresh_token = entry.get_password()?;
        let resp = self
            .client
            .post(MSA_TOKEN_URL)
            .form(&[
                ("client_id", MS_CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh_token),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let access = resp
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| KernelError::Account("刷新令牌响应缺少 access_token".into()))?;
        let new_refresh = resp
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or(&refresh_token);
        if let Ok(entry) = keyring::Entry::new(&format!("{KEYRING_SERVICE}:access"), &account.id) {
            let _ = entry.set_password(access);
        }
        if new_refresh != refresh_token {
            let _ = entry.set_password(new_refresh);
        }
        Ok(())
    }

    /// 用 MSA access_token 换取 XSTS（RelyingParty xboxlive.com）。
    async fn exchange_xsts(&self, ms_access: &str) -> Result<String, KernelError> {
        let xbl = exchange_xbox(&self.client, ms_access).await?;
        exchange_xsts(&self.client, &xbl).await
    }
}

/// 后台轮询授权并完成登录。`pending` 用于在流程结束时清空"进行中"标记，
/// 以便用户失败后能再次发起登录。
async fn poll_login_completion(
    (db, events, client): (Arc<DatabaseService>, Arc<EventBus>, Client),
    pending: Arc<Mutex<Option<String>>>,
    device_code: String,
    interval_sec: u64,
    expires_in_sec: u64,
) {
    let deadline = Instant::now() + Duration::from_secs(expires_in_sec.max(60));
    let interval = Duration::from_secs(interval_sec.max(1));

    let ms_token = loop {
        if Instant::now() >= deadline {
            finish_login(&events, &pending, LoginState::Failed, Some("授权超时，请重试"));
            return;
        }
        tokio::time::sleep(interval).await;

        let resp = client
            .post(MSA_TOKEN_URL)
            .form(&[
                ("client_id", MS_CLIENT_ID),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &device_code),
            ])
            .send()
            .await;

        let body: Value = match resp {
            Ok(r) => r.json().await.unwrap_or(Value::Null),
            Err(e) => {
                finish_login(&events, &pending, LoginState::Failed, Some(&format!("网络错误: {e}")));
                return;
            }
        };

        if let Some(access) = body.get("access_token").and_then(Value::as_str) {
            break (
                access.to_string(),
                body.get("refresh_token")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            );
        }
        // 错误处理：authorization_pending 继续等待，其余失败终止。
        let error = body.get("error").and_then(Value::as_str).unwrap_or("unknown");
        match error {
            "authorization_pending" => continue,
            "authorization_declined" | "expired_token" | "bad_verification_code" => {
                finish_login(&events, &pending, LoginState::Failed, Some("授权未完成或被拒绝"));
                return;
            }
            other => {
                finish_login(&events, &pending, LoginState::Failed, Some(&format!("授权失败: {other}")));
                return;
            }
        }
    };

    // Xbox Live 用户认证 + XSTS。
    let result = async {
        let xbl = exchange_xbox(&client, &ms_token.0).await?;
        let (xuid, gamertag) = xbl_identity(&xbl);
        let _xsts = exchange_xsts(&client, &xbl).await?;
        Ok::<_, KernelError>((xuid, gamertag))
    }
    .await;

    match result {
        Ok((xuid, gamertag)) => {
            let id = Uuid::new_v4().to_string();
            let account = AccountInfo {
                id: id.clone(),
                gamertag,
                xuid,
            };
            // 加密存 refresh_token；access_token 短期有效，也一并加密存储。
            if let Some(refresh) = &ms_token.1 {
                if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &id) {
                    let _ = entry.set_password(refresh);
                }
            }
            if let Ok(entry) = keyring::Entry::new(&format!("{KEYRING_SERVICE}:access"), &id) {
                let _ = entry.set_password(&ms_token.0);
            }
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let result = db.with_conn(|conn| {
                conn.execute(
                    "INSERT INTO core_account (id, gamertag, xuid, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    rusqlite::params![account.id, account.gamertag, account.xuid, now_ms],
                )?;
                Ok(())
            });
            if let Err(e) = result {
                finish_login(&events, &pending, LoginState::Failed, Some(&format!("保存账户失败: {e}")));
                return;
            }
            finish_login(&events, &pending, LoginState::Done, None);
            events.publish("account.changed", json!({ "account": account }));
        }
        Err(e) => {
            finish_login(&events, &pending, LoginState::Failed, Some(&e.friendly()));
        }
    }
}

/// 结束登录流程：清空进行中标记并广播最终状态。
fn finish_login(
    events: &EventBus,
    pending: &Arc<Mutex<Option<String>>>,
    state: LoginState,
    reason: Option<&str>,
) {
    *pending.lock() = None;
    events.publish("account.login.state", json!({ "state": state, "reason": reason }));
}

/// Xbox Live 用户认证。
async fn exchange_xbox(client: &Client, ms_access: &str) -> Result<String, KernelError> {
    let resp = client
        .post(XBOX_AUTH_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    resp.get("Token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| KernelError::Account("Xbox 认证响应缺少 Token".into()))
}

/// XSTS 授权（RelyingParty `http://xboxlive.com`，MCBE 使用）。
async fn exchange_xsts(client: &Client, xbl_token: &str) -> Result<String, KernelError> {
    let resp = client
        .post(XSTS_AUTH_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token],
            },
            "RelyingParty": "http://xboxlive.com",
            "TokenType": "JWT",
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    resp.get("Token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| KernelError::Account("XSTS 响应缺少 Token".into()))
}

/// 从 Xbox 认证响应提取身份（xuid / gamertag）。
fn xbl_identity(xbl_token: &str) -> (Option<String>, String) {
    // 尝试从 JWT payload 中解出 DisplayClaims（不严格校验签名，仅读取）。
    let claims = decode_jwt_payload(xbl_token);
    let xui = claims
        .as_ref()
        .and_then(|c| c.get("DisplayClaims"))
        .and_then(|c| c.get("xui"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first());
    let xuid = xui.and_then(|u| u.get("xid")).and_then(Value::as_str).map(str::to_string);
    let gamertag = xui
        .and_then(|u| u.get("gt"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "玩家".to_string());
    (xuid, gamertag)
}

/// 解 JWT 的 payload 段（base64url 解码 + JSON 解析，失败返回 None）。
fn decode_jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn decode_jwt_payload_extracts_claims() {
        // header.payload.signature（payload 为 base64url 的 {"xid":"123","gt":"Steve"}）
        let token = format!(
            "eyJhbGciOiJIUzI1NiJ9.{}.c2ln",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(r#"{"xid":"2535467518347612","gt":"Steve"}"#)
        );
        let claims = decode_jwt_payload(&token).expect("jwt payload should decode");
        assert_eq!(claims["xid"], "2535467518347612");
        assert_eq!(claims["gt"], "Steve");
    }

    #[test]
    fn xbl_identity_extracts_display_claims() {
        let payload = r#"{"DisplayClaims":{"xui":[{"xid":"1","gt":"Copper","uhs":"abc"}]}}"#;
        let token = format!(
            "h.{}.s",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
        );
        let (xuid, gt) = xbl_identity(&token);
        assert_eq!(xuid.as_deref(), Some("1"));
        assert_eq!(gt, "Copper");
    }

    #[test]
    fn xbl_identity_falls_back_to_placeholder() {
        let (_, gt) = xbl_identity("not-a-jwt");
        assert_eq!(gt, "玩家");
    }
}
