//! ADR 2026-11-07-root-done-tick-phase-timing: `root_done_tick` の段別計測。
//!
//! 計測は毎 tick 固定の大きさの struct に足すだけで、確保はしない（D4）。`root_done_tick` 全体が閾値を
//! 超えた tick だけ、`cos.root_done.*` の項目を tick_cos_chat_launch の内訳に足す（D3）。slow tick の警告
//! （`tick_steps.rs` の "slow tick phases"）はその列を `steps_us` に出す。時間の項目は µs、行数・回数の項目は
//! 名前の末尾が `_rows`・`_calls`・`_max`。store の呼び出しは待ち（接続を取るまで）と処理（残り）に分ける（D2）。
//! 測りながら事実を集める `collect_facts`（root_done の D4）と、そこから呼ぶ `progress_markdown` もここに置く。

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use task_core::{DeliveryStore, SqliteStore, TaskId, TaskStore, WorkspaceSpec, store_lock_wait};
use task_ops::root_done::{self, ChildFact, RootDoneFacts, RootDoneInputs};

/// これを超えた `root_done_tick` だけ内訳を出す（D3）。`CosChatLaunch::root_done_probe_threshold` の既定値。
pub(crate) const ROOT_DONE_PROBE_THRESHOLD: Duration = Duration::from_millis(100);

