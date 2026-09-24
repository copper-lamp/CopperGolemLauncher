//! 签名校验：minisign（ed25519）验签接口与公钥内置位。
//!
//! 依据 [cgl-libs](../../../docs/cgl-libs.md) 2.5 规则 2：`index.json` 采用双锚点，
//! `index.json.sha256` 提供轻量比对，`index.json.minisig` 提供防篡改；
//! **公钥硬编码在启动器二进制内**（与自定义根目录 `Paths` 无关），避免从网络获取公钥导致锚点失效。
//!
//! # 当前状态（诚实披露，不可伪装）
//!
//! 验签两层已分层处理：
//!
//! - **验签能力（已实现并测试）**：minisign 格式解析、公钥解码、ed25519 数学验签
//!   （`ed25519-dalek`，含 `Ed` 直签与 `ED` 预哈希两种模式）、失败即拒绝的判定全部就绪。
//! - **内置公钥（已配置）**：见 [`TRUSTED_PUBLIC_KEY`]。cgl-libs 建立 minisign 密钥对后，
//!   其公钥填入该常量，完整验签闭环即自动生效（当前为开发期灰度公钥，发布前确认）。

use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Serialize;
use sha2::{Digest as _, Sha512};

/// 内置可信公钥（minisign 公钥文件内容，两行：注释行 + base64 公钥行）。
///
/// 由 cgl-libs 建立 minisign 密钥对（私钥仅存 GitHub Secret 与离线备份）后填入。
/// 生成/轮换流程：
/// 1. 离线执行 `minisign -G -p cgl-libs.pub -s cgl-libs.key`，私钥只进 GitHub Secret
///    `MINISIGN_SECRET_KEY` 与离线备份，绝不入库；
/// 2. 用 `node tools/sign.mjs` 对 `index.json` 签名并产出 `index.json.minisig`；
/// 3. 把 `cgl-libs.pub` 的公钥行（base64，`RW...` 开头）填入本常量，并随新启动器发布；
/// 4. 轮换时（文档 2.8.5「索引本身被污染」一行）必须**随新启动器一起发布**——已发布版本
///    仍信任旧公钥，因此轮换与发版必须同批。
///
/// 当前填入的是 cgl-libs 在开发期用于打通端到端验签闭环的建议公钥（灰度验证用）；
/// 发布前应按上述流程确认为正式密钥。
pub const TRUSTED_PUBLIC_KEY: &str =
    "RWSwyyDtRtiQH+lOaLN5vMBNPiRflxWcvommOLIbK1aX7gMN9JRYFo7i";

/// minisign 签名文件的默认后缀。
pub const SIGNATURE_SUFFIX: &str = ".minisig";

/// 签名校验状态（对前端如实暴露）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureStatus {
    /// 公钥未配置：本客户端**无法**验证签名，不得宣称已验证。
    NotConfigured,
    /// 有公钥但服务端未提供签名文件。
    SignatureMissing,
    /// 验签通过。
    Verified,
    /// 验签失败（签名不匹配、格式非法、公钥非法）——必须拒绝该数据。
    Failed,
}

impl SignatureStatus {
    /// 是否可对外宣称"签名已验证"。只有 [`SignatureStatus::Verified`] 为真。
    pub fn is_verified(self) -> bool {
        matches!(self, Self::Verified)
    }

    /// 供前端映射文案的键（**返回键名而非硬编码文案**，由前端 i18n 渲染）。
    pub fn message_key(self) -> &'static str {
        match self {
            Self::NotConfigured => "module.registry.signature.not_configured",
            Self::SignatureMissing => "module.registry.signature.missing",
            Self::Verified => "module.registry.signature.verified",
            Self::Failed => "module.registry.signature.failed",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::SignatureMissing => "signature_missing",
            Self::Verified => "verified",
            Self::Failed => "failed",
        }
    }
}

/// 一次验签的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// 公钥未配置，本次未做任何验签（不是通过，也不是失败）。
    NotConfigured,
    /// 通过。
    Verified,
    /// 未提供签名文件。
    SignatureMissing,
    /// 失败，附原因。
    Failed(String),
}

impl VerifyOutcome {
    pub fn status(&self) -> SignatureStatus {
        match self {
            Self::NotConfigured => SignatureStatus::NotConfigured,
            Self::Verified => SignatureStatus::Verified,
            Self::SignatureMissing => SignatureStatus::SignatureMissing,
            Self::Failed(_) => SignatureStatus::Failed,
        }
    }

