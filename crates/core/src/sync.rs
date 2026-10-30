//! 同步状态机（纯逻辑，无 IO）。
//!
//! 服务端权威 `rev`；客户端保存 `base_rev`（上次与服务端一致的 rev）与是否有本地改动。
//! 冲突不丢数据：由调用方生成冲突副本，见 `docs/adr/0003-notes-concurrency.md`。

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncState {
    /// 本地已知的服务端 rev（上次同步完成时的 rev）。
    pub base_rev: u64,
    /// 本地是否有未上行的改动。
    pub local_dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 已一致，无需动作。
    UpToDate,
    /// 远端没变，本地有改动 → 推送。
    Push,
    /// 远端前进且本地干净 → 快进。
    Pull,
    /// 双方都有改动（或服务端回退）→ 拉取并生成冲突副本。
    Conflict,
}

/// 根据远端当前 rev 决定下一步。
pub fn decide(state: SyncState, remote_rev: u64) -> Decision {
    match remote_rev.cmp(&state.base_rev) {
        Ordering::Equal => {
            if state.local_dirty {
                Decision::Push
            } else {
                Decision::UpToDate
            }
        }
        Ordering::Greater => {
            if state.local_dirty {
                Decision::Conflict
            } else {
                Decision::Pull
            }
        }
        // 服务端 rev 不应回退；真出现（恢复旧备份）时按冲突处理，绝不静默覆盖。
        Ordering::Less => Decision::Conflict,
    }
}

/// 冲突副本标题：一定带来源与时间，避免与原文同名。
pub fn conflict_copy_title(title: &str, device: &str, ts: i64) -> String {
    format!("{title}（冲突副本 · {device} · {ts}）")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_and_equal_is_up_to_date() {
        assert_eq!(
            decide(
                SyncState {
                    base_rev: 5,
                    local_dirty: false
                },
                5
            ),
            Decision::UpToDate
        );
    }

    #[test]
    fn dirty_and_equal_pushes() {
        assert_eq!(
            decide(
                SyncState {
                    base_rev: 5,
                    local_dirty: true
                },
                5
            ),
            Decision::Push
        );
    }

    #[test]
    fn remote_ahead_and_clean_pulls() {
        assert_eq!(
            decide(
                SyncState {
                    base_rev: 5,
                    local_dirty: false
                },
                6
            ),
            Decision::Pull
        );
    }

    #[test]
    fn both_changed_conflicts() {
        assert_eq!(
            decide(
                SyncState {
                    base_rev: 5,
                    local_dirty: true
                },
                6
            ),
            Decision::Conflict
        );
    }

    #[test]
    fn rewound_server_is_conflict() {
        assert_eq!(
            decide(
                SyncState {
                    base_rev: 5,
                    local_dirty: false
                },
                4
            ),
            Decision::Conflict
        );
    }

    #[test]
    fn conflict_copy_title_is_distinct() {
        let a = conflict_copy_title("周报", "desktop", 1_790_000_000);
        let b = conflict_copy_title("周报", "laptop", 1_790_000_000);
        assert_ne!(a, b);
        assert!(a.starts_with("周报（冲突副本"));
    }
}
