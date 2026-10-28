package com.cista.keystore

import android.app.Activity
import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** 用 AndroidKeyStore 里的 AES 密钥保护设备种子与 MK；密钥不可导出。 */
@TauriPlugin
class KeystorePlugin(private val activity: Activity) : Plugin(activity) {
  /** 只允许设备种子与 MK；与 Web/Rust 侧白名单保持一致。 */
  private fun isValidKey(key: String): Boolean {
    if (key == "master-key") return true
    if (!key.startsWith("device-seed:") || key.length != 44) return false
    return key.substring(12).all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }
  }
  private val prefs = activity.getSharedPreferences("cista-secrets", Context.MODE_PRIVATE)

  private fun secretKey(): SecretKey {
    val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    (store.getEntry(ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
    generator.init(
      KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setUserAuthenticationRequired(false)
        .build()
    )
    return generator.generateKey()
  }

  private fun encrypt(value: String): String {
    val cipher = Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, secretKey()) }
    val ct = cipher.doFinal(value.toByteArray(Charsets.UTF_8))
    return Base64.encodeToString(cipher.iv + ct, Base64.NO_WRAP)
  }

  private fun decrypt(blob: String): String? = try {
    val bytes = Base64.decode(blob, Base64.NO_WRAP)
    val iv = bytes.copyOfRange(0, GCM_IV)
    val ct = bytes.copyOfRange(GCM_IV, bytes.size)
    val cipher = Cipher.getInstance(TRANSFORMATION)
      .apply { init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(128, iv)) }
    String(cipher.doFinal(ct), Charsets.UTF_8)
  } catch (_: Exception) {
    null
  }

  @Command
  fun secret_save(invoke: Invoke) {
    val key = invoke.getArgs().getString("key") ?: return invoke.reject("missing key")
    if (!isValidKey(key)) return invoke.reject("unknown secret key")
    val value = invoke.getArgs().getString("value") ?: return invoke.reject("missing value")
    prefs.edit().putString(key, encrypt(value)).apply()
    invoke.resolve()
  }

  @Command
  fun secret_load(invoke: Invoke) {
    val key = invoke.getArgs().getString("key") ?: return invoke.reject("missing key")
    if (!isValidKey(key)) return invoke.reject("unknown secret key")
    val result = prefs.getString(key, null)?.let { decrypt(it) }
    val out = app.tauri.plugin.JSObject()
    out.put("value", result)
    invoke.resolve(out)
  }

  @Command
  fun secret_delete(invoke: Invoke) {
    val key = invoke.getArgs().getString("key") ?: return invoke.reject("missing key")
    if (!isValidKey(key)) return invoke.reject("unknown secret key")
    prefs.edit().remove(key).apply()
    invoke.resolve()
  }

  companion object {
    private const val ALIAS = "cista-master"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"
    private const val GCM_IV = 12
  }
}