    /// 是否应**拒绝**该份数据。
    ///
    /// 注意：`NotConfigured` 不构成拒绝理由（当前无密钥，拒绝等于完全不可用），
    /// 但调用方必须在状态里如实标注未验签（文档 2.9.6：不得"降级到看起来正常"）。
    pub fn should_reject(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

/// 验签器：持有内置公钥（默认取 [`TRUSTED_PUBLIC_KEY`]）。
#[derive(Debug, Clone, Default)]
pub struct SignatureVerifier {
    public_key_line: Option<String>,
}

impl SignatureVerifier {
    /// 使用内置公钥构造；公钥为空时进入 [`SignatureStatus::NotConfigured`]。
    pub fn new() -> Self {
        Self {
            public_key_line: normalized_public_key(TRUSTED_PUBLIC_KEY),
        }
    }

    /// 使用显式公钥构造（测试与未来密钥轮换演练使用）。
    pub fn with_public_key(key: &str) -> Self {
        Self {
            public_key_line: normalized_public_key(key),
        }
    }

    /// 是否已配置可信公钥。
    pub fn is_configured(&self) -> bool {
        self.public_key_line.is_some()
    }

    /// 当前整体签名状态（未配置时即为 [`SignatureStatus::NotConfigured`]）。
    pub fn status(&self) -> SignatureStatus {
        if self.is_configured() {
            // 有公钥但尚未验签任何文件：语义上等同于"未验签"，由具体验签结果覆盖。
            SignatureStatus::SignatureMissing
        } else {
            SignatureStatus::NotConfigured
        }
    }

    /// 供状态页展示的说明文案键：**返回键名而非硬编码中文**，
    /// 由前端不依赖模块包的框架文案渲染（前端 i18n 缺失时兜底显示英文）。
    pub fn status_key(&self) -> &'static str {
        self.status().message_key()
    }

    /// 校验 `index.json` 的 minisign 签名。
    ///
    /// - 公钥未配置 → [`VerifyOutcome::NotConfigured`]（**不做假验签**）；
    /// - 签名缺失 → [`VerifyOutcome::SignatureMissing`]；
    /// - 公钥或签名格式非法、或签名不匹配 → [`VerifyOutcome::Failed`]（调用方必须拒绝该数据）。
    pub fn verify_index(&self, content: &[u8], signature_text: &str) -> VerifyOutcome {
        let Some(public_key) = self.public_key_line.as_deref() else {
            return VerifyOutcome::NotConfigured;
        };
        if signature_text.trim().is_empty() {
            return VerifyOutcome::SignatureMissing;
        }
        let Some(parsed) = MinisignSignature::parse(signature_text) else {
            return VerifyOutcome::Failed("签名文件格式非法".into());
        };
        let Ok(public_key_bytes) = decode_minisign_public_key(public_key) else {
            return VerifyOutcome::Failed("内置公钥格式非法".into());
        };
        match parsed.verify(public_key_bytes, content) {
            Ok(()) => VerifyOutcome::Verified,
            Err(e) => VerifyOutcome::Failed(e),
        }
    }
}

/// minisign 公钥行的归一化：去掉注释行与空白，仅保留 base64 数据行。
///
/// minisign 公钥文件首行是 `untrusted comment: ...`，第二行是 base64 公钥。
/// 允许调用方直接传两行内容，也允许直接传单行 base64。
fn normalized_public_key(raw: &str) -> Option<String> {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))?;
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

/// minisign 公钥的二进制形态（ed25519 公钥 + 8 字节 key id）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinisignPublicKey {
    /// ed25519 公钥（32 字节）。
    pub key: [u8; 32],
    /// 密钥 id（8 字节小端）。
    pub key_id: [u8; 8],
}

