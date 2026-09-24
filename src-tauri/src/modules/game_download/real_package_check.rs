//! 真实包端到端验证：确认原生解析链路完整走到授权边界。
//!
//! 这是一个由开发者在本地手动运行的验证入口，不依赖 test profile：
//! `cargo test -p copper-core --lib real_package -- --ignored --nocapture`
//! 真实包不存在时直接跳过，因此不会影响常规 CI。

use std::path::Path;

/// 真实 Xbox 游戏包路径（仅本机存在，缺失时跳过）。
const REAL_PACKAGE: &str = r"D:\minecraft\versions\.download\1.26.45.01.msixvc";

#[test]
#[ignore = "需要本机存在真实 MSIXVC 包"]
fn real_package_reaches_authorization_boundary() {
    let path = Path::new(REAL_PACKAGE);
    if !path.exists() {
        eprintln!("真实包不存在，跳过：{REAL_PACKAGE}");
        return;
    }

    let (header, metadata) =
        super::msixvc::parse_file_metadata(path).expect("真实包头解析必须成功");
    eprintln!(
        "头部: 格式版本={} 容器类型={} 用户区长度={} XVC 长度={}",
        header.format_version, header.container_type, header.user_data_length, header.xvc_data_length
    );
    eprintln!("段数量={}", metadata.segment_count);
    assert_eq!(metadata.header_len, 100, "XVD 段元数据头固定为 100 字节");
    assert!(metadata.segment_count > 0, "真实包必须包含段");

    let identifiers = super::msixvc::read_package_identifiers(path).expect("包标识必须可读");
    eprintln!(
        "包标识: content_id={} key_id={}",
        identifiers.content_id, identifiers.key_id
    );
    assert!(!identifiers.content_id.is_empty());
    assert!(!identifiers.key_id.is_empty());

    let paths = super::msixvc::validate_segment_plan(&metadata).expect("全部段路径必须安全");
    assert_eq!(paths.len(), metadata.segments.len());
    eprintln!("路径校验通过，共 {} 条", paths.len());

    let encrypted = super::msixvc::has_encrypted_regions(path).expect("区域表必须可解析");
    eprintln!("含加密区域={encrypted}");

    // 无密钥时提取必须停在授权边界，而不是报解析错误。
    let out = std::env::temp_dir().join(format!("copper-real-{}", std::process::id()));
    let dll_dir = std::env::temp_dir().join(format!("copper-real-dll-{}", std::process::id()));
    let outcome = super::extractor::extract_package_with_key(path, &out, &dll_dir, None);
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&dll_dir);
    match outcome {
        Ok(()) => eprintln!("无密钥却提取成功（包未加密或已回退到兼容后端）"),
        Err(error) => {
            let text = error.to_string();
            eprintln!("无密钥提取结果: {text}");
            assert!(
                !text.contains("SegmentMetadata") && !text.contains("不是受支持的 XVD"),
                "真实包不应再出现解析类错误，实际: {text}"
            );
        }
    }
}
