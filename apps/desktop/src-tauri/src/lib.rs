//! cista desktop 客户端（Tauri 2）。
//!
//! UI 复用 `web/` 的 React SPA；本 crate 只提供原生壳与本地能力
//! （系统钥匙串、附件 IO、SQLite 索引）。业务逻辑一律走 `cista-core`，不在此复制。

use keyring::Entry;

/// 钥匙串里的 service 名：与标识符一致，便于用户在产品级“钥匙串访问”里识别。
const SECRET_SERVICE: &str = "com.cista.notes";

/// 写入系统钥匙串（macOS Keychain / Windows Credential Manager / Linux Secret Service）。
///
/// Web 端只在内存里保存密钥；桌面端可显式选择“记住这台设备”，由这个命令落盘。
#[tauri::command]
fn secret_save(key: String, value: String) -> Result<(), String> {
    Entry::new(SECRET_SERVICE, &key)
        .map_err(|error| error.to_string())?
        .set_password(&value)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn secret_load(key: String) -> Result<Option<String>, String> {
    let entry = Entry::new(SECRET_SERVICE, &key).map_err(|error| error.to_string())?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn secret_delete(key: String) -> Result<(), String> {
    let entry = Entry::new(SECRET_SERVICE, &key).map_err(|error| error.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            secret_save,
            secret_load,
            secret_delete
        ])
        .run(tauri::generate_context!())
        .expect("cista-desktop: 运行 Tauri 应用失败");
}
