//! ADR 2026-11-07-health-review-integration 付記 (b) B1〜B5: worker と planner の run に repo の review 方針
//! （base 側の profile の `[review]` 節）を `RunContext.workspace.review_policy` として届ける。
//!
//! - tick の中（`run_extras`。同期）は [`Dispatcher::run_workspace`] で作業場所の説明と、方針を読む repo と target
//!   （[`Dispatcher::review_policy_target`]）を決めるだけにする。
//! - 読み込み（`git archive` と python。上限 [`REVIEW_POLICY_TIMEOUT`]）は run の起動の非同期部分（`run_worker`）が
//!   [`RunWorkspace::into_context`] で `spawn_blocking` を使って 1 回だけ行う。tick を止めない。cache は置かない。
//! - 方針を入れるのは worker の run（atomic task・WU・repair・integration_repair）と planner の run（/1・/2・/3・replan）。
//!   `run_extras` を通らない reviewer の run（設計 review・final review は別の欄で方針を受け取る）と CoS chat、対話 run
//!   には入れない（B3）。
//! - `Absent`・`Unavailable` は `review_policy = None` で、prompt に repo 固有の節を出さない。`Unavailable` の理由は
//!   warn の log に残す（B5）。prompt の節は情報で、関門は `design_gate`（計画）と `policy_review_gate`（候補）にある。
//!
//! LLM は呼ばない。候補（head・作業ツリー）の profile は読まない（I3）。
//!
//! `dispatcher::run_context` の子 module として `#[path]` で読む（`run_context.rs` の `mod worker_profile`）。file を
//! `health_review/` に置くのは、`dispatcher/` の module が size の閾値を超えているため（`health_review` の module 注釈）。

use std::path::PathBuf;

use task_core::design_review::ReviewPolicyState;
use task_core::{Task, TaskId};
use task_worker::protocol::RunWorkspaceContext;

use crate::dispatcher::Dispatcher;
use crate::health_review::policy::{REVIEW_POLICY_TIMEOUT, load_review_policy};

/// tick が決めた、この run の作業場所の情報（`RunExtras.workspace`）。
#[derive(Debug)]
pub(crate) struct RunWorkspace {
    /// ADR-0039 D3: 作業場所の説明（`RunWorkspaceContext.note` にそのまま入る）。
    pub(crate) note: Option<String>,
    /// 方針を読む repo と target。`Ok(None)` は読まない（方針の無い repo・方針を入れない run）、`Err` は
    /// `Unavailable` の理由。
    pub(crate) policy_target: Result<Option<(PathBuf, String)>, String>,
}

impl Default for RunWorkspace {
    fn default() -> Self {
        Self {
            note: None,
            policy_target: Ok(None),
        }
    }
}

impl RunWorkspace {
    /// run の起動の非同期部分で方針を読み、`RunContext.workspace` を組む。説明も方針も無ければ `None`。
    pub(crate) async fn into_context(self, task_id: TaskId) -> Option<RunWorkspaceContext> {
        let state = match self.policy_target {
            Ok(None) => ReviewPolicyState::Absent,
            Err(why) => ReviewPolicyState::Unavailable(why),
            Ok(Some((repo, target))) => tokio::task::spawn_blocking(move || {
                load_review_policy(
                    &repo,
                    &target,
                    crate::health_review::gate::bundled_checker(),
                    REVIEW_POLICY_TIMEOUT,
                )
            })
            .await
            .unwrap_or_else(|e| ReviewPolicyState::Unavailable(format!("読み込みの task: {e}"))),
        };
        if let ReviewPolicyState::Unavailable(why) = &state {
            tracing::warn!(task_id = %task_id, reason = %why, "review policy unavailable");
        }
        let review_policy = state.policy().cloned();
        if self.note.is_none() && review_policy.is_none() {
            return None;
        }
        Some(RunWorkspaceContext {
            note: self.note,
            review_policy,
        })
    }
}

impl Dispatcher {
    /// `run_extras` の作業場所の情報。`with_policy` は方針を入れる run（対話 run でない）か。
    pub(crate) fn run_workspace(
        &self,
        task: &Task,
        note: Option<String>,
        with_policy: bool,
    ) -> RunWorkspace {
        let policy_target = if with_policy {
            self.review_policy_target(task)
        } else {
            Ok(None)
        };
        RunWorkspace {
            note,
            policy_target,
        }
    }
}

#[cfg(test)]
#[path = "worker_profile/tests.rs"]
mod tests;
