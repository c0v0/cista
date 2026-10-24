//! 进程内令牌桶限速（单实例；`design.md` §2.3「每 IP/设备/路由限速」）。
//!
//! 载荷：每个 key 一个桶，按 `per_minute` 匀速补充，容量 `burst`。
//! 超限返回建议的 `Retry-After`（秒），由 `AppError::RateLimited` 转成 429。

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy)]
pub struct RateConfig {
    /// 每分钟补充的令牌数。
    pub per_minute: u32,
    /// 桶容量（允许的突发）。
    pub burst: u32,
}

impl Default for RateConfig {
    fn default() -> Self {
        Self {
            per_minute: 240,
            burst: 120,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    updated_at: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RateLimited {
    pub retry_after_secs: u64,
}

#[derive(Debug)]
pub struct RateLimiter {
    config: RateConfig,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    pub fn new(config: RateConfig) -> Self {
        Self {
            config: RateConfig {
                per_minute: config.per_minute.max(1),
                burst: config.burst.max(1),
            },
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// 消耗一个令牌；超限返回需要等待的秒数。
    pub fn check(&self, key: &str, now: i64) -> Result<(), RateLimited> {
        let per_second = f64::from(self.config.per_minute) / 60.0;
        let burst = f64::from(self.config.burst);
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // key 数量有界（设备/链接），但仍做一次粗粒度清理，避免长期累积。
        if buckets.len() > 4096 {
            buckets.retain(|_, bucket| now.saturating_sub(bucket.updated_at) < 300);
        }

        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: burst,
            updated_at: now,
        });
        #[allow(clippy::cast_precision_loss)]
        let elapsed = (now - bucket.updated_at).max(0) as f64;
        bucket.tokens = (bucket.tokens + elapsed * per_second).min(burst);
        bucket.updated_at = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Ok(());
        }

        let missing = 1.0 - bucket.tokens;
        let wait = (missing / per_second).ceil().max(1.0);
        Err(RateLimited {
            retry_after_secs: wait as u64,
        })
    }
}

/// 认证失败计数与指数退避（`phase6-hardening.md` §1 最后一行）。
///
/// 同一 key（设备 id，缺省为 `dev:anonymous`）在 `window` 秒内连续失败 `threshold` 次后加锁：
/// 第 n 次越界锁 `min(2^(n-1), 60)` 秒。成功验签会清空计数。
#[derive(Debug)]
pub struct FailureTracker {
    threshold: u32,
    window: i64,
    inner: Mutex<HashMap<String, FailureState>>,
}

#[derive(Debug, Clone, Copy)]
struct FailureState {
    count: u32,
    last_failure: i64,
    locked_until: i64,
}

impl FailureTracker {
    pub fn new(threshold: u32, window_secs: i64) -> Self {
        Self {
            threshold: threshold.max(1),
            window: window_secs.max(1),
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// 是否被锁；返回剩余秒数（至少 1）。
    pub fn locked_for(&self, key: &str, now: i64) -> Option<u64> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = inner.get(key)?;
        if state.locked_until > now {
            Some(u64::try_from(state.locked_until - now).unwrap_or(1))
        } else {
            None
        }
    }

    pub fn record(&self, key: &str, now: i64) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.len() > 4096 {
            inner.retain(|_, state| now.saturating_sub(state.last_failure) < 3600);
        }
        let state = inner.entry(key.to_string()).or_insert(FailureState {
            count: 0,
            last_failure: now,
            locked_until: 0,
        });
        if now.saturating_sub(state.last_failure) > self.window {
            state.count = 0;
        }
        state.count = state.count.saturating_add(1);
        state.last_failure = now;
        if state.count >= self.threshold {
            let over = state.count - self.threshold;
            let backoff = 1i64.checked_shl(over.min(6)).unwrap_or(60).clamp(1, 60);
            state.locked_until = now.saturating_add(backoff);
        }
    }

    pub fn clear(&self, key: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_is_enforced_per_key() {
        let limiter = RateLimiter::new(RateConfig {
            per_minute: 60,
            burst: 2,
        });
        assert!(limiter.check("dev:a", 1000).is_ok());
        assert!(limiter.check("dev:a", 1000).is_ok());
        let limited = limiter.check("dev:a", 1000).expect_err("third must fail");
        assert!(limited.retry_after_secs >= 1);

        // 另一个 key 不受影响。
        assert!(limiter.check("dev:b", 1000).is_ok());
    }

    #[test]
    fn tokens_refill_over_time() {
        let limiter = RateLimiter::new(RateConfig {
            per_minute: 60,
            burst: 1,
        });
        assert!(limiter.check("k", 0).is_ok());
        assert!(limiter.check("k", 0).is_err());
        // 60/分钟 → 1 秒补 1 个。
        assert!(limiter.check("k", 1).is_ok());
        assert!(limiter.check("k", 1).is_err());
    }

    #[test]
    fn retry_after_is_positive_and_bounded() {
        let limiter = RateLimiter::new(RateConfig {
            per_minute: 1,
            burst: 1,
        });
        assert!(limiter.check("k", 0).is_ok());
        let limited = limiter.check("k", 0).expect_err("must fail");
        // 每分钟 1 个 → 需要等 60 秒。
        assert_eq!(limited.retry_after_secs, 60);
    }

    #[test]
    fn time_going_backwards_does_not_grant_tokens() {
        let limiter = RateLimiter::new(RateConfig {
            per_minute: 60,
            burst: 1,
        });
        assert!(limiter.check("k", 100).is_ok());
        // 时间倒退（NTP 调整）：elapsed 归零，不能凭空补令牌。
        assert!(limiter.check("k", 50).is_err());
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let tracker = FailureTracker::new(2, 300);
        assert_eq!(tracker.locked_for("dev:a", 1000), None);
        tracker.record("dev:a", 1000);
        assert_eq!(tracker.locked_for("dev:a", 1000), None, "未到阈值");
        tracker.record("dev:a", 1000);
        assert_eq!(tracker.locked_for("dev:a", 1000), Some(1));
        tracker.record("dev:a", 1000);
        assert_eq!(tracker.locked_for("dev:a", 1000), Some(2));
        for _ in 0..10 {
            tracker.record("dev:a", 1000);
        }
        assert_eq!(tracker.locked_for("dev:a", 1000), Some(60), "上限 60s");
    }

    #[test]
    fn success_and_window_reset() {
        let tracker = FailureTracker::new(2, 60);
        tracker.record("k", 0);
        tracker.record("k", 0);
        assert!(tracker.locked_for("k", 0).is_some());
        tracker.clear("k");
        assert_eq!(tracker.locked_for("k", 0), None);

        tracker.record("k", 0);
        // 超出窗口后重新计数：一次失败不应触发锁定。
        tracker.record("k", 1000);
        assert_eq!(tracker.locked_for("k", 1000), None);
    }

    #[test]
    fn keys_are_isolated() {
        let tracker = FailureTracker::new(1, 60);
        tracker.record("dev:a", 5);
        assert!(tracker.locked_for("dev:a", 5).is_some());
        assert_eq!(tracker.locked_for("dev:b", 5), None);
    }
}
