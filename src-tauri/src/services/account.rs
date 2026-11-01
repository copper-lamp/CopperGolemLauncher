//! 账户服务：正版 MCBE（Microsoft 账户）设备码登录。
//!
//! 流程：MSA 设备码授权 → 换取 MSA access_token → Xbox Live 用户认证 → XSTS
//!（RelyingParty `http://xboxlive.com`，MCBE 客户端使用）。
//!
//! 凭证安全：refresh_token / access_token 经系统安全存储加密保存
//!（见 `platform::secret`：Windows Credential Manager / Linux Secret Service /
//! Android 待接 Keystore），数据库仅保存账户公开信息（gamertag / xuid）。
//!
//! 事件：`account.login.state`（登录流程状态）、`account.changed`（账户变化）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use reqwest::Client;

use crate::platform::secret::SharedSecretStore;
use crate::services::http_client;
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
/// Xbox 档案接口（取头像 `GameDisplayPicRaw`）。需要 XSTS 票据 + 用户 uhs。
const XBOX_PROFILE_URL: &str = "https://profile.xboxlive.com/users/xuid({xuid})/profile/settings";
/// 头像请求的上限：登录主流程在等它，宁可没有头像也不能卡住登录。
const AVATAR_TIMEOUT: Duration = Duration::from_secs(6);
const KEYRING_SERVICE: &str = "copper-golem";

/// access_token 的存储命名空间。
///
/// 与 refresh_token 分属不同 service，避免同一账户的两类令牌互相覆盖；
/// 键位与既有密钥环布局保持一致，升级后已登录用户无需重新登录。
fn access_service() -> String {
    format!("{KEYRING_SERVICE}:access")
}

/// 账户公开信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AccountInfo {
    pub id: String,
    pub gamertag: String,
    /// 真实 Xbox XUID（十进制）。
    pub xuid: Option<String>,
    /// WAM 的 `WebAccount.Id`（PUID，可能含 A-F）。只用于向 WAM 取票，不参与
    /// profile 比对或 license 绑定。
    #[serde(default)]
    pub wam_account_id: Option<String>,
    /// Xbox 档案头像（GameDisplayPicRaw）的绝对 URL。
    ///
    /// 取不到就是 None：前端回退成 gamertag 首字母。头像只是一张图，
    /// 不允许它把登录流程拖垮，所以所有获取路径都是「尽力而为」。
    #[serde(default)]
    pub avatar_url: Option<String>,
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
    secret: SharedSecretStore,
    client: Client,
    runtime: tokio::runtime::Handle,
    /// 当前登录进行中的设备码（避免重复发起；轮询任务结束即清空）。
    pending_device_code: Arc<Mutex<Option<String>>>,
}

