//! Google Play Android catalog/delivery response models.
//!
//! This module deliberately contains no authentication or network transport. The
//! observed FDFE protocol is private and must not be called without a supported,
//! user-authorized credential source.

use serde::{Deserialize, Serialize};

/// A Minecraft listing parsed from the observed Play catalog response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayCatalogVersion {
    pub package_name: String,
    pub version_code: u64,
    pub version_name: String,
    pub download_size: u64,
    pub updated_on: i64,
    pub offer_type: i32,
}

/// A deliverable APK file. URLs are transient server-issued locations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayApkFile {
    pub name: String,
    pub url: String,
}

/// Parsed result of the observed Play delivery response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayDelivery {
    pub package_name: String,
    pub version_code: u64,
    pub files: Vec<PlayApkFile>,
}

/// A delivery failure status reported by the observed Play protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayDeliveryStatus {
    NotPurchased,
    Unverified(i32),
}

impl PlayDeliveryStatus {
    pub fn from_wire(value: i32) -> Self {
        match value {
            3 => Self::NotPurchased,
            other => Self::Unverified(other),
        }
    }
}

/// Sort catalog results newest version first, then by update timestamp.
pub fn sort_versions(versions: &mut [PlayCatalogVersion]) {
    versions.sort_by(|a, b| {
        b.version_code
            .cmp(&a.version_code)
            .then_with(|| b.updated_on.cmp(&a.updated_on))
            .then_with(|| a.version_name.cmp(&b.version_name))
    });
}

/// Parse a catalog row from the observed FDFE JSON projection.
///
/// The caller supplies the projection (rather than a protobuf decoder) so protocol
/// extraction remains isolated and can be replaced when Google changes the format.
pub fn parse_catalog_row(row: &serde_json::Value) -> Result<PlayCatalogVersion, String> {
    let package_name = row.pointer("/item/packageName").and_then(serde_json::Value::as_str)
        .ok_or("catalog package name missing")?;
    if package_name != crate::modules::game_download::apk::MCBE_PACKAGE {
        return Err("catalog package is not Minecraft Bedrock".into());
    }
    let version_code = row.pointer("/app/versionCode").and_then(serde_json::Value::as_u64)
        .ok_or("catalog version code missing")?;
    let version_name = row.pointer("/app/versionName").and_then(serde_json::Value::as_str)
        .filter(|v| !v.trim().is_empty()).ok_or("catalog version name missing")?;
    let download_size = row.pointer("/app/downloadSize").and_then(serde_json::Value::as_u64)
        .ok_or("catalog download size missing")?;
    let updated_on = row.pointer("/app/updatedOn").and_then(serde_json::Value::as_i64)
        .ok_or("catalog update timestamp missing")?;
    let offer_type = row.pointer("/offer/offerType").and_then(serde_json::Value::as_i64)
        .and_then(|v| i32::try_from(v).ok()).ok_or("catalog offer type missing")?;
    Ok(PlayCatalogVersion { package_name: package_name.into(), version_code, version_name: version_name.into(), download_size, updated_on, offer_type })
}

/// Validate a delivery response status before exposing files to the downloader.
pub fn validate_delivery_status(status: i32) -> Result<(), String> {
    match PlayDeliveryStatus::from_wire(status) {
        PlayDeliveryStatus::NotPurchased => Err("Google Play reports Minecraft is not purchased for this account".into()),
        PlayDeliveryStatus::Unverified(code) => Err(format!("Google Play delivery status is unverified ({code})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(version_code: u64, updated_on: i64) -> serde_json::Value {
        serde_json::json!({
            "item": { "packageName": "com.mojang.minecraftpe" },
            "app": { "versionCode": version_code, "versionName": format!("1.0.{version_code}"), "downloadSize": 100, "updatedOn": updated_on },
            "offer": { "offerType": 1 }
        })
    }

    #[test]
    fn parses_catalog_and_rejects_non_minecraft() {
        let parsed = parse_catalog_row(&row(710000001, 1700000000)).unwrap();
        assert_eq!(parsed.version_code, 710000001);
        assert_eq!(parsed.package_name, "com.mojang.minecraftpe");
        let mut wrong = row(1, 1);
        wrong["item"]["packageName"] = serde_json::json!("example.other");
        assert!(parse_catalog_row(&wrong).is_err());
    }

    #[test]
    fn rejects_incomplete_catalog_rows() {
        assert!(parse_catalog_row(&serde_json::json!({})).is_err());
    }

    #[test]
    fn sorts_by_version_then_timestamp() {
        let mut versions = vec![
            parse_catalog_row(&row(3, 20)).unwrap(),
            parse_catalog_row(&row(4, 10)).unwrap(),
            parse_catalog_row(&row(3, 30)).unwrap(),
        ];
        sort_versions(&mut versions);
        assert_eq!(versions.iter().map(|v| v.version_code).collect::<Vec<_>>(), [4, 3, 3]);
        assert_eq!(versions[1].updated_on, 30);
    }

    #[test]
    fn not_purchased_is_never_treated_as_downloadable() {
        assert!(validate_delivery_status(1).unwrap_err().contains("unverified"));
        assert!(validate_delivery_status(3).unwrap_err().contains("not purchased"));
        assert!(validate_delivery_status(99).is_err());
    }
}
