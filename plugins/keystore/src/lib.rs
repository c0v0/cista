//! cista 密钥存储插件：Android 走 AndroidKeyStore，其它平台由宿主 crate 的 keyring 实现。
use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("cista-keystore").build()
}
