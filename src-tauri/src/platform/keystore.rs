//! Android Keystore 凭证存储（`SecretStore` 的移动端实现）。
//!
//! 职责划分遵循「Rust 管事实、Kotlin 管 Android 机制」：
//! - **Kotlin**（`com.copperlamp.coppergolem.secret.SecretStoreBridge`）持有
//!   `AndroidKeyStore` 里的 AES-256 密钥，负责 AES/GCM 加解密与密文落盘；
//! - **Rust**（本模块）只做三件事：拼存储键、经 JNI 调桥、把错误码翻译成用户
//!   能读懂的中文说明。
//!
//! ## 初始化：为什么不用 `ndk-context`
//!
//! `ndk_context::android_context()` 在未初始化时**直接 panic**
//! （`expect("android context was not initialized")`），而本项目的依赖链里
//! **没有任何一方调用 `initialize_android_context`**：`tao` 自持一份
//! `ndk_glue::main_android_context()`，不写这个全局量。用它等于在真机上留一颗
//! 「第一次读写凭证就崩溃」的雷（本模块早期版本正是这样，已纠正）。
//!
//! 改为**由桥类在类加载时主动交接**：
//! `SecretStoreBridge` 的 `init` 块调用 native `nativeRegisterBridge(Class)`，
//! 本模块据此拿到 `JavaVM` 与桥类的全局引用。
//!
//! - 拿 `JavaVM`：JNI 规范保证 `JNI_OnLoad` 先于任何 native 方法调用执行，所以
//!   这里直接读 [`JNI_OnLoad`] 存下的进程级 VM。
//! - 拿桥类：由 Java 侧传入（类加载事件本身携带），**不需要**用
//!   `FindClass` / ClassLoader 反射去猜——从原生线程 attach 后 `FindClass` 走的是
//!   系统 ClassLoader，解析不到应用自己的类（Android JNI 的已知行为）。
//!
//! 交接没发生时 [`KeystoreStore::new`] 返回 `Err`，`platform::mod` 退回
//! `UnsupportedStore`（显式失败，不伪造存储）。
//!
//! ## 失败语义
//!
//! **任何情况下都不降级为明文。** Keystore 不可用、密钥被系统作废、密文损坏
//! 分别映射到不同文案，且都要求用户重新登录，而不是静默接受一个坏的凭证状态。

use std::sync::OnceLock;

use jni::objects::{GlobalRef, JClass, JObject, JObjectArray, JString, JThrowable, JValue};
use jni::sys::{jint, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM};

use super::secret::SecretStore;

/// 桥类的二进制名（JNI 用的 `/` 分隔形式）。
const BRIDGE_CLASS: &str = "com/copperlamp/coppergolem/secret/SecretStoreBridge";

/// 桥方法签名——与 `SecretStoreBridge.kt` 是同一份跨语言契约，改一处必须改两处。
const SIG_SET: &str = "(Ljava/lang/String;Ljava/lang/String;)[Ljava/lang/String;";
const SIG_KEY: &str = "(Ljava/lang/String;)[Ljava/lang/String;";
const SIG_IS_READY: &str = "()Z";

/// 错误码（与 `SecretStoreBridge.kt` 的常量逐字对应）。
const CODE_OK: &str = "";
const CODE_NOT_FOUND: &str = "not_found";
const CODE_KEY_INVALIDATED: &str = "key_invalidated";
const CODE_CORRUPTED: &str = "corrupted";

/// 键分隔符：`SecretStore` 的寻址是二元组，而桥只接受单个字符串键。
///
/// 用不可打印码位拼接，保证 `(a, b+c)` 与 `(a+b, c)` 不会撞到同一个存储槽
/// （撞了会表现为「A 账户的令牌覆盖了 B 账户的」）。
const KEY_SEPARATOR: char = '\u{1f}';

/// 进程级 JavaVM，由 [`JNI_OnLoad`] 写入。
static VM: OnceLock<JavaVM> = OnceLock::new();

/// 桥类的进程级引用，由 [`Java_com_copperlamp_coppergolem_secret_SecretStoreBridge_nativeRegisterBridge`] 写入。
static BRIDGE: OnceLock<GlobalRef> = OnceLock::new();

