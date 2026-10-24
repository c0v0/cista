//! cista-core: 协议类型、加密、B+ 认证、同步状态机与客户端索引逻辑。
//!
//! 无 IO：不依赖 tokio / std::fs / 网络；传输由调用方注入。可编译到 wasm32。

#![forbid(unsafe_code)]

pub mod auth;
pub mod crypto;
pub mod protocol;
pub mod search;
pub mod share;
pub mod sync;

/// 协议版本：随密文格式与 API 兼容性一起变更（v1 冻结）。
pub const PROTOCOL_VERSION: u32 = 1;
