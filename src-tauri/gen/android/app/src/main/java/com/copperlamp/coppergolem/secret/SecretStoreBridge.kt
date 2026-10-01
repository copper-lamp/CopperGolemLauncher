package com.copperlamp.coppergolem.secret

import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import java.security.KeyStore
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * 凭证安全存储的安卓实现：Android Keystore 托管密钥 + AES/GCM 密文落盘。
 *
 * 设计边界（与 `platform::secret::SecretStore` 一一对应）：
 * - **密钥不落盘**：AES-256 密钥由 Android Keystore 生成并保管，进程只拿到
 *   `SecretKey` 句柄，私钥料永不进入应用可读内存或文件系统。
 * - **密文落盘**：值以 `IV(12B) || ciphertext || tag(16B)` 形式 base64 后写入
 *   应用私有 `SharedPreferences`；文件被复制走也解不开。
 * - **不提供明文降级**：Keystore 不可用、密钥失效、密文损坏一律返回明确错误，
 *   由上层提示用户重新登录，绝不退回明文存储。
 *
 * 为什么自己实现而不用 `EncryptedSharedPreferences`：那条路径依赖
 * `androidx.security:security-crypto`，其 1.0.0 稳定版 API 已废弃、1.1.0 长期停在
 * alpha，且它自身的失败语义（密钥失效后抛什么）不透明。这里的实现只用到
 * `Cipher` / `KeyStore` / `KeyGenerator` 这些平台 API，失败点可控、错误可分类。
 *
 * ## 与 Rust 的接口
 *
 * Rust 侧（`platform/keystore.rs`）需要两样东西才能调本类：
 * 1. **JavaVM**：Rust 在自己的 `JNI_OnLoad` 里取得，不经本类；
 * 2. **本类的 `jclass`**：由下面的 `init` 块在第 2 步主动交接。
 *
 * 之所以由 Java 侧交出类引用，是因为 Rust 若从原生线程 attach 后用
 * `FindClass` 找本类，走的是**系统** ClassLoader，解析不到应用自己的类
 * （Android JNI 的已知行为）。类加载事件本身携带权威引用，用它最省事也最稳。
 *
 * `nativeRegisterBridge` 的签名（含类名）是跨语言契约，改名必须同步 Rust。
 *
 * 返回值三元组 `[值, 错误码, 说明]` 同样是契约：错误码决定 Rust 给用户看什么
 * （例如 `key_invalidated` 必须提示「重新登录」而不是「重试」）。
 */
object SecretStoreBridge {
    private const val TAG = "CopperSecretStore"

    /** 应用私有偏好文件名；应用私有目录内，卸载即清除。 */
    private const val PREFS_NAME = "copper_secrets"

    /** Keystore 内的密钥别名。 */
    private const val KEY_ALIAS = "copper-golem-credentials"

    /** Keystore provider（AndroidKeyStore 由系统提供，不随应用分发）。 */
    private const val KEYSTORE_PROVIDER = "AndroidKeyStore"

    private const val TRANSFORMATION = "AES/GCM/NoPadding"

    /** GCM 认证标签长度（bit）与 IV 长度（byte）。 */
    private const val GCM_TAG_BITS = 128
    private const val GCM_IV_BYTES = 12

    /** 成功 */
    const val CODE_OK = ""

    /** 值不存在：不是错误，`get` 据此让调用方走「请重新登录」分支。 */
    const val CODE_NOT_FOUND = "not_found"

    /** Keystore 里的密钥被系统作废（改锁屏/重置安全设置/清除凭证）。 */
    const val CODE_KEY_INVALIDATED = "key_invalidated"

    /** 密文无法认证（被篡改、截断，或换过密钥）。 */
    const val CODE_CORRUPTED = "corrupted"

    /** 其它 Keystore / 加解密失败。 */
    const val CODE_UNAVAILABLE = "unavailable"

    /** Rust 侧注册入口（见类文档）。 */
    @JvmStatic
    private external fun nativeRegisterBridge(bridge: Class<*>)

    init {
        // 类加载即交接：保证 Rust 侧在第一次读写凭证之前已经拿到本类引用。
        // 加载原生库失败（例如桌面端跑测试）时给出明确日志而不是让异常穿透，
        // 后续 read/write 会以「桥未注册」的可读错误上报。
        try {
            nativeRegisterBridge(SecretStoreBridge::class.java)
        } catch (error: UnsatisfiedLinkError) {
            Log.e(TAG, "原生库未加载，凭证存储不可用: ${error.message}")
        }
    }

    @Volatile
    private var prefs: SharedPreferences? = null

    /**
     * 绑定应用上下文。由 `CopperCoreApplication.onCreate` 调用；必须在任何
     * 凭证读写之前完成，否则 [requirePrefs] 会以 `unavailable` 失败。
     */
    @JvmStatic
    fun initialize(context: Context) {
        if (prefs == null) {
            synchronized(this) {
                if (prefs == null) {
                    prefs = context.applicationContext
                        .getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                }
            }
        }
    }

    /** Rust 侧就绪探针：`false` 表示宿主漏调 [initialize]。 */
    @JvmStatic
    fun isReady(): Boolean = prefs != null