/// 库加载钩子。
///
/// JNI 规范保证它在任何 native 方法被调用之前执行一次，因此是取得 `JavaVM` 的
/// 唯一可靠位置（`Env::get_java_vm` 也能拿到，但那只在调用点有效）。
///
/// # Safety
///
/// 由 JVM 调用，`vm` 为进程内有效且长期存活的 `JavaVM*`。
#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: *mut jni::sys::JavaVM, _reserved: *mut std::ffi::c_void) -> jint {
    if vm.is_null() {
        log::error!("[keystore] JNI_OnLoad 收到空 JavaVM，凭证存储将不可用");
        return JNI_VERSION_1_6;
    }
    // SAFETY: 见上。
    match unsafe { JavaVM::from_raw(vm) } {
        Ok(java_vm) => {
            if VM.set(java_vm).is_err() {
                log::warn!("[keystore] JavaVM 已注册，忽略重复的 JNI_OnLoad");
            }
        }
        Err(error) => log::error!("[keystore] JavaVM 指针无效: {error}"),
    }
    JNI_VERSION_1_6
}

/// 桥类加载时由 Kotlin 调用，交接桥类的全局引用。
///
/// 方法名按 JNI 约定推导自 `SecretStoreBridge` 的包名与 `@JvmStatic external`
/// 声明，改名会同时破坏两侧。
///
/// # Safety
///
/// 由 JVM 以 `(Ljava/lang/Class;)V` 签名调用。
#[no_mangle]
pub extern "system" fn Java_com_copperlamp_coppergolem_secret_SecretStoreBridge_nativeRegisterBridge(
    mut env: JNIEnv,
    _class: JClass,
    bridge: JClass,
) {
    // 只接受本模块认识的那个类：宿主若从别处传入别的类，后续方法签名会不匹配，
    // 在这里拒绝比在业务调用点随机失败更容易定位。
    let actual = describe_class_name(&mut env, &bridge);
    if actual.as_deref() != Some(BRIDGE_CLASS) {
        log::error!(
            "[keystore] 桥类交接被拒绝：期望 {BRIDGE_CLASS}，收到 {}",
            actual.unwrap_or_else(|| "<无法读取类名>".to_string())
        );
        return;
    }

    match env.new_global_ref(&bridge) {
        Ok(reference) => {
            if BRIDGE.set(reference).is_err() {
                log::warn!("[keystore] 桥类已注册，忽略重复交接");
            } else {
                log::info!("[keystore] Android Keystore 桥已注册");
            }
        }
        Err(error) => log::error!("[keystore] 缓存桥类引用失败: {error}"),
    }
}

/// 读取一个 `jclass` 的二进制名（如 `com/x/y/Z`）。
fn describe_class_name(env: &mut JNIEnv, class: &JClass) -> Option<String> {
    let name = env
        .call_method(class, "getName", "()Ljava/lang/String;", &[])
        .ok()?
        .l()
        .ok()?;
    let name_obj = JString::from(name);
    let dotted = String::from(env.get_string(&name_obj).ok()?);
    Some(dotted.replace('.', "/"))
}

/// Android Keystore 存储后端。
pub struct KeystoreStore {
    vm: JavaVM,
    bridge: GlobalRef,
}

