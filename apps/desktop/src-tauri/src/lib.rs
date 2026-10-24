//! cista desktop 客户端（Tauri 2）。
//!
//! UI 复用 `web/` 的 React SPA；本 crate 只提供原生壳与本地能力
//! （SQLite 索引、密钥库、附件 IO）。业务逻辑一律走 `cista-core`，不在此复制。

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("cista-desktop: 运行 Tauri 应用失败");
}
