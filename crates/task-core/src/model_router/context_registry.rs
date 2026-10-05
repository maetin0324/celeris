//! run に結ぶ期限付きの `context_ref` の登録・解決（ADR 2026-10-04 §10 Phase 3）。
//!
//! ref は中身を運ばない opaque な文字列で、daemon がプロセス内の registry に登録した context を指す。
//! 時計は呼び出し側が `now` で渡す（この型は時計を読まない）。試験は偽の時刻を渡して決定的に動かす。
//! registry は in-memory で、再起動で消える。消えた後の ref は `Invalid` になる（fail-closed）。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use thiserror::Error;

use super::context::RoutingContext;

/// ref を解決できなかった理由。proxy はどちらも要求を拒否し、context の代わりに既定を使わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ContextRefError {
    /// 登録されていない・release 済み・prune 済みの ref。
    #[error("routing context ref is invalid")]
    Invalid,
    /// 登録はあるが ttl を過ぎた ref。
    #[error("routing context ref has expired")]
    Expired,
}

/// run に結んだ context の登録簿。`&self` で共有でき、実装は内部で同期する。
pub trait RoutingContextRegistry: Send + Sync {
    /// `ctx` を `run_id` に結んで登録し、opaque な ref を返す。期限は `now + ttl`。
    fn register(&self, run_id: &str, ctx: RoutingContext, ttl: Duration, now: Instant) -> String;
    /// ref を解決する。期限（`now >= expires_at`）を過ぎた ref は `Expired`。
    fn resolve(&self, reference: &str, now: Instant) -> Result<RoutingContext, ContextRefError>;
    /// run の in-flight が終わったときに、その run の ref をすべて外す。
    fn release(&self, run_id: &str);
    /// 期限を過ぎた ref を取り除き、取り除いた件数を返す。
    fn prune(&self, now: Instant) -> usize;
}

struct Entry {
    run_id: String,
    ctx: RoutingContext,
    expires_at: Instant,
}

/// 時計を注入できる in-memory の registry。
#[derive(Default)]
pub struct InMemoryRoutingContextRegistry {
    entries: Mutex<HashMap<String, Entry>>,
}

impl InMemoryRoutingContextRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        // 登録の途中で panic しても表の内容は各 entry 単位で完結しているので、poison は無視して読む。
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl RoutingContextRegistry for InMemoryRoutingContextRegistry {
    fn register(&self, run_id: &str, ctx: RoutingContext, ttl: Duration, now: Instant) -> String {
        // 期限が表せない大きさなら即時に失効させる（長く残すより拒否する側に倒す）。
        let expires_at = now.checked_add(ttl).unwrap_or(now);
        let reference = format!("ctx_{}", ulid::Ulid::new());
        self.lock().insert(
            reference.clone(),
            Entry {
                run_id: run_id.to_owned(),
                ctx,
                expires_at,
            },
        );
        reference
    }

    fn resolve(&self, reference: &str, now: Instant) -> Result<RoutingContext, ContextRefError> {
        let entries = self.lock();
        let entry = entries.get(reference).ok_or(ContextRefError::Invalid)?;
        if now >= entry.expires_at {
            return Err(ContextRefError::Expired);
        }
        Ok(entry.ctx.clone())
    }

    fn release(&self, run_id: &str) {
        self.lock().retain(|_, entry| entry.run_id != run_id);
    }

    fn prune(&self, now: Instant) -> usize {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|_, entry| now < entry.expires_at);
        before - entries.len()
    }
}