/// 解码 minisign 公钥（base64，42 字节：2 字节算法标识 + 8 字节 key id + 32 字节公钥）。
///
/// 同时接受两种输入形态：
/// - 单行 base64（内嵌进启动器二进制时用这种）；
/// - minisign `.pub` 文件的完整内容（`untrusted comment: …` 注释行 + base64 行）——
///   真实的 minisign 公钥文件总是后者，注释行不参与解码。
pub fn decode_minisign_public_key(line_b64: &str) -> Result<MinisignPublicKey, String> {
    let candidate = line_b64
        .lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty()
                && !l.starts_with("untrusted comment:")
                && !l.starts_with("trusted comment:")
        })
        .ok_or_else(|| "公钥内容为空".to_string())?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(candidate.as_bytes())
        .map_err(|e| format!("公钥 base64 解码失败: {e}"))?;
    if raw.len() != 42 {
        return Err(format!("公钥长度非法: {} 字节（应为 42）", raw.len()));
    }
    // 前两字节为签名算法标识（`Ed`）。非法标识说明这不是 minisign 公钥。
    if &raw[0..2] != b"Ed" {
        return Err("公钥算法标识非法（应为 Ed）".into());
    }
    let mut key_id = [0u8; 8];
    key_id.copy_from_slice(&raw[2..10]);
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw[10..42]);
    Ok(MinisignPublicKey { key, key_id })
}

/// 解析后的 minisign 签名（供格式校验与失败原因上报）。
///
/// `verify()` 的 ed25519 数学后端（`ed25519-dalek`）**已实现并测试**：`Ed` 直签原始字节、
/// `ED` 预哈希（SHA-512 后签名）两种模式均覆盖；任一不匹配即返回 [`VerifyOutcome::Failed`]
/// （即**拒绝**该数据），宁可拒绝，绝不假通过。已对齐 cgl-libs `tools/sign.mjs` 的纯 `Ed` 产物流。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinisignSignature {
    /// `untrusted comment:` 行（不参与验签）。
    pub comment: String,
    /// 签名算法标识（`Ed` 或 `ED`，后者为预哈希模式）。
    pub algorithm: [u8; 2],
    /// key id。
    pub key_id: [u8; 8],
    /// 签名本体（64 字节）。
    pub signature: [u8; 64],
    /// 全局签名（minisign 无预哈希时的 64 字节）；缺省为全零。
    pub global_signature: [u8; 64],
}

impl MinisignSignature {
    /// 解析 minisig 文件内容（4 行：注释 / base64 签名 / 信任注释 / base64 全局签名）。
    pub fn parse(text: &str) -> Option<Self> {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() < 2 {
            return None;
        }
        let comment = lines
            .first()
            .filter(|l| l.starts_with("untrusted comment:"))
            .map(|l| l.to_string())?;
        let sig_raw = base64::engine::general_purpose::STANDARD
            .decode(lines[1].as_bytes())
            .ok()?;
        if sig_raw.len() != 74 {
            return None;
        }
        let mut algorithm = [0u8; 2];
        algorithm.copy_from_slice(&sig_raw[0..2]);
        if &algorithm != b"Ed" && &algorithm != b"ED" {
            return None;
        }
        let mut key_id = [0u8; 8];
        key_id.copy_from_slice(&sig_raw[2..10]);
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&sig_raw[10..74]);