impl KeystoreStore {
    /// 取已交接的 JavaVM 与桥类，并探活。
    ///
    /// 失败即返回原因：此处的失败意味着**该设备上账户凭证无法安全持久化**，
    /// 必须让调用方看到，而不是退化成一个看起来很正常的假存储。
    pub fn new() -> Result<Self, String> {
        // `JavaVM` 不实现 `Clone`（它就是个 `*mut sys::JavaVM` 包装），因此由
        // 静态引用重建一个自有值：同一个 JVM，指针相同，语义上是同一件事。
        let vm = borrowed_java_vm()?;
        let bridge = BRIDGE
            .get()
            .ok_or_else(|| {
                "Android Keystore 未就绪：SecretStoreBridge 未完成注册\
                 （请确认 APK 内已打包该 Kotlin 类）"
                    .to_string()
            })?
            .clone();

        // `env` 单独作用域：`AttachGuard` 借用着 `vm`，必须在把 `vm` 移进返回值
        // 之前析构掉（线程附着状态由 JVM 自己维持，guard 只负责记账）。
        let ready = {
            // `vm` 是 `&JavaVM`，`attach_current_thread` 要 `&self`；显式解引用
            // 而不是依赖自动 reborrow —— `JavaVM` 不实现 `Deref`。
            let mut env = vm
                .attach_current_thread()
                .map_err(|e| format!("无法附着到 Java 虚拟机: {e}"))?;
            let value = env
                .call_static_method(&bridge, "isReady", SIG_IS_READY, &[])
                .map_err(|e| describe_jni("查询凭证存储就绪状态", &mut env, e))?;
            value
                .z()
                .map_err(|e| format!("凭证存储就绪探针返回了非布尔值: {e}"))?
        };

        if !ready {
            return Err(
                "凭证存储未初始化：Android Keystore 桥拿不到应用上下文\
                 （SecretStoreBridge.initialize 未被调用）"
                    .to_string(),
            );
        }

        Ok(Self { vm, bridge })
    }

    /// 调桥的 `get` / `delete`（单 `String` 入参，返回三元组）。
    fn call_key_method(
        &self,
        method: &str,
        service: &str,
        account: &str,
        stage: &str,
    ) -> Result<Vec<String>, String> {
        let key = storage_key(service, account);
        let mut env = self
            .vm
            .attach_current_thread()
            .map_err(|e| format!("无法附着到 Java 虚拟机: {e}"))?;
        let key_arg = env
            .new_string(&key)
            .map_err(|e| format!("构造凭证键失败: {e}"))?;
        // `JValue` 借的是 `JObject`，必须具名绑定：临时值会在语句结束即析构，
        // 而借用要活到 `call_static_method` 之后。
        let key_obj = JObject::from(key_arg);
        let args = [JValue::Object(&key_obj)];
        let value = env
            .call_static_method(&self.bridge, method, SIG_KEY, &args)
            .map_err(|e| describe_jni(stage, &mut env, e))?;
        let array = value
            .l()
            .map_err(|e| format!("{stage}：桥返回了非对象结果: {e}"))?;
        read_string_array(&mut env, JObjectArray::from(array))
    }

    /// 把桥返回的三元组 `[值, 错误码, 说明]` 按业务语义解释。
    ///
    /// `absent_ok`：`set` / `delete` 不返回内容，空值不算异常；`get` 的空值由桥
    /// 明确标成 `not_found`，不会走到这里。
    fn interpret(fields: Vec<String>, absent_ok: bool, stage: &str) -> Result<Option<String>, String> {
        let mut it = fields.into_iter();
        let value = it.next().unwrap_or_default();
        let code = it.next().unwrap_or_default();
        let message = it.next().unwrap_or_default();
        let hint = if message.is_empty() {
            String::new()
        } else {
            format!("（{message}）")
        };

        match code.as_str() {
            CODE_OK => {
                if value.is_empty() {
                    if absent_ok {
                        Ok(None)
                    } else {
                        Err(format!("{stage}：系统安全存储返回了空凭证"))
                    }
                } else {
                    Ok(Some(value))
                }
            }
            CODE_NOT_FOUND => Ok(None),
            CODE_KEY_INVALIDATED => Err(format!(
                "系统安全设置变更导致凭证密钥失效，本地凭证已不可用：{stage}，请重新登录{hint}"
            )),
            CODE_CORRUPTED => Err(format!("本地凭证已损坏、无法解密：{stage}，请重新登录{hint}")),
            // 含 unavailable 与任何未来新增的错误码：一律按「不可用」上报，
            // 绝不把未知状态当成成功。
            other => Err(format!(
                "系统安全存储不可用（{other}）：{stage}，请检查设备安全设置后重试{hint}"
            )),
        }
    }
}