impl AccountService {
    pub fn new(
        db: Arc<DatabaseService>,
        events: Arc<EventBus>,
        secret: SharedSecretStore,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let client = http_client::build_client(Duration::from_secs(30));
        Self {
            db,
            events,
            secret,
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
                    "SELECT id, gamertag, xuid, wam_account_id, avatar_url FROM core_account ORDER BY updated_at DESC LIMIT 1",
                    [],
                    |r| {
                        Ok(AccountInfo {
                            id: r.get(0)?,
                            gamertag: r.get(1)?,
                            xuid: r.get(2)?,
                            wam_account_id: r.get(3)?,
                            avatar_url: r.get(4)?,
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
        let secret = self.secret.clone();
        let pending = self.pending_device_code.clone();
        let interval = resp.get("interval").and_then(Value::as_u64).unwrap_or(5);
        let expires = info.expires_in_sec;
        self.runtime.spawn(async move {
            poll_login_completion(
                (db, events, client, secret),
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

    /// Xbox Live 账户登录（WAM 交互路径）。
    ///
    /// 与旧实现的关键区别：
    /// - WAM scope 用 **Xbox Live**（`service::user.auth.xboxlive.com::MBI_SSL`），
    ///   不再用 Store scope；
    /// - 票据换 XBL User Token + XSTS，从 XSTS 的 `DisplayClaims.xui[0].xid` 取
    ///   **真实 XUID**；WAM 返回的 `WebAccount.Id` 只是 PUID，单独存字段。
    ///
    /// 这条路径同时刷新系统 Xbox Identity Provider 的 TokenBroker 账户，游戏才能
    /// 用同一个账户静默登录。阻塞时长不可控，因此放到阻塞线程池执行。
    pub async fn begin_wam_sign_in(&self, hwnd: isize) -> Result<AccountInfo, KernelError> {
        self.publish_login_state(LoginState::Waiting, None);
        let outcome = self
            .runtime
            .spawn_blocking(move || crate::services::store_wam::xbox_ticket_interactive(hwnd))
            .await
            .map_err(|e| KernelError::Account(format!("账户授权任务异常退出: {e}")))?;

        let ticket = match outcome {
            Ok(ticket) => ticket,
            Err(crate::services::store_wam::WamStoreError::UserCancelled) => {
                self.publish_login_state(LoginState::Failed, Some("已取消账户授权"));
                return Err(KernelError::Account("已取消账户授权".into()));
            }
            Err(e) => {
                let reason = e.to_string();
                self.publish_login_state(LoginState::Failed, Some(&reason));
                return Err(KernelError::Account(reason));
            }
        };

        // WAM 票据 → XBL User Token → XSTS。失败同样要落状态，不能停在「等待中」。
        let result: Result<(String, String, Option<String>), KernelError> = async {
            let xbl = exchange_xbox_wam(&self.client, &ticket.ticket).await?;
            // xid / gtg / uhs 都在 XSTS **响应体**的 DisplayClaims 里，不在 Token 的 JWT 里。
            let xsts = exchange_xsts_with_identity(&self.client, &xbl).await?;
            let XstsAuth { token, uhs, xuid, gamertag } = xsts;
            let xuid = xuid.ok_or_else(|| {
                KernelError::Account("XSTS 响应里没有 xid（不是 Xbox 账户）".into())
            })?;
            let avatar = fetch_avatar(&self.client, &token, uhs.as_deref(), Some(&xuid)).await;
            Ok((xuid, gamertag, avatar))
        }
        .await;

        let (xuid, gamertag, avatar_url) = match result {
            Ok(value) => value,
            Err(error) => {
                self.publish_login_state(LoginState::Failed, Some(&error.to_string()));
                return Err(error);
            }
        };

        let display_name = if gamertag.trim().is_empty() {
            xuid.clone()
        } else {
            gamertag
        };
        let account =
            self.save_account(display_name, Some(xuid), Some(ticket.account_id), avatar_url)?;
        self.publish_login_state(LoginState::Done, None);
        Ok(account)
    }

    /// 退出登录：清除安全存储中的凭证与数据库记录。
    pub fn logout(&self) -> Result<(), KernelError> {
        // 清除凭证的失败必须留痕：登出的语义是「令牌不再可用」，静默忽略会让
        // 「点了登出但令牌仍留在盘上」这种安全事件完全不可见。
        // 仍继续执行库清理——否则一次密钥环故障会把用户卡在「登不出去」的状态。
        if let Some(account) = self.current() {
            if let Err(e) = self.secret.delete(KEYRING_SERVICE, &account.id) {
                log::warn!("[account] 登出时清除刷新令牌失败: {e}");
            }
            if let Err(e) = self.secret.delete(&access_service(), &account.id) {
                log::warn!("[account] 登出时清除访问令牌失败: {e}");
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

    /// 补齐头像：只对「已登录但没有头像」的账户跑一次。
    ///
    /// 旧版本登录写下的账户行没有 `avatar_url`。没有这条补取路径，老用户必须
    /// 退出再登录才能看到自己的头像——那不是一个可接受的修复。
    /// 返回最新头像 URL；仍为 None 时说明取不到（例如只完成了商店授权、没有 MSA 凭据）。
    pub async fn ensure_avatar(&self) -> Result<Option<String>, KernelError> {
        let account = self
            .current()
            .ok_or_else(|| KernelError::Account("未登录".into()))?;
        if let Some(url) = account.avatar_url.clone() {
            return Ok(Some(url));
        }
        let xuid = account
            .xuid
            .clone()
            .ok_or_else(|| KernelError::Account("账户缺少 XUID，无法获取头像".into()))?;
        // 两条取票路径：有 MSA 令牌就走设备码那套；只完成过商店授权的账户没有
        // MSA 令牌，只能复用 WAM 缓存账户静默取票（不弹窗）。
        let xsts = match self.xsts_via_msa(&account).await {
            Some(xsts) => xsts,
            None => self.xsts_via_wam().await.ok_or_else(|| {
                KernelError::Account("暂时取不到 Xbox 票据，头像稍后再试".into())
            })?,
        };
        let avatar =
            fetch_avatar(&self.client, &xsts.token, xsts.uhs.as_deref(), Some(&xuid)).await;
        if let Some(url) = avatar.clone() {
            self.update_avatar(&account.id, &url)?;
        }
        Ok(avatar)
    }

    /// 用 MSA 令牌换 XSTS。access_token 只有一小时有效期，老账户行里的那份几乎
    /// 必然过期，所以失败后先用 refresh_token 换一张新的再试一次。
    async fn xsts_via_msa(&self, account: &AccountInfo) -> Option<XstsAuth> {
        let Ok(ms_token) = self.load_ms_token(account) else {
            return None;
        };
        let mut xbl = exchange_xbox(&self.client, &ms_token).await;
        if xbl.is_err() {
            self.do_refresh(account).await.ok()?;
            let retry = self.load_ms_token(account).ok()?;
            xbl = exchange_xbox(&self.client, &retry).await;
        }
        exchange_xsts_with_identity(&self.client, &xbl.ok()?).await.ok()
    }

    /// 复用 WAM 缓存账户静默取 Xbox 票据（不弹系统界面）。
    ///
    /// 只完成商店授权（WAM）的账户没有 MSA 令牌，这条是它唯一的取头像通道。
    /// WAM 是阻塞 API，必须放到阻塞线程池，不能占用 async 线程。
    async fn xsts_via_wam(&self) -> Option<XstsAuth> {
        #[cfg(windows)]
        {
            let client = self.client.clone();
            let ticket = self
                .runtime
                .spawn_blocking(crate::services::store_wam::xbox_ticket_silent)
                .await
                .ok()?
                .ok()?;
            let xbl = exchange_xbox_wam(&client, &ticket.ticket).await.ok()?;
            exchange_xsts_with_identity(&client, &xbl).await.ok()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// 头像落库并广播 `account.changed`：前端不用重新拉取账户就能换上新头像。
    fn update_avatar(&self, account_id: &str, url: &str) -> Result<(), KernelError> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE core_account SET avatar_url = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![url, now_ms, account_id],
            )?;
            Ok(())
        })?;
        if let Some(account) = self.current() {
            self.events.publish("account.changed", json!({ "account": account }));
        }
        Ok(())
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

    /// 写入账户记录并广播 `account.changed`。
    fn save_account(
        &self,
        gamertag: String,
        xuid: Option<String>,
        wam_account_id: Option<String>,
        avatar_url: Option<String>,
    ) -> Result<AccountInfo, KernelError> {
        let account = AccountInfo {
            id: Uuid::new_v4().to_string(),
            gamertag,
            xuid,
            wam_account_id,
            avatar_url,
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO core_account
                     (id, gamertag, xuid, wam_account_id, avatar_url, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                rusqlite::params![
                    account.id,
                    account.gamertag,
                    account.xuid,
                    account.wam_account_id,
                    account.avatar_url,
                    now_ms
                ],
            )?;
            Ok(())
        })?;
        self.events.publish("account.changed", json!({ "account": account.clone() }));
        Ok(account)
    }

    fn load_ms_token(&self, account: &AccountInfo) -> Result<String, KernelError> {
        self.secret
            .get(&access_service(), &account.id)
            .map_err(KernelError::Secret)?
            .ok_or_else(|| {
                // 两条登录路径写入的是同一张 `core_account` 行，但只有设备码登录会往
                // 密钥环写 MSA 令牌。仅完成商店授权（WAM）时必然落到这里，因此文案
                // 必须同时点出两种成因，不能笼统说成「凭证过期」。
                KernelError::Account(
                    "缺少启动正版所需的 MSA 凭据：若仅完成了商店授权，请改用「登录正版（Microsoft）」完成设备码登录；若已登录过，可能是凭证已失效，请重新登录"
                        .into(),
                )
            })
    }

    async fn do_refresh(&self, account: &AccountInfo) -> Result<(), KernelError> {
        let refresh_token = self
            .secret
            .get(KEYRING_SERVICE, &account.id)
            .map_err(KernelError::Secret)?
            .ok_or_else(|| KernelError::Account("本地凭证缺失，请重新登录".into()))?;
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

        // MSA 每次刷新都可能轮换 refresh_token，**旧的随即失效**：轮换值没落盘
        // 就等于本地留了一个死令牌，下次刷新必然失败。因此这里必须上抛，而不是
        // 像此前那样 `let _ =` 吞掉——吞掉的代价是用户过一会儿莫名其妙掉登录。
        if new_refresh != refresh_token {
            self.secret
                .set(KEYRING_SERVICE, &account.id, new_refresh)
                .map_err(|e| KernelError::Secret(format!("保存轮换后的刷新令牌失败: {e}")))?;
        }
        // access_token 是短期的可再生产物：写失败只影响下一次刷新的时机，故不致命，
        // 但仍要留痕，避免「每次启动都刷新」这类问题无从定位。
        if let Err(e) = self.secret.set(&access_service(), &account.id, access) {
            log::warn!("[account] 写入 access_token 失败（不影响登录状态）: {e}");
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
    (db, events, client, secret): (
        Arc<DatabaseService>,
        Arc<EventBus>,
        Client,
        SharedSecretStore,
    ),
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
        let xsts = exchange_xsts_with_identity(&client, &xbl).await?;
        let XstsAuth { token, uhs, xuid, gamertag } = xsts;
        let avatar = fetch_avatar(&client, &token, uhs.as_deref(), xuid.as_deref()).await;
        Ok::<_, KernelError>((xuid, gamertag, avatar))
    }
    .await;

    match result {
        Ok((xuid, gamertag, avatar_url)) => {
            let id = Uuid::new_v4().to_string();
            let account = AccountInfo {
                id: id.clone(),
                gamertag,
                xuid,
                wam_account_id: None,
                avatar_url,
            };
            // 先把凭证写进安全存储，**写失败即视为登录失败**。
            //
            // 此前这里用 `let _ =` 吞掉写入错误，再照常往数据库插账户行：结果是界面
            // 显示「已登录」，但 `core_account` 有行、安全存储里没令牌，用户重启后
            // 掉登录且没有任何线索（本仓库的「假成功」问题）。
            // 顺序上先写 refresh_token（唯一不可再生的一条），再写 access_token：
            // access_token 可用 refresh_token 重新换发，反之不成立。
            let mut stored_refresh = false;
            if let Some(refresh) = &ms_token.1 {
                if let Err(e) = secret.set(KEYRING_SERVICE, &id, refresh) {
                    finish_login(
                        &events,
                        &pending,
                        LoginState::Failed,
                        Some(&format!("保存登录凭证失败: {e}")),
                    );
                    return;
                }
                stored_refresh = true;
            }
            if let Err(e) = secret.set(&access_service(), &id, &ms_token.0) {
                // 回滚已写入的 refresh_token：宁可让用户重新登录，也不留下「半套
                // 凭证」——那会在下次刷新时以「本地凭证缺失」的形式误导排查。
                let _ = secret.delete(&access_service(), &id);
                if stored_refresh {
                    let _ = secret.delete(KEYRING_SERVICE, &id);
                }
                finish_login(
                    &events,
                    &pending,
                    LoginState::Failed,
                    Some(&format!("保存登录凭证失败: {e}")),
                );
                return;
            }
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let result = db.with_conn(|conn| {
                conn.execute(
                    "INSERT INTO core_account
                         (id, gamertag, xuid, wam_account_id, avatar_url, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                    rusqlite::params![
                        account.id,
                        account.gamertag,
                        account.xuid,
                        account.wam_account_id,
                        account.avatar_url,
                        now_ms
                    ],
                )?;
                Ok(())
            });
            if let Err(e) = result {
                // 库写入失败同样不能留下孤儿凭证。
                let _ = secret.delete(KEYRING_SERVICE, &account.id);
                let _ = secret.delete(&access_service(), &account.id);
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

/// WAM 票据换 XBL User Token。
///
/// WAM 的 Xbox scope 票据本身就是 RPS ticket，**不能再加 `d=` 前缀**。
/// LeviLauncher 的 `xblUserAuth` 也是原样传入。失败时把响应体带出来，避免
/// 只看到一句「网络请求失败」。
async fn exchange_xbox_wam(client: &Client, ticket: &str) -> Result<String, KernelError> {
    let response = client
        .post(XBOX_AUTH_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": ticket,
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(KernelError::Account(format!(
            "XBL 用户认证失败（{status}）：{}",
            body.chars().take(300).collect::<String>()
        )));
    }
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| KernelError::Account(format!("XBL 响应不是 JSON: {e}")))?;
    value
        .get("Token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .ok_or_else(|| KernelError::Account("XBL 响应缺少 Token".into()))
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

/// XSTS 响应：Token + 身份（xid / gtg 在响应体的 DisplayClaims 里）。
#[derive(Debug, Clone)]
struct XstsAuth {
    token: String,
    /// 用户的私有化哈希（uhs）。请求 Xbox 档案接口时需要 `XBL3.0 x=<uhs>;<token>`
    /// 形式的授权头，和 Token 一样来自 XSTS 响应体。
    uhs: Option<String>,
    xuid: Option<String>,
    gamertag: String,
}

/// XSTS 授权（RelyingParty `http://xboxlive.com`，MCBE 使用）。
///
/// 身份必须从**响应体**的 `DisplayClaims.xui[0]` 取：`Token` 是 JWT，但
/// `DisplayClaims` 是响应体的顶层字段，不在 JWT payload 里。
async fn exchange_xsts_with_identity(
    client: &Client,
    xbl_token: &str,
) -> Result<XstsAuth, KernelError> {
    let response = client
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
        .await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(KernelError::Account(format!(
            "XSTS 授权失败（{status}）：{}",
            body.chars().take(300).collect::<String>()
        )));
    }
    let resp: Value = serde_json::from_str(&body)
        .map_err(|e| KernelError::Account(format!("XSTS 响应不是 JSON: {e}")))?;
    let token = resp
        .get("Token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| KernelError::Account("XSTS 响应缺少 Token".into()))?;
    let claim = resp
        .get("DisplayClaims")
        .and_then(|claims| claims.get("xui"))
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let xuid = claim
        .and_then(|item| item.get("xid"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let gamertag = claim
        .and_then(|item| item.get("gtg").or_else(|| item.get("gt")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "玩家".to_string());
    let uhs = claim
        .and_then(|item| item.get("uhs"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(XstsAuth {
        token,
        uhs,
        xuid,
        gamertag,
    })
}

/// 取 Xbox 档案头像 URL（`GameDisplayPicRaw`）。
///
/// 失败一律返回 None 并只记一条 warn：头像缺失是可以接受的降级，
/// 但把它变成登录失败就会让用户莫名其妙地登不进去。
///
/// 授权头是 `XBL3.0 x=<uhs>;<xsts_token>`，两个值都来自同一次 XSTS 响应；
/// 少了 uhs 服务端只会回 401，所以这里直接放弃，不去发一次注定失败的请求。
async fn fetch_avatar(
    client: &Client,
    xsts_token: &str,
    uhs: Option<&str>,
    xuid: Option<&str>,
) -> Option<String> {
    let (Some(uhs), Some(xuid)) = (uhs, xuid) else {
        log::debug!("[account] 缺少 uhs/xuid，跳过头像获取");
        return None;
    };
    let url = XBOX_PROFILE_URL.replace("{xuid}", xuid);
    let request = client
        .get(&url)
        .query(&[("settings", "GameDisplayPicRaw")])
        .header("Authorization", format!("XBL3.0 x={uhs};{xsts_token}"))
        .header("x-xbl-contract-version", "3")
        .header("Accept", "application/json")
        .send();
    let response = match tokio::time::timeout(AVATAR_TIMEOUT, request).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            log::warn!("[account] 头像请求失败: {error}");
            return None;
        }
        Err(_) => {
            log::warn!("[account] 头像请求超时");
            return None;
        }
    };
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        log::warn!(
            "[account] 头像接口返回 {status}: {}",
            body.chars().take(200).collect::<String>()
        );
        return None;
    }
    let body: Value = match response.json().await {
        Ok(value) => value,
        Err(error) => {
            log::warn!("[account] 头像响应不是 JSON: {error}");
            return None;
        }
    };
    avatar_from_profile(&body)
}

/// 从档案响应体里挑出头像地址（单独成函数，便于单测）。
fn avatar_from_profile(body: &Value) -> Option<String> {
    body.get("profileUsers")?
        .as_array()?
        .first()?
        .get("settings")?
        .as_array()?
        .iter()
        .find(|setting| setting.get("id").and_then(Value::as_str) == Some("GameDisplayPicRaw"))
        .and_then(|setting| setting.get("value"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 只要 XSTS token 的调用方。
async fn exchange_xsts(client: &Client, xbl_token: &str) -> Result<String, KernelError> {
    Ok(exchange_xsts_with_identity(client, xbl_token).await?.token)
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
        .and_then(|u| u.get("gtg").or_else(|| u.get("gt")))
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
    fn avatar_from_profile_picks_display_picture() {
        let body = serde_json::json!({
            "profileUsers": [{
                "id": "2535467518347612",
                "settings": [
                    { "id": "Gamerscore", "value": "1234" },
                    { "id": "GameDisplayPicRaw", "value": "https://images-eds.xboxlive.com/image?url=abc" }
                ]
            }]
        });
        assert_eq!(
            avatar_from_profile(&body).as_deref(),
            Some("https://images-eds.xboxlive.com/image?url=abc")
        );
    }

    #[test]
    fn avatar_from_profile_rejects_empty_and_missing() {
        assert_eq!(avatar_from_profile(&serde_json::json!({})), None);
        let blank = serde_json::json!({
            "profileUsers": [{ "settings": [{ "id": "GameDisplayPicRaw", "value": "   " }] }]
        });
        assert_eq!(avatar_from_profile(&blank), None);
    }

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