    /**
     * 写入（覆盖）一条凭证。
     *
     * 成功：`[null, "", ""]`；失败：`[null, 错误码, 说明]`。
     */
    @JvmStatic
    fun set(key: String, value: String): Array<String?> = try {
        val store = requirePrefs()
        val payload = encrypt(value)
        if (!store.edit().putString(key, payload).commit()) {
            failure(CODE_UNAVAILABLE, "写入凭证存储失败（磁盘或权限问题）")
        } else {
            success()
        }
    } catch (error: Throwable) {
        classify("写入凭证失败", error)
    }

    /**
     * 读取一条凭证。
     *
     * 成功且存在：`[明文, "", ""]`；成功但不存在：`[null, "not_found", ""]`；
     * 失败：`[null, 错误码, 说明]`。
     */
    @JvmStatic
    fun get(key: String): Array<String?> = try {
        val store = requirePrefs()
        val payload = store.getString(key, null)
        if (payload == null) {
            notFound()
        } else {
            arrayOf(decrypt(payload), CODE_OK, "")
        }
    } catch (error: Throwable) {
        classify("读取凭证失败", error)
    }

    /** 删除一条凭证（幂等：不存在也算成功）。 */
    @JvmStatic
    fun delete(key: String): Array<String?> = try {
        val store = requirePrefs()
        if (!store.edit().remove(key).commit()) {
            failure(CODE_UNAVAILABLE, "清除凭证存储失败（磁盘或权限问题）")
        } else {
            success()
        }
    } catch (error: Throwable) {
        classify("清除凭证失败", error)
    }

    // ------------------------------------------------------------- 加解密实现

    private fun encrypt(plain: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, loadOrCreateKey())
        val iv = cipher.iv
        check(iv != null && iv.size == GCM_IV_BYTES) {
            "Keystore 返回了非预期的 GCM IV 长度: ${iv?.size}"
        }
        val body = cipher.doFinal(plain.toByteArray(Charsets.UTF_8))
        return Base64.encodeToString(iv + body, Base64.NO_WRAP)
    }

    private fun decrypt(payload: String): String {
        val raw = try {
            Base64.decode(payload, Base64.NO_WRAP)
        } catch (error: IllegalArgumentException) {
            throw CorruptedSecretException("凭证密文不是合法 base64", error)
        }
        if (raw.size <= GCM_IV_BYTES) {
            throw CorruptedSecretException("凭证密文长度不足（${raw.size} 字节）", null)
        }
        val iv = raw.copyOfRange(0, GCM_IV_BYTES)
        val body = raw.copyOfRange(GCM_IV_BYTES, raw.size)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, loadOrCreateKey(), GCMParameterSpec(GCM_TAG_BITS, iv))
        return String(cipher.doFinal(body), Charsets.UTF_8)
    }

    /**
     * 取 Keystore 中的密钥，不存在则生成。
     *
     * `setUserAuthenticationRequired(false)`：启动器会在后台刷新令牌，不能要求
     * 用户每次解锁；令牌本身已受应用私有目录 + Keystore 双重保护。
     * `setRandomizedEncryptionRequired(true)`（默认）：强制每次加密使用随机 IV，
     * 杜绝 GCM 下 IV 重用的致命错误。
     */
    private fun loadOrCreateKey(): SecretKey {
        val keyStore = KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }
        (keyStore.getEntry(KEY_ALIAS, null) as? KeyStore.SecretKeyEntry)
            ?.let { return it.secretKey }

        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE_PROVIDER)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setUserAuthenticationRequired(false)
                .build()
        )
        return generator.generateKey()
    }

    // ----------------------------------------------------------------- 结果码

    /** 密文不可认证（自定义类型，避免与 Keystore 异常混淆）。 */
    private class CorruptedSecretException(message: String, cause: Throwable?) :
        Exception(message, cause)

    private fun requirePrefs(): SharedPreferences =
        prefs ?: throw IllegalStateException(
            "凭证存储未初始化（宿主未调用 SecretStoreBridge.initialize）"
        )

    private fun success(): Array<String?> = arrayOf(null, CODE_OK, "")

    private fun notFound(): Array<String?> = arrayOf(null, CODE_NOT_FOUND, "")

    private fun failure(code: String, message: String): Array<String?> = arrayOf(null, code, message)

    /**
     * 把异常归到稳定的错误码上。
     *
     * 分类是**契约的一部分**：Rust 侧据错误码决定给用户看什么（例如
     * `key_invalidated` 必须提示「重新登录」而不是「重试」）。
     */
    private fun classify(stage: String, error: Throwable): Array<String?> {
        Log.w(TAG, "$stage: ${error.javaClass.name}: ${error.message}", error)
        val message = error.message ?: error.javaClass.simpleName
        return when (error) {
            is CorruptedSecretException, is AEADBadTagException ->
                failure(CODE_CORRUPTED, "$stage：凭证已损坏，请重新登录（$message）")

            // KeyPermanentlyInvalidatedException / InvalidKeyException 继承自
            // KeyException；锁屏变更后密钥被系统作废即落在这里。
            is java.security.KeyException ->
                failure(CODE_KEY_INVALIDATED, "$stage：系统安全设置变更导致密钥失效，请重新登录（$message）")

            is java.security.GeneralSecurityException ->
                failure(CODE_UNAVAILABLE, "$stage：系统加密不可用（$message）")

            is IllegalStateException -> failure(CODE_UNAVAILABLE, "$stage：$message")

            else -> failure(CODE_UNAVAILABLE, "$stage：$message")
        }
    }
}