impl SecretStore for KeystoreStore {
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String> {
        let key = storage_key(service, account);
        let mut env = self
            .vm
            .attach_current_thread()
            .map_err(|e| format!("无法附着到 Java 虚拟机: {e}"))?;
        let key_arg = env
            .new_string(&key)
            .map_err(|e| format!("构造凭证键失败: {e}"))?;
        let value_arg = env
            .new_string(value)
            .map_err(|e| format!("构造凭证值失败: {e}"))?;
        let key_obj = JObject::from(key_arg);
        let value_obj = JObject::from(value_arg);
        let args = [JValue::Object(&key_obj), JValue::Object(&value_obj)];
        let result = env
            .call_static_method(&self.bridge, "set", SIG_SET, &args)
            .map_err(|e| describe_jni("写入凭证", &mut env, e))?;
        let array = result
            .l()
            .map_err(|e| format!("写入凭证：桥返回了非对象结果: {e}"))?;
        let fields = read_string_array(&mut env, JObjectArray::from(array))?;
        Self::interpret(fields, true, "写入凭证").map(|_| ())
    }

    fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
        let fields = self.call_key_method("get", service, account, "读取凭证")?;
        Self::interpret(fields, false, "读取凭证")
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        let fields = self.call_key_method("delete", service, account, "清除凭证")?;
        Self::interpret(fields, true, "清除凭证").map(|_| ())
    }
}

/// 把 (service, account) 拼成存储键。
fn storage_key(service: &str, account: &str) -> String {
    format!("{service}{KEY_SEPARATOR}{account}")
}

/// 读取桥返回的 `Array<String?>`。
///
/// 参数收 [`JObjectArray`] 而不是 `JObject`：`get_array_length` /
/// `get_object_array_element` 都要求数组类型的 trait 约束，裸 `JObject` 不满足
/// （虽然它在运行时确实指向一个数组）。转换点只有一个，即调用处的 `From`。
fn read_string_array(env: &mut JNIEnv, array: JObjectArray) -> Result<Vec<String>, String> {
    let len = env
        .get_array_length(&array)
        .map_err(|e| format!("读取凭证存储返回值长度失败: {e}"))?;
    let mut out = Vec::with_capacity(len as usize);
    for index in 0..len {
        let element = env
            .get_object_array_element(&array, index)
            .map_err(|e| format!("读取凭证存储返回值失败: {e}"))?;
        if element.is_null() {
            out.push(String::new());
            continue;
        }
        // `JString::from(element)` 的临时值必须具名绑定：`get_string` 借它，
        // 直接用 `&JString::from(..)` 会在语句结束即失效。
        let element = JString::from(element);
        let text = env
            .get_string(&element)
            .map_err(|e| format!("解码凭证存储返回值失败: {e}"))?;
        // `JavaStr` 只 Deref 到 `JNIStr`，没有 `Display`，必须显式转 `String`
        // （顺便把 JNI 的 modified UTF-8 收成 Rust 的 UTF-8）。
        out.push(String::from(text));
    }
    Ok(out)
}

/// 把 JNI 失败连同**挂起的 Java 异常**一起变成可读文案。
///
/// 只返回 `jni::errors::Error` 会丢掉真正的成因（异常类名与消息），而它恰恰是
/// 「为什么令牌存不下」的唯一线索。顺序：取异常描述 → 清异常 → 拼完整说明。
/// 挂起异常必须清掉，否则后续 JNI 调用会连续失败。
fn describe_jni(stage: &str, env: &mut JNIEnv, error: jni::errors::Error) -> String {
    let thrown = match env.exception_occurred() {
        Ok(throwable) if !throwable.is_null() => describe_throwable(env, &throwable),
        _ => None,
    };
    let _ = env.exception_clear();

    match thrown {
        Some(detail) => format!("{stage} 失败: {detail}（{error}）"),
        None => format!("{stage} 失败: {error}"),
    }
}

