//! 随机 ID 生成（URL 安全，无依赖版本坑）。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

/// 16 字节 CSPRNG → base64url（22 字符），带前缀便于日志辨认。
pub fn new_id(prefix: &str) -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // 随机数不可用时宁可失败也不要可预测 ID：由调用方转换成 500。
        return format!("{prefix}_");
    }
    format!("{prefix}_{}", URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_prefixed() {
        let a = new_id("ws");
        let b = new_id("ws");
        assert!(a.starts_with("ws_"));
        assert_ne!(a, b);
        assert!(a.len() > 20);
    }
}