pub(crate) fn us(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

/// store の呼び出しの回数・待ち・処理（µs）。
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StoreSpan {
    pub(crate) calls: u64,
    pub(crate) wait_us: u64,
    pub(crate) work_us: u64,
}

impl StoreSpan {
    /// `f` を 1 回の store の呼び出しとして測る。待ちはこの thread の `store_lock_wait` の増分、
    /// 処理は経過時間から待ちを引いた残り（SQLite の `busy_timeout` の待ちは処理に入る）。
    pub(crate) fn time<T>(&mut self, f: impl FnOnce() -> T) -> T {
        let wait_before = store_lock_wait();
        let started = Instant::now();
        let out = f();
        let elapsed = started.elapsed();
        let wait = store_lock_wait().saturating_sub(wait_before);
        self.calls += 1;
        self.wait_us = self.wait_us.saturating_add(us(wait));
        self.work_us = self
            .work_us
            .saturating_add(us(elapsed.saturating_sub(wait)));
        out
    }

    fn add(&mut self, other: &StoreSpan) {
        self.calls += other.calls;
        self.wait_us = self.wait_us.saturating_add(other.wait_us);
        self.work_us = self.work_us.saturating_add(other.work_us);
    }

    fn total_us(&self) -> u64 {
        self.wait_us.saturating_add(self.work_us)
    }
}

/// `root_done_facts` 1 回分（または合計）の内訳。
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct FactsProbe {
    pub(crate) get: StoreSpan,
    pub(crate) events_for: StoreSpan,
    /// `events_for` が返した件数の最大。
    pub(crate) events_for_max: u64,
    pub(crate) delivery_get: StoreSpan,
    pub(crate) children: StoreSpan,
    pub(crate) repo_get: StoreSpan,
    /// `progress_markdown` の時間（git の process を含む）と git の起動回数。
    pub(crate) progress_us: u64,
    pub(crate) progress_git_calls: u64,
}

impl FactsProbe {
    pub(crate) fn total_us(&self) -> u64 {
        [
            self.get.total_us(),
            self.events_for.total_us(),
            self.delivery_get.total_us(),
            self.children.total_us(),
            self.repo_get.total_us(),
            self.progress_us,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }

    fn add(&mut self, other: &FactsProbe) {
        self.get.add(&other.get);
        self.events_for.add(&other.events_for);
        self.events_for_max = self.events_for_max.max(other.events_for_max);
        self.delivery_get.add(&other.delivery_get);
        self.children.add(&other.children);
        self.repo_get.add(&other.repo_get);
        self.progress_us = self.progress_us.saturating_add(other.progress_us);
        self.progress_git_calls = self
            .progress_git_calls
            .saturating_add(other.progress_git_calls);
    }
}

/// 1 回の `root_done_tick` の内訳。
#[derive(Debug, Default)]
pub(crate) struct RootDoneProbe {
    pub(crate) ingest_us: u64,
    pub(crate) cursor_get: StoreSpan,
    pub(crate) events_since: StoreSpan,
    pub(crate) events_since_rows: u64,
    /// done 遷移の event を持つ task（頁の中で重複を除いた数）。
    pub(crate) candidate_rows: u64,
    pub(crate) candidate_get: StoreSpan,
    pub(crate) candidate_events_for: StoreSpan,
    pub(crate) candidate_events_for_max: u64,
    pub(crate) insert: StoreSpan,
    pub(crate) cursor_set: StoreSpan,
    pub(crate) advance_us: u64,
    pub(crate) open: StoreSpan,
    pub(crate) open_rows: u64,
    pub(crate) facts: FactsProbe,
    pub(crate) facts_row_max_us: u64,
}

impl RootDoneProbe {
    /// 1 行分の事実づくりを合計に足す。
    pub(crate) fn add_row(&mut self, row: &FactsProbe) {
        self.facts.add(row);
        self.facts_row_max_us = self.facts_row_max_us.max(row.total_us());
    }

    /// 閾値を超えた行の警告（D3）。task・state・段の µs・件数を 1 行に出す。
    pub(crate) fn warn_row(task: TaskId, state: &str, row: &FactsProbe) {
        tracing::warn!(
            task = %task,
            state,
            facts_us = row.total_us(),
            get_wait_us = row.get.wait_us,
            get_work_us = row.get.work_us,
            events_for_wait_us = row.events_for.wait_us,
            events_for_work_us = row.events_for.work_us,
            events_for_rows = row.events_for_max,
            delivery_get_wait_us = row.delivery_get.wait_us,
            delivery_get_work_us = row.delivery_get.work_us,
            children_wait_us = row.children.wait_us,
            children_work_us = row.children.work_us,
            progress_us = row.progress_us,
            progress_git_calls = row.progress_git_calls,
            "slow root-done row"
        );
    }

    /// `cos.root_done.*` の項目（D3）。
    pub(crate) fn parts(&self) -> Vec<(&'static str, u64)> {
        let f = &self.facts;
        let advance_other = self
            .advance_us
            .saturating_sub(self.open.total_us())
            .saturating_sub(f.total_us());
        vec![
            ("cos.root_done.ingest_us", self.ingest_us),
            ("cos.root_done.cursor_get.wait_us", self.cursor_get.wait_us),
            ("cos.root_done.cursor_get.work_us", self.cursor_get.work_us),
            ("cos.root_done.events_since_calls", self.events_since.calls),
            ("cos.root_done.events_since_rows", self.events_since_rows),
            (
                "cos.root_done.events_since.wait_us",
                self.events_since.wait_us,
            ),
            (
                "cos.root_done.events_since.work_us",
                self.events_since.work_us,
            ),
            ("cos.root_done.candidate_rows", self.candidate_rows),
            (
                "cos.root_done.candidate_get.wait_us",
                self.candidate_get.wait_us,
            ),
            (
                "cos.root_done.candidate_get.work_us",
                self.candidate_get.work_us,
            ),
            (
                "cos.root_done.candidate_events_for.wait_us",
                self.candidate_events_for.wait_us,
            ),
            (
                "cos.root_done.candidate_events_for.work_us",
                self.candidate_events_for.work_us,
            ),
            (
                "cos.root_done.candidate_events_for_max",
                self.candidate_events_for_max,
            ),
            ("cos.root_done.insert_calls", self.insert.calls),
            ("cos.root_done.insert.wait_us", self.insert.wait_us),
            ("cos.root_done.insert.work_us", self.insert.work_us),
            ("cos.root_done.cursor_set.wait_us", self.cursor_set.wait_us),
            ("cos.root_done.cursor_set.work_us", self.cursor_set.work_us),
            ("cos.root_done.advance_us", self.advance_us),
            ("cos.root_done.open_rows", self.open_rows),
            ("cos.root_done.open.wait_us", self.open.wait_us),
            ("cos.root_done.open.work_us", self.open.work_us),
            ("cos.root_done.facts_calls", f.get.calls),
            ("cos.root_done.facts.get.wait_us", f.get.wait_us),
            ("cos.root_done.facts.get.work_us", f.get.work_us),
            (
                "cos.root_done.facts.events_for.wait_us",
                f.events_for.wait_us,
            ),
            (
                "cos.root_done.facts.events_for.work_us",
                f.events_for.work_us,
            ),
            ("cos.root_done.facts.events_for_max", f.events_for_max),
            (
                "cos.root_done.facts.delivery_get.wait_us",
                f.delivery_get.wait_us,
            ),
            (
                "cos.root_done.facts.delivery_get.work_us",
                f.delivery_get.work_us,
            ),
            ("cos.root_done.facts.children.wait_us", f.children.wait_us),
            ("cos.root_done.facts.children.work_us", f.children.work_us),
            ("cos.root_done.facts.repo_get.wait_us", f.repo_get.wait_us),
            ("cos.root_done.facts.repo_get.work_us", f.repo_get.work_us),
            ("cos.root_done.facts.progress_us", f.progress_us),
            (
                "cos.root_done.facts.progress_git_calls",
                f.progress_git_calls,
            ),
            ("cos.root_done.facts.row_max_us", self.facts_row_max_us),
            ("cos.root_done.advance_other_us", advance_other),
        ]
    }
}

/// 統合 commit の tree（delivery の head）から、この task を `tasks:` に持つ進捗ファイルを探して本文を返す。
/// 読めなければ `None`（事実には「不明」と出る）。`git_calls` に起こした git の process の数を足す
/// （ADR 2026-11-07-root-done-tick-phase-timing D1）。
fn progress_markdown(
    repo: &std::path::Path,
    head: &str,
    task_id: TaskId,
    git_calls: &mut u64,
) -> Option<String> {
    let mut git = |args: &[&str]| -> Option<String> {
        *git_calls += 1;
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let listing = git(&[
        "ls-tree",
        "-r",
        "--name-only",
        head,
        "--",
        "agent-docs/progress",
    ])?;
    for path in listing.lines().filter(|p| p.ends_with(".md")) {
        let Some(text) = git(&["show", &format!("{head}:{path}")]) else {
            continue;
        };
        if root_done::progress_mentions_task(&text, task_id) {
            return Some(text);
        }
    }
    None
}

/// D4: DB の行と進捗ファイルだけから事実を集める。段の時間は `probe` に足す
/// （ADR 2026-11-07-root-done-tick-phase-timing D1）。
pub(crate) fn collect_facts(
    store: &SqliteStore,
    task_id: TaskId,
    probe: &mut FactsProbe,
) -> Result<RootDoneFacts, String> {
    let task = probe
        .get
        .time(|| store.get(task_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("task {task_id} is gone"))?;
    let events = probe
        .events_for
        .time(|| store.events_for(task_id))
        .map_err(|e| e.to_string())?;
    probe.events_for_max = probe.events_for_max.max(events.len() as u64);
    let delivery = probe
        .delivery_get
        .time(|| store.delivery_get(task_id))
        .map_err(|e| e.to_string())?;
    let children: Vec<ChildFact> = probe
        .children
        .time(|| store.children(task_id))
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|c| ChildFact {
            title: c.title,
            status: c.status,
        })
        .collect();
    let progress = delivery.as_ref().and_then(|d| {
        let repo = probe
            .repo_get
            .time(|| store.repo_get(d.repo_id))
            .ok()
            .flatten()?;
        let path: PathBuf = match repo.location {
            WorkspaceSpec::Local { path, .. } => path,
            _ => return None,
        };
        let started = Instant::now();
        let text = progress_markdown(&path, &d.head, task_id, &mut probe.progress_git_calls);
        probe.progress_us = probe.progress_us.saturating_add(us(started.elapsed()));
        text
    });
    Ok(root_done::collect(RootDoneInputs {
        task: &task,
        events: &events,
        delivery: delivery.as_ref(),
        children: &children,
        progress_markdown: progress.as_deref(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 待ちと処理の和は経過時間を超えず、回数は呼んだ数になる（store を呼ばない区間の待ちは 0）。
    #[test]
    fn store_span_counts_calls_and_splits_wait_from_work() {
        let mut span = StoreSpan::default();
        let v = span.time(|| 7);
        span.time(|| ());
        assert_eq!(v, 7);
        assert_eq!(span.calls, 2);
        assert_eq!(span.wait_us, 0);
    }

    #[test]
    fn parts_are_prefixed_and_unique() {
        let probe = RootDoneProbe::default();
        let parts = probe.parts();
        let mut names: Vec<_> = parts.iter().map(|(n, _)| *n).collect();
        assert!(names.iter().all(|n| n.starts_with("cos.root_done.")));
        let len = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), len, "duplicate names");
    }
}
