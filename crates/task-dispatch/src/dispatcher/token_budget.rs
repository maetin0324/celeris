//! ADR 2026-10-20-mlsys-main2-fixes D1-3/D1-4: task 木の token 予算（`Budget.max_tokens`）。
//! run の途中の累積 usage（`UsageProgress`）で run を止める watch と、continuation・retry・reviewer を
//! 起こす前の「task 木の消費 ≥ 上限」の判定。数え方は `Usage::budget_tokens`（input+output+cache）。
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// 予算を持つ task（自分か、`max_tokens` を持つ最も近い祖先）と、その上限。無ければ `None`。
pub(super) fn budget_scope(
    store: &dyn TaskStore,
    task: &Task,
) -> Result<Option<(TaskId, u64)>, StoreError> {
    let mut cur = task.clone();
    // 親の鎖は深さの上限（max_depth）で有限だが、壊れた DB でも回り続けないように数で切る。
    for _ in 0..64 {
        if let Some(max) = cur.budget.max_tokens {
            return Ok(Some((cur.id, max)));
        }
        let Some(parent) = cur.parent_id else {
            return Ok(None);
        };
        match store.get(parent)? {
            Some(p) => cur = p,
            None => return Ok(None),
        }
    }
    Ok(None)
}

/// `root` とその子孫の全 run（reviewer・planner を含む）の `runs[].usage` の和。`exclude_run` の行は数えない
/// （途中の run は呼び出し側が自分の途中値を足す）。終わった run は確定値、走っている他の run は途中値。
pub(super) fn tree_tokens_used(
    store: &dyn TaskStore,
    root: TaskId,
    exclude_run: Option<&str>,
) -> Result<u64, StoreError> {
    let mut total = 0u64;
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for row in store.runs_for_task(id)? {
            if Some(row.run_id.as_str()) == exclude_run {
                continue;
            }
            total += row.usage.map(|u| u.budget_tokens()).unwrap_or(0);
        }
        stack.extend(store.children(id)?.into_iter().map(|c| c.id));
    }
    Ok(total)
}

/// 予算に達していれば `(used, max)`。予算の無い task は常に `None`。
pub(super) fn exhausted(
    store: &dyn TaskStore,
    task: &Task,
) -> Result<Option<(u64, u64)>, StoreError> {
    let Some((root, max)) = budget_scope(store, task)? else {
        return Ok(None);
    };
    let used = tree_tokens_used(store, root, None)?;
    Ok((used >= max).then_some((used, max)))
}

/// `WorkerFinished.outcome` / 遷移の理由に書く文（`token_budget_exhausted: <used>/<max>`）。
pub(super) fn outcome(used: u64, max: u64) -> String {
    format!("token_budget_exhausted: {used}/{max}")
}

/// run の途中の監視。`prior`（この run 以外の消費）＋この run の累積値 ≥ `max` で `wake` を鳴らす。
#[derive(Debug)]
pub(crate) struct TokenBudgetWatch {
    max: u64,
    prior: u64,
    last: StdMutex<Option<task_core::Usage>>,
    exceeded: AtomicBool,
    pub(crate) wake: tokio::sync::Notify,
}

impl TokenBudgetWatch {
    pub(crate) fn new(prior: u64, max: u64) -> Self {
        Self {
            max,
            prior,
            last: StdMutex::new(None),
            exceeded: AtomicBool::new(prior >= max),
            wake: tokio::sync::Notify::new(),
        }
    }

    /// run 内の累積 usage を受ける（`UsageProgress`）。上限に達したら `wake` を 1 回鳴らす。
    pub(crate) fn observe(&self, usage: &task_core::Usage) {
        if let Ok(mut last) = self.last.lock() {
            *last = Some(*usage);
        }
        if self.prior + usage.budget_tokens() >= self.max
            && !self.exceeded.swap(true, Ordering::SeqCst)
        {
            self.wake.notify_one();
        }
    }

    pub(crate) fn exceeded(&self) -> bool {
        self.exceeded.load(Ordering::SeqCst)
    }

    /// この run の最後の累積 usage（止めた run の `WorkerFinished.usage` に入れる）。
    pub(crate) fn last_usage(&self) -> Option<task_core::Usage> {
        self.last.lock().ok().and_then(|l| *l)
    }

    /// 止めた run の終端（`BudgetExhausted{Tokens}`、usage は最後の累積値）。
    pub(crate) fn terminal(&self) -> Terminal {
        let run = self.last_usage().map(|u| u.budget_tokens()).unwrap_or(0);
        Terminal::BudgetExhausted {
            kind: task_core::BudgetKind::Tokens,
            message: format!(
                "task token budget reached: {}/{} (ADR 2026-10-20-mlsys-main2-fixes D1)",
                self.prior + run,
                self.max
            ),
            usage: self.last_usage(),
        }
    }
}