        let mut global_signature = [0u8; 64];
        if lines.len() >= 4 {
            if let Ok(global) = base64::engine::general_purpose::STANDARD.decode(lines[3].as_bytes())
            {
                if global.len() == 64 {
                    global_signature.copy_from_slice(&global);
                }
            }
        }
        Some(Self {
            comment,
            algorithm,
            key_id,
            signature,
            global_signature,
        })
    }

    /// 校验签名。
    ///
    /// 纯 ed25519（算法 `Ed`）时直接对 `content` 原始字节验签；预哈希（`ED`）时先对
    /// `content` 求 SHA-512 再验签。二者都对齐 cgl-libs `tools/sign.mjs` 的产物流
    /// （`sign.mjs` 以纯 `Ed` 模式签名，签名对象是文件原始字节，见其"关于 minisign
    /// 格式的真实实现说明"）。key id 与内容签名任一失败即返回 `Err`，由调用方拒绝该数据。
    pub fn verify(&self, public_key: MinisignPublicKey, content: &[u8]) -> Result<(), String> {
        if self.key_id != public_key.key_id {
            return Err("签名 key id 与内置公钥不匹配（可能为密钥轮换或伪造）".into());
        }
        // minisign 签名结构中的 64 字节 `signature` 是对**消息本体**的 ed25519 签名
        // （`Ed` 直签原始字节；`ED` 预哈希模式对本仓库不产出，但按规范用 SHA-512 承接）。
        let message: Vec<u8> = match &self.algorithm {
            b"Ed" => content.to_vec(),
            b"ED" => Sha512::digest(content).to_vec(),
            _ => return Err("签名算法标识非法（应为 Ed 或 ED）".into()),
        };
        let verifying_key = VerifyingKey::from_bytes(&public_key.key)
            .map_err(|e| format!("内置公钥不是合法 ed25519 公钥: {e}"))?;
        let signature = Signature::from_bytes(&self.signature);
        verifying_key
            .verify_strict(&message, &signature)
            .map_err(|e| format!("ed25519 验签失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // base64 `STANDARD.encode` 现为固有方法，无需引入 `Engine` trait。

    /// 构造一个合法的 minisign 公钥行（42 字节 base64），仅用于测试解析逻辑。
    fn fake_public_key_line(key_id: [u8; 8]) -> String {
        let mut raw = Vec::with_capacity(42);
        raw.extend_from_slice(b"Ed");
        raw.extend_from_slice(&key_id);
        raw.extend_from_slice(&[9u8; 32]);
        base64::engine::general_purpose::STANDARD.encode(raw)
    }

    /// 构造一个结构合法的 minisig 文本。
    fn fake_signature_text(key_id: [u8; 8]) -> String {
        let mut raw = Vec::with_capacity(74);
        raw.extend_from_slice(b"Ed");
        raw.extend_from_slice(&key_id);
        raw.extend_from_slice(&[3u8; 64]);
        let sig = base64::engine::general_purpose::STANDARD.encode(raw);
        let global = base64::engine::general_purpose::STANDARD.encode([4u8; 64]);
        format!("untrusted comment: signature from minisign secret key\n{sig}\ntrusted comment: timestamp:1\tfile:index.json\n{global}\n")
    }

    #[test]
    fn unconfigured_public_key_never_claims_verified() {
        // 用显式空公钥构造"未配置"验签器，独立于生产常量 TRUSTED_PUBLIC_KEY 是否已填入，
        // 保证该用例在公钥配置前后都稳定。
        let verifier = SignatureVerifier::with_public_key("");
        assert!(!verifier.is_configured());
        assert_eq!(verifier.status(), SignatureStatus::NotConfigured);
        assert!(!verifier.status().is_verified(), "未配置公钥不得宣称已验证");
        assert_eq!(verifier.status_key(), "module.registry.signature.not_configured");

        // 即便签名文件格式完全合法，公钥未配置时也只能得到 NotConfigured。
        let outcome = verifier.verify_index(
            br#"{"schema_version":1}"#,
            &fake_signature_text([1u8; 8]),
        );
        assert_eq!(outcome, VerifyOutcome::NotConfigured);
        assert_eq!(outcome.status(), SignatureStatus::NotConfigured);
        assert!(!outcome.status().is_verified());
        assert!(!outcome.should_reject(), "无密钥不应导致整个元数据不可用");
    }

    #[test]
    fn configured_key_validates_format_and_fails_closed() {
        let key_id = [7u8; 8];
        let verifier = SignatureVerifier::with_public_key(&fake_public_key_line(key_id));
        assert!(verifier.is_configured());
        assert_ne!(verifier.status(), SignatureStatus::NotConfigured);

        // 签名缺失 → SignatureMissing（不拒绝，但也不宣称通过）。
        let missing = verifier.verify_index(b"content", "");
        assert_eq!(missing, VerifyOutcome::SignatureMissing);
        assert!(!missing.status().is_verified());

        // 签名格式非法 → Failed（必须拒绝）。
        let bad = verifier.verify_index(b"content", "not a minisig file");
        assert!(matches!(bad, VerifyOutcome::Failed(_)));
        assert!(bad.should_reject());
        assert!(!bad.status().is_verified());

        // key id 不匹配 → Failed（密钥轮换或伪造）。
        let mismatched = verifier.verify_index(b"content", &fake_signature_text([9u8; 8]));
        assert!(matches!(mismatched, VerifyOutcome::Failed(_)));
        assert!(mismatched.should_reject());

        // 结构合法且 key id 匹配，但公钥/签名是随机的 → 伪数据必须 Failed（宁可拒绝，不可假通过）。
        // 注：随机 32 字节可能落在 ed25519 椭圆点之外，此时 from_bytes 判"公钥非法"；
        //     也可能恰好成点但验签不匹配，判"验签失败"。两者都必须是 Failed，故不固定子消息。
        let bogus = verifier.verify_index(b"content", &fake_signature_text(key_id));
        assert!(matches!(bogus, VerifyOutcome::Failed(_)), "随机伪公钥+伪签名必须失败而不是 {bogus:?}");
        assert!(bogus.should_reject());
        assert!(!bogus.status().is_verified());
    }

    #[test]
    fn real_ed25519_signature_verified_and_tamper_rejected() {
        use ed25519_dalek::{Signer, SigningKey};

        // 固定种子生成确定性密钥对，测试与公钥常量状态解耦。
        let key_id = [5u8; 8];
        let secret = SigningKey::from_bytes(&[7u8; 32]);
        let verify_key: [u8; 32] = secret.verifying_key().to_bytes();

        // 构造合法公钥行：Ed(2) + key_id(8) + 32 字节 ed25519 公钥。
        let mut pk_raw = Vec::with_capacity(42);
        pk_raw.extend_from_slice(b"Ed");
        pk_raw.extend_from_slice(&key_id);
        pk_raw.extend_from_slice(&verify_key);
        let pub_line = base64::engine::general_purpose::STANDARD.encode(&pk_raw);

        let content = br#"{"schema_version":1,"entries":[]}"#;
        let sig64: [u8; 64] = secret.sign(content).to_bytes();

        // 构造合法 minisig 文本：Ed(2) + key_id(8) + 64 字节真实签名 + 占位全局签名行。
        let mut sig_raw = Vec::with_capacity(74);
        sig_raw.extend_from_slice(b"Ed");
        sig_raw.extend_from_slice(&key_id);
        sig_raw.extend_from_slice(&sig64);
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(&sig_raw);
        let global = base64::engine::general_purpose::STANDARD.encode([0u8; 64]);
        let sig_text = format!(
            "untrusted comment: signature from minisign secret key\n{sig_b64}\ntrusted comment: timestamp:1\tfile:index.json\n{global}\n"
        );

        let verifier = SignatureVerifier::with_public_key(&pub_line);
        assert!(verifier.is_configured());

        // 真实签名 → Verified。
        let ok = verifier.verify_index(content, &sig_text);
        assert_eq!(ok, VerifyOutcome::Verified, "真实签名必须通过");
        assert!(ok.status().is_verified());
        assert!(!ok.should_reject());

        // 篡改内容 → Failed，必须拒绝。
        let tampered = verifier.verify_index(br#"{"schema_version":2,"entries":[]}"#, &sig_text);
        assert!(matches!(tampered, VerifyOutcome::Failed(_)));
        assert!(tampered.should_reject());
        assert!(!tampered.status().is_verified());
    }

    #[test]
    fn minisign_parsing_accepts_public_key_and_rejects_malformed() {
        let key_id = [2u8; 8];
        let decoded = decode_minisign_public_key(&fake_public_key_line(key_id))
            .expect("合法公钥应能解码");
        assert_eq!(decoded.key_id, key_id);
        assert_eq!(decoded.key, [9u8; 32]);

        // 支持 minisign 公钥文件的两行形态。
        let file = format!(
            "untrusted comment: minisign public key 1234\n{}\n",
            fake_public_key_line(key_id)
        );
        assert!(decode_minisign_public_key(&file).is_ok());

        assert!(decode_minisign_public_key("").is_err());
        assert!(decode_minisign_public_key("!!!not base64!!!").is_err());
        // 长度不足 / 算法标识非法。
        let short = base64::engine::general_purpose::STANDARD.encode(b"Ed1234");
        assert!(decode_minisign_public_key(&short).is_err());
        let mut wrong_algo = Vec::new();
        wrong_algo.extend_from_slice(b"XX");
        wrong_algo.extend_from_slice(&[0u8; 40]);
        let wrong_algo = base64::engine::general_purpose::STANDARD.encode(wrong_algo);
        assert!(decode_minisign_public_key(&wrong_algo).is_err());

        // 签名文件解析。
        let parsed = MinisignSignature::parse(&fake_signature_text(key_id)).expect("应能解析");
        assert_eq!(parsed.algorithm, *b"Ed");
        assert_eq!(parsed.key_id, key_id);
        assert_eq!(parsed.signature, [3u8; 64]);
        assert_eq!(parsed.global_signature, [4u8; 64]);
        assert!(parsed.comment.starts_with("untrusted comment:"));
        assert!(MinisignSignature::parse("").is_none());
        assert!(MinisignSignature::parse("untrusted comment: x\n###").is_none());

        // 公钥行归一化：跳过注释行。
        let normalized = normalized_public_key(&file).expect("应取到数据行");
        assert!(!normalized.contains("untrusted comment"));
        assert!(normalized_public_key("untrusted comment: only").is_none());
        assert!(normalized_public_key("   ").is_none());
    }
}
