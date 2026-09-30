//! Android host bridge. The Java host owns Activity and native-loader lifecycle;
//! Rust only sends a validated instance request through the Tauri event channel.

use serde_json::json;
use crate::error::KernelError;
use crate::registry::events::EventBus;

pub const PREPARE_EVENT: &str = "android-game-prepare";
pub const EXIT_EVENT: &str = "android-game-exited";

pub fn request_prepare(events: &EventBus, instance: &str, package_name: &str) -> Result<(), KernelError> {
    if instance.is_empty() || package_name.is_empty() {
        return Err(KernelError::InvalidArgument("Android 游戏启动参数为空".into()));
    }
    events.publish(PREPARE_EVENT, json!({
        "instance_name": instance,
        "package_name": package_name,
    }));
    Ok(())
}

pub fn publish_exited(events: &EventBus, instance: &str, reason: &str) {
    events.publish(EXIT_EVENT, json!({
        "instance_name": instance,
        "reason": reason,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_empty_prepare_request() {
        let events = EventBus::new();
        assert!(request_prepare(&events, "", "com.mojang.minecraftpe").is_err());
        assert!(request_prepare(&events, "v", "").is_err());
    }
}
