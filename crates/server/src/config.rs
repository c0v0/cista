//! 服务端配置。全部来自环境变量（不提供远程配置 API，`design.md` §4.4）。

use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    /// 一次性 bootstrap token：首台设备入网用；为空表示已关闭。
    pub bootstrap_token: Option<String>,
    pub max_file_bytes: u64,
    /// 实例配额（设计：0.80 × 卷容量，部署时用 df 算出后写进环境变量）。
    pub instance_quota_bytes: u64,
    /// 单工作区配额（设计：0.25 × instance）。
    pub workspace_quota_bytes: u64,
    /// 单链接配额（设计：0.10 × instance）。
    pub link_quota_bytes: u64,
    /// 每 key 每分钟补充的令牌数（设备/链接）。
    pub rate_per_minute: u32,
    /// 桶容量：允许的突发请求数。
    pub rate_burst: u32,
    /// 跨域部署时的前端 origin（`design.md` §4.2）；为空表示只接受同源。
    pub cors_origin: Option<String>,
    pub log_json: bool,
}

/// 配额三元组，随请求传递。
#[derive(Debug, Clone, Copy)]
pub struct Quotas {
    pub instance_bytes: u64,
    pub workspace_bytes: u64,
    pub link_bytes: u64,
}

impl Config {
    pub fn quotas(&self) -> Quotas {
        Quotas {
            instance_bytes: self.instance_quota_bytes,
            workspace_bytes: self.workspace_quota_bytes,
            link_bytes: self.link_quota_bytes,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("环境变量 {0} 非法")]
    Invalid(&'static str),
}

const MIB: u64 = 1024 * 1024;

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind: SocketAddr = std::env::var("CISTA_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8787".into())
            .parse()
            .map_err(|_| ConfigError::Invalid("CISTA_BIND"))?;

        let data_dir: PathBuf = std::env::var("CISTA_DATA_DIR")
            .unwrap_or_else(|_| "./data".into())
            .into();

        let bootstrap_token = std::env::var("CISTA_BOOTSTRAP_TOKEN")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let max_file_bytes = parse_u64("CISTA_MAX_FILE_BYTES").unwrap_or(100 * MIB);
        let instance_quota_bytes =
            parse_u64("CISTA_INSTANCE_QUOTA_BYTES").unwrap_or(8 * 1024 * MIB);
        let workspace_quota_bytes =
            parse_u64("CISTA_WORKSPACE_QUOTA_BYTES").unwrap_or(instance_quota_bytes / 4);
        let link_quota_bytes =
            parse_u64("CISTA_LINK_QUOTA_BYTES").unwrap_or(instance_quota_bytes / 10);
        let rate_per_minute = parse_u64("CISTA_RATE_LIMIT_PER_MIN")
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(240);
        let rate_burst = parse_u64("CISTA_RATE_LIMIT_BURST")
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(rate_per_minute / 2)
            .max(1);

        let cors_origin = std::env::var("CISTA_CORS_ORIGIN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let log_json = std::env::var("CISTA_LOG_FORMAT")
            .map(|v| v != "pretty")
            .unwrap_or(true);

        Ok(Self {
            bind,
            data_dir,
            bootstrap_token,
            max_file_bytes,
            instance_quota_bytes,
            workspace_quota_bytes,
            link_quota_bytes,
            rate_per_minute,
            rate_burst,
            cors_origin,
            log_json,
        })
    }

    /// 测试用：小配额，便于覆盖边界。
    pub fn for_tests(data_dir: PathBuf) -> Self {
        Self {
            bind: "127.0.0.1:0".parse().expect("bind"),
            data_dir,
            bootstrap_token: None,
            max_file_bytes: MIB,
            instance_quota_bytes: 4 * MIB,
            workspace_quota_bytes: 2 * MIB,
            link_quota_bytes: MIB,
            rate_per_minute: 240,
            rate_burst: 120,
            cors_origin: None,
            log_json: false,
        }
    }
}

fn parse_u64(name: &'static str) -> Option<u64> {
    std::env::var(name).ok().and_then(|raw| raw.parse().ok())
}
