//! 进程状态与时钟。
//!
//! 私有库与共享库分离（ADR 0001）：分享路由只能拿到 `shared`。
//! 每个工作区的 yrs 文档与 WS 广播都在这里集中持有。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use cista_core::auth::ReplayCache;
use tokio::sync::broadcast;

use crate::config::{Config, Quotas};
use crate::db::{PrivateStore, SharedStore};
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
}

impl AppState {
    pub fn new(cfg: Config, private: PrivateStore, shared: SharedStore) -> Self {
        Self {
            cfg: Arc::new(cfg),
            private,
            shared,
            replay: Arc::new(Mutex::new(ReplayCache::default())),
            events: Arc::new(EventBus::default()),
            ws_events: Arc::new(FrameBus::default()),
            documents: Arc::new(Documents::new()),
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
}

/// Yjs 帧广播（`/ws`）：每个工作区一个通道，载荷是完整的 y-sync 编码帧。
#[derive(Debug, Default)]
pub struct FrameBus {
    channels: Mutex<HashMap<String, broadcast::Sender<Vec<u8>>>>,
}

impl FrameBus {
    pub fn subscribe(&self, workspace_id: &str) -> broadcast::Receiver<Vec<u8>> {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        channels
            .entry(workspace_id.to_string())
            .or_insert_with(|| broadcast::channel(128).0)
            .subscribe()
    }

    pub fn publish(&self, workspace_id: &str, frame: Vec<u8>) {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sender = channels
            .entry(workspace_id.to_string())
            .or_insert_with(|| broadcast::channel(128).0);
        let _ = sender.send(frame);
    }
}

/// Unix 秒。签名时间窗与所有时间戳都以它为准。
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