/// 读出挂起异常的类名与消息（只用于诊断，不改变控制流）。
fn describe_throwable(env: &mut JNIEnv, throwable: &JThrowable) -> Option<String> {
    let class = env.get_object_class(throwable).ok()?;
    let name = env
        .call_method(&class, "getName", "()Ljava/lang/String;", &[])
        .ok()?
        .l()
        .ok()?;
    let name_obj = JString::from(name);
    let name = String::from(env.get_string(&name_obj).ok()?);

    // 消息可能为 null，也可能内容为空：两种情况都退化成「只有异常类名」。
    // 这里刻意不用 `and_then` 链——闭包里借 `env` 再返回引用会与临时值的
    // 生命周期打架，展开写反而更清楚。
    let message = match env
        .call_method(throwable, "getMessage", "()Ljava/lang/String;", &[])
        .ok()
        .and_then(|value| value.l().ok())
        .filter(|message| !message.is_null())
    {
        Some(message_obj) => {
            let message_obj = JString::from(message_obj);
            env.get_string(&message_obj)
                .map(String::from)
                .unwrap_or_default()
        }
        None => String::new(),
    };

    if message.is_empty() {
        Some(name)
    } else {
        Some(format!("{name}: {message}"))
    }
}

/// 由进程级静态引用重建一个自有的 `JavaVM`。
///
/// `JavaVM` 只是 `*mut sys::JavaVM` 的包装且未实现 `Clone`；`OnceLock` 给出的是
/// `&'static JavaVM`，而调用方需要一个自有值来持有。从同一个指针重建是安全的：
/// JVM 在进程内唯一，其 `JavaVM*` 由 `JNI_OnLoad` 提供并长期有效。
fn borrowed_java_vm() -> Result<JavaVM, String> {
    let reference = VM
        .get()
        .ok_or_else(|| "Android Keystore 未就绪：原生库的 JNI_OnLoad 未执行".to_string())?;
    // SAFETY: 指针来自 `JNI_OnLoad`，在本进程内始终有效。
    unsafe { JavaVM::from_raw(reference.get_java_vm_pointer()) }
        .map_err(|e| format!("重建 JavaVM 句柄失败: {e}"))
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 键拼接必须无歧义，否则两个不同的 (service, account) 会撞同一个存储槽。
    #[test]
    fn storage_key_is_unambiguous() {
        assert_ne!(storage_key("svc", "a+b"), storage_key("svc+a", "b"));
        assert_ne!(storage_key("a", "b"), storage_key("ab", ""));
        assert_eq!(storage_key("svc", "acct"), storage_key("svc", "acct"));
    }

    /// 分隔符不能出现在正常命名里，否则常规键会互相串。
    #[test]
    fn storage_key_separator_is_not_printable() {
        assert!(!KEY_SEPARATOR.is_alphanumeric());
        assert!(!KEY_SEPARATOR.is_ascii());
        assert!(!"copper-golem:access".contains(KEY_SEPARATOR));
    }

    /// 错误码解读：成功、缺失、密钥失效、损坏、不可用各有稳定语义。
    #[test]
    fn interpret_maps_error_codes() {
        let ok = |code: &str, message: &str| vec![String::new(), code.into(), message.into()];

        assert_eq!(
            KeystoreStore::interpret(
                vec!["tok".into(), CODE_OK.into(), String::new()],
                false,
                "读取"
            )
            .unwrap()
            .as_deref(),
            Some("tok")
        );
        assert_eq!(
            KeystoreStore::interpret(ok(CODE_OK, ""), true, "写入").unwrap(),
            None,
            "set/delete 无返回值是正常成功"
        );
        assert!(
            KeystoreStore::interpret(ok(CODE_OK, ""), false, "读取").is_err(),
            "读取拿到空内容必须报错，不能当成空凭证"
        );
        assert_eq!(
            KeystoreStore::interpret(ok(CODE_NOT_FOUND, ""), false, "读取").unwrap(),
            None,
            "不存在返回 None —— 调用方据此走「请重新登录」而不是报错"
        );

        let invalidated = KeystoreStore::interpret(ok(CODE_KEY_INVALIDATED, "boom"), true, "写入")
            .expect_err("密钥失效必须报错");
        assert!(
            invalidated.contains("重新登录"),
            "文案要给出可执行的下一步: {invalidated}"
        );

        let corrupted = KeystoreStore::interpret(ok(CODE_CORRUPTED, ""), false, "读取")
            .expect_err("损坏必须报错");
        assert!(corrupted.contains("损坏"));

        // 未知错误码按不可用上报，绝不当成成功。
        let unknown = KeystoreStore::interpret(ok("some_future_code", "x"), true, "写入")
            .expect_err("未知码不可当成成功");
        assert!(unknown.contains("不可用"));
    }
}
