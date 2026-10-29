//! 进程状态与时钟。
//!
//! 私有库与共享库分离（ADR 0001）：分享路由只能拿到 `shared`。
//! 每个工作区的 yrs 文档与 WS 广播都在这里集中持有。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use cista_core::auth::ReplayCache;
use tokio::sync::{broadcast, Semaphore};

use crate::config::{Config, Quotas};
use crate::db::{PrivateStore, SharedStore};
use crate::rate_limit::{FailureTracker, RateConfig, RateLimiter};
use crate::yjs::Documents;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub private: PrivateStore,
    pub shared: SharedStore,
    pub replay: Arc<Mutex<ReplayCache>>,
    pub events: Arc<EventBus>,
    pub ws_events: Arc<FrameBus>,
    pub documents: Arc<Documents>,
    pub rate: Arc<RateLimiter>,
    pub drop_rate: Arc<RateLimiter>,
    pub failures: Arc<FailureTracker>,
    /// 限制同时被缓冲的已签名请求体数量（每份 ≤ 6 MiB）。
    pub body_budget: Arc<Semaphore>,
    /// 限制并发 Argon2 校验，避免同步哈希阻塞 runtime。
    pub hash_budget: Arc<Semaphore>,
    /// 限制同时进行的私有附件流式上传（磁盘与连接占用）。
    pub upload_budget: Arc<Semaphore>,
}

impl AppState {
    pub fn new(cfg: Config, private: PrivateStore, shared: SharedStore) -> Self {
        let rate = RateLimiter::new(RateConfig {
            per_minute: cfg.rate_per_minute,
            burst: cfg.rate_burst,
        });
        // 投递链接（只上传、不列目录）用更严的额度。
        let drop_rate = RateLimiter::new(RateConfig {
            per_minute: (cfg.rate_per_minute / 4).max(1),
            burst: (cfg.rate_burst / 4).max(1),
        });
        Self {
            cfg: Arc::new(cfg),
            private,
            shared,
            replay: Arc::new(Mutex::new(ReplayCache::default())),
            events: Arc::new(EventBus::default()),
            ws_events: Arc::new(FrameBus::default()),
            documents: Arc::new(Documents::new()),
            rate: Arc::new(rate),
            drop_rate: Arc::new(drop_rate),
            failures: Arc::new(FailureTracker::new(5, 300)),
            body_budget: Arc::new(Semaphore::new(16)),
            hash_budget: Arc::new(Semaphore::new(4)),
            upload_budget: Arc::new(Semaphore::new(8)),
        }
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.cfg.data_dir
    }

    pub fn quotas(&self) -> Quotas {
        self.cfg.quotas()
    }
}

/// 每个工作区一个广播通道；只广播 `rev`（SSE 的载荷，见 `design.md` §8）。
#[derive(Debug, Default)]
pub struct EventBus {
    channels: Mutex<HashMap<String, broadcast::Sender<i64>>>,
}

impl EventBus {
    pub fn subscribe(&self, workspace_id: &str) -> broadcast::Receiver<i64> {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        channels
            .entry(workspace_id.to_string())
            .or_insert_with(|| broadcast::channel(64).0)
            .subscribe()
    }

    pub fn publish(&self, workspace_id: &str, rev: i64) {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sender = channels
            .entry(workspace_id.to_string())
            .or_insert_with(|| broadcast::channel(64).0);
        // 没有订阅者时 send 会失败，忽略即可。
        let _ = sender.send(rev);
    }

    /// 清理没有订阅者的通道，避免长期运行时 map 只增不减。
    pub fn retain_active(&self) {
        self.channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, sender| sender.receiver_count() > 0);
    }
}

/// Yjs 帧广播（`/ws`）：每个工作区一个通道，载荷是完整的 y-sync 编码帧。
///
/// 载荷用 `Arc<[u8]>` 共享，避免为每个订阅者复制；通道容量压到 32，
/// 慢客户端会收到 `Lagged` 并跳过帧，而不是无限占用内存。
#[derive(Debug, Default)]
pub struct FrameBus {
    channels: Mutex<HashMap<String, broadcast::Sender<Arc<[u8]>>>>,
}

impl FrameBus {
    pub fn subscribe(&self, workspace_id: &str) -> broadcast::Receiver<Arc<[u8]>> {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        channels
            .entry(workspace_id.to_string())
            .or_insert_with(|| broadcast::channel(32).0)
            .subscribe()
    }

    pub fn publish(&self, workspace_id: &str, frame: Arc<[u8]>) {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sender = channels
            .entry(workspace_id.to_string())
            .or_insert_with(|| broadcast::channel(32).0);
        let _ = sender.send(frame);
    }

    /// 清理没有订阅者的通道。
    pub fn retain_active(&self) {
        self.channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, sender| sender.receiver_count() > 0);
    }
}

/// Unix 秒。签名时间窗与所有时间戳都以它为准。
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// 数据目录所在卷的可用字节数；拿不到时为 `None`（Windows 或异常）。
///
/// `statvfs` 的字段宽度随平台不同（Linux 为 u64，部分平台为 u32），
/// 因此这里的 `u64::from` 在 Linux 上会被 clippy 视为同类型转换。
#[allow(clippy::useless_conversion)]
#[cfg(unix)]
pub fn free_bytes(path: &std::path::Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` 只写入由我们提供的、对齐正确的结构体。
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let block = u64::from(stats.f_frsize)
        .checked_mul(u64::from(stats.f_bavail))
        .unwrap_or(0);
    Some(block)
}

#[cfg(not(unix))]
pub fn free_bytes(_path: &std::path::Path) -> Option<u64> {
    None
}
