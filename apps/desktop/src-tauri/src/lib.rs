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

// Android：敏感键走 `plugins/keystore`（AndroidKeyStore）。这里的明文 JSON 只作为
// 非敏感键的兜底；敏感键在插件不可用时直接报错，且读取时会清理旧版遗留条目。
#[cfg(target_os = "android")]
fn secrets_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager as _;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join("cista-secrets.json"))
}

#[cfg(target_os = "android")]
fn read_secrets(
    app: &tauri::AppHandle,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let path = secrets_path(app)?;
    if !path.exists() {
        return Ok(std::collections::BTreeMap::new());
    }
    let raw = std::fs::read(&path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&raw).map_err(|error| error.to_string())
}

#[cfg(target_os = "android")]
fn write_secrets(
    app: &tauri::AppHandle,
    secrets: &std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    use std::io::Write as _;
    let path = secrets_path(app)?;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp).map_err(|error| error.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = file.metadata().map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o600);
        file.set_permissions(perms).map_err(|e| e.to_string())?;
    }
    file.write_all(&serde_json::to_vec(secrets).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|error| error.to_string())
}

#[cfg(target_os = "android")]
#[tauri::command]
fn secret_save(app: tauri::AppHandle, key: String, value: String) -> Result<(), String> {
    if !is_known_secret_key(&key) {
        return Err("unknown secret key".to_string());
    }
    // 敏感键必须由 AndroidKeyStore 插件处理；插件失败时绝不落明文。
    if is_sensitive_key(&key) {
        return Err("sensitive keys require AndroidKeyStore".to_string());
    }
    let mut secrets = read_secrets(&app)?;
    secrets.insert(key, value);
    write_secrets(&app, &secrets)
}

#[cfg(target_os = "android")]
#[tauri::command]
fn secret_load(app: tauri::AppHandle, key: String) -> Result<Option<String>, String> {
    if !is_known_secret_key(&key) {
        return Err("unknown secret key".to_string());
    }
    if is_sensitive_key(&key) {
        // 旧版本可能留下明文条目：读之前先清掉，且永远不返回明文。
        purge_sensitive(&app)?;
        return Ok(None);
    }
    Ok(read_secrets(&app)?.get(&key).cloned())
}

#[cfg(target_os = "android")]
#[tauri::command]
fn secret_delete(app: tauri::AppHandle, key: String) -> Result<(), String> {
    if !is_known_secret_key(&key) {
        return Err("unknown secret key".to_string());
    }
    let mut secrets = read_secrets(&app)?;
    let mut changed = secrets.remove(&key).is_some();
    if is_sensitive_key(&key) {
        let before = secrets.len();
        secrets.retain(|candidate, _| !is_sensitive_key(candidate));
        changed |= secrets.len() != before;
    }
    if changed {
        write_secrets(&app, &secrets)?;
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn is_sensitive_key(key: &str) -> bool {
    key == "master-key" || key.starts_with("device-seed:")
}

/// 删除历史版本可能写入的敏感条目（明文 JSON 文件）。
#[cfg(target_os = "android")]
fn purge_sensitive(app: &tauri::AppHandle) -> Result<(), String> {
    let mut secrets = read_secrets(app)?;
    let before = secrets.len();
    secrets.retain(|key, _| !is_sensitive_key(key));
    if secrets.len() != before {
        write_secrets(app, &secrets)?;
    }
    Ok(())
}

#[cfg(target_os = "android")]
#[tauri::command]
fn purge_legacy_secrets(app: tauri::AppHandle) -> Result<(), String> {
    purge_sensitive(&app)
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
