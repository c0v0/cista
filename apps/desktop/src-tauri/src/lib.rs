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
    Entry::new(SECRET_SERVICE, &key)
        .map_err(|error| error.to_string())?
        .set_password(&value)
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
fn secret_load(key: String) -> Result<Option<String>, String> {
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
    let entry = Entry::new(SECRET_SERVICE, &key).map_err(|error| error.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(target_os = "android")]
#[tauri::command]
fn secret_save(_key: String, _value: String) -> Result<(), String> {
    Err("Android Keystore 适配尚未实现：本次会话不会持久化设备密钥".into())
}

#[cfg(target_os = "android")]
#[tauri::command]
fn secret_load(_key: String) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(target_os = "android")]
#[tauri::command]
fn secret_delete(_key: String) -> Result<(), String> {
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            platform_info,
            secret_save,
            secret_load,
            secret_delete
        ])
        .run(tauri::generate_context!())
        .expect("cista-desktop: 运行 Tauri 应用失败");
}
