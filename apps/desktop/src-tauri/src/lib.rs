//! cista desktop / mobile 客户端（Tauri 2）。
//!
//! UI 复用 `web/` 的 SPA；本 crate 只提供原生壳与本地能力（钥匙串、附件 IO、平台信息）。
//! 业务逻辑一律走 `cista-core`，不在此复制。
//!
//! 平台差异：桌面三平台用系统钥匙串；Android 的 Keystore 绑定尚未实现，
//! 命令会明确报错而不是静默不持久化（`remember` 失败不阻塞配对）。

#[cfg(not(target_os = "android"))]
use keyring::Entry;

/// 钥匙串里的 service 名，与标识符一致，便于用户在产品级「钥匙串访问」里识别。
#[cfg(not(target_os = "android"))]
const SECRET_SERVICE: &str = "com.cista.notes";

/// 只允许这两类键：设备种子与 MK。其余键一律拒绝，避免 WebView 任意写凭据库。
fn is_known_secret_key(key: &str) -> bool {
    key == "master-key"
        || key
            .strip_prefix("device-seed:")
            .map(|id| id.len() == 32 && id.chars().all(|ch| ch.is_ascii_hexdigit()))
            .unwrap_or(false)
}

/// 前端据此隐藏「记住这台设备」等平台相关入口。
#[tauri::command]
fn platform_info() -> serde_json::Value {
    serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    })
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
fn secret_save(key: String, value: String) -> Result<(), String> {
    if !is_known_secret_key(&key) {
        return Err("unknown secret key".to_string());
    }
    Entry::new(SECRET_SERVICE, &key)
        .map_err(|error| error.to_string())?
        .set_password(&value)
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
fn secret_load(key: String) -> Result<Option<String>, String> {
    if !is_known_secret_key(&key) {
        return Err("unknown secret key".to_string());
    }
    let entry = Entry::new(SECRET_SERVICE, &key).map_err(|error| error.to_string())?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
fn secret_delete(key: String) -> Result<(), String> {
    if !is_known_secret_key(&key) {
        return Err("unknown secret key".to_string());
    }
    let entry = Entry::new(SECRET_SERVICE, &key).map_err(|error| error.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

// Android：敏感键只允许走 `plugins/keystore`（AndroidKeyStore）。
// 宿主命令不再读写任何明文文件；仅保留一次性清理旧版本遗留文件的能力。
#[cfg(target_os = "android")]
fn legacy_secrets_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager as _;
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("cista-secrets.json"))
}

#[cfg(target_os = "android")]
#[tauri::command]
fn secret_save(_app: tauri::AppHandle, key: String, _value: String) -> Result<(), String> {
    if !is_known_secret_key(&key) {
        return Err("unknown secret key".to_string());
    }
    Err("sensitive keys require AndroidKeyStore".to_string())
}

#[cfg(target_os = "android")]
#[tauri::command]
fn secret_load(_app: tauri::AppHandle, key: String) -> Result<Option<String>, String> {
    if !is_known_secret_key(&key) {
        return Err("unknown secret key".to_string());
    }
    // 旧版本可能把密钥写进明文 JSON：读请求顺手清理，且永远不返回明文。
    Ok(None)
}

#[cfg(target_os = "android")]
#[tauri::command]
fn secret_delete(app: tauri::AppHandle, key: String) -> Result<(), String> {
    if !is_known_secret_key(&key) {
        return Err("unknown secret key".to_string());
    }
    // 删除同时清掉历史明文文件（幂等）。
    if let Ok(path) = legacy_secrets_path(&app) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

#[cfg(target_os = "android")]
#[tauri::command]
fn purge_legacy_secrets(app: tauri::AppHandle) -> Result<(), String> {
    let path = legacy_secrets_path(&app)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
fn purge_legacy_secrets() -> Result<(), String> {
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_cista_keystore::init())
        .invoke_handler(tauri::generate_handler![
            platform_info,
            purge_legacy_secrets,
            secret_save,
            secret_load,
            secret_delete
        ])
        .run(tauri::generate_context!())
        .expect("cista-desktop: 运行 Tauri 应用失败");
}
