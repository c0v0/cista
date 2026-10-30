//! cista 密钥存储插件：Android 走 AndroidKeyStore，其它平台由宿主 crate 的 keyring 实现。
use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("cista-keystore")
        .setup(|_app, api| {
            #[cfg(target_os = "android")]
            api.register_android_plugin("com.cista.keystore", "KeystorePlugin")
                .map(|_| ())?;
            #[cfg(not(target_os = "android"))]
            let _ = api;
            Ok(())
        })
        .build()
}
