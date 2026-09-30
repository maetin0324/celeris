//! タスク状態機械の純粋関数。DESIGN.md / ADR-0002 D2,D3,D8 で定義された
//! 遷移規則をそのまま実装する。I/O・時計・乱数は一切使わない（ADR-0001 D2）。

use crate::model::{Status, TaskKind};

/// `transition` の入力となる現在状態のスナップショット。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateView {
    pub kind: TaskKind,
    pub status: Status,
    pub attempts: u32,
    pub max_retries: u32,
}

/// タスクの状態遷移を駆動するトリガー。
///
/// ADR-0074 D2.2（Phase F3 途中確認）: `PhaseGate` が `String` を持つため `Copy` は外した
/// （導入前は全 variant が `Copy` だった。呼び出し側は値渡しのままなので影響は無い）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    Accept,
    Dispatch,
    WorkerDone,
    WorkerQuestion,
    WorkerError {
        retryable: bool,
    },
    LeaseExpired,
    ReviewPass,
    ReviewFail,
    Answer,
    Approve,
    Reject,
    Cancel,
    /// 供給側失敗（レート制限・認証失敗・枯渇・起動失敗）: `running → ready`、attempts 据え置き（ADR-0010 D1, P-21）。
    Requeue,
    /// 先行タスクが `failed`/`cancelled` になった後続: 非終端 → `cancelled`（ADR-0010 D1, P-9）。
    DependencyFailed,
    /// 委譲した子が全て終端になり、集約 run を行う親: `reviewing → ready`、attempts 据え置き（ADR-0016 D3 / M1）。
    Aggregate,
    /// 委譲した子が失敗した親（ADR-0021 D1）: やり直せるなら `reviewing → ready`（attempts 消費）、
    /// やり直せないなら `reviewing → blocked`（人間の判断待ち）。**`failed` にはしない**。
    ChildFailed,
    /// ADR-0044 D2（Phase 53）: 人のコメントによる割り込み。`running`/`reviewing → ready`、
    /// attempts は据え置き（人が口を出しただけで試行を 1 回使わせない）。`Outcome::reason` は
    /// **`"comment"`**（`name()` の `"interrupt"` とは別。ADR-0044 D2 の表がそう書いている）。
    Interrupt,
    /// ADR-0044 D2（Phase 53）: 終端のタスクの再開（`POST /tasks/{id}/reopen`）。
    /// `done`/`failed → ready`、attempts は 0 に戻す。`cancelled` は再開しない（worktree が無い）。
    Reopen,
    /// ADR-0051: 完了成果の再レビュー（実装runを再実行しない）。
    Rereview,
    /// ADR-0044 D6（Phase 55）: 案件の中止による連鎖。遷移は `Cancel` と同じ（非終端 → `cancelled`、
    /// attempts 据え置き）で、`Event::Transitioned.reason` だけが `"project_cancelled"` になる
    /// （タイムラインで「自分が止めたのか、案件ごと止まったのか」が読めるように）。
    ProjectCancelled,
    /// ADR-0044 D6（Phase 55）: 途中目標の中止による連鎖。理由は `"milestone_cancelled"`。
    MilestoneCancelled,
    /// ADR-0079 D4 / §7 R1b: 木の親 task が中止（または失敗）で終わったことによる、子 task（とその
    /// subtree）の連鎖の中止。遷移は `Cancel` と同じで、理由は `"parent_cancelled"`。
    ParentCancelled,
    /// ADR-0046 D5（Phase 59）: 担当が決まらない（matching の候補が 1 つも無い）タスク:
    /// `ready → blocked`、attempts 据え置き。人が組織を直すか担当を指定したら `Answer` で再開する
    /// （ADR-0021 の質問経路と同じ出口）。
    Unroutable,
    /// ADR-0070 D3（Phase 116）: インフラ都合の失敗（lease 失効・切替中断・result.json 不在・
    /// セッション再開拒否・レート制限・DB busy）: `running → ready`、attempts 据え置き
    /// （`Requeue` と同じ形だが、`consecutive_infra_requeues` という別のカウンタで数える。
    /// `[dispatch] max_infra_retries` に達したら `WorkerError{retryable:false}` で打ち切る）。
    InfraRequeue,
    /// ADR-0072 D6/D11（Phase E1）: `running → ready`、attempts 据え置き。`why` は E1 では常に
    /// `Continue`（予算切れ・yield の続き）。`why` ごとに `reason` の文字列が変わる（`ContinueWhy::name`）。
    /// `advance`/`work_unit_retry`/`planned`/`replan` は E2 以降の配線先。
    Continue {
        why: crate::execution::ContinueWhy,
    },
    /// ADR-0072 D16（Phase E4）: 最終レビューの修復できる不合格（D16 の分類表、repair の上限内）。
    /// `Reviewing → Ready`、attempts は据え置き（同じ `Event::Transitioned{reason:"review_repair"}`
    /// のトランザクションで、呼び出し側〈`task_ops::execution`〉が repair の WorkUnit を作る）。
    ReviewRepair,
    /// ADR-0074 D2.2（Phase F3 途中確認）: 停止点の工程の統合の後。`Running → Blocked`、
    /// `reason = "awaiting_human"`、attempts 不変。`phase` はどの工程の後かを運ぶだけ
    /// （遷移そのものの判定には使わない）。
    PhaseGate {
        phase: String,
    },
    /// ADR-0074 D2.2/D2.4: 人の「続ける」/「replan」。`Blocked → Ready`、
    /// `reason = "phase_continue"` / `"phase_replan"`、attempts 不変。
    PhaseResume {
        mode: crate::pause::PhaseResumeMode,
    },
    /// ADR-0080 D4: browser の人待ち（登録・承認）を保存した。`Running → Blocked`、attempts 不変。
    /// `reason` は `"waiting_for_auth"` / `"waiting_for_approval"`（`approval` で分ける）。
    BrowserWait {
        approval: bool,
    },
    /// ADR-0080 D4: 専用の browser 操作（登録完了・承認）で wait を一度だけ解決した。
    /// `Blocked → Ready`、attempts 不変、`reason = "browser_resume"`。
    BrowserResume,
    /// ADR-0080 D4: 拒否・wait の期限切れ。`Blocked → Failed`（自動 retry なし）、attempts 不変。
    /// `reason` は固定の `"browser_wait_expired"` / `"approval_denied"`。
    BrowserFail {
        expired: bool,
    },
    /// ADR-0079 D8（Phase R3b）: root の /3 の計画が人の承認を要する（決定を含む / `review: human` の段階 /
    /// 上限に近い）。計画の採用の直後に `Running → Blocked`、`reason = "awaiting_plan_approval"`、attempts 不変
    /// （`PhaseGate` と同じ形）。再開は `PhaseResume{PlanApprove | PlanReplan}`、取り下げは `Cancel`。
    PlanGate {
        plan_id: String,
    },
    /// ADR-0074「F5-fix8 実装時の明確化」: 計画の unit がすべて終わっている（unit が 1 つも無い場合を含む）
    /// のに `ready` のままの Task を、run を起こさずに最終レビューへ進める。採用した計画（replan で何も
    /// 足さなかった版を含む）がまだ最終レビューを受けていないときだけ dispatcher が使う。
    /// `Ready → Reviewing`（Execute kind のみ）、`reason = "plan_complete"`、attempts 不変。
    PlanComplete,
    /// ADR-0090 D2: worker の run が `result.json` の `wait`（クラスタ job の終了待ち）で終わった
    /// （または v2 の工程に起こせる unit が無く、job を待つ unit だけが残った）。`Running → Blocked`、
    /// attempts 不変、`reason = "waiting_for_cluster_jobs"`。lease を解放する（provider の枠・account を持たない）。
    ClusterJobWait,
    /// ADR-0090 D2: 待っていた job がすべて終わった（daemon の poll）。`Blocked → Ready`、attempts 不変、
    /// `reason = "cluster_job_resume"`。次の run は continuation（前置きに job の最終状態と終了コード）。
    ClusterJobResume,
}

impl Trigger {
    /// snake_case の trigger 名。成功時の `Outcome::reason` および失敗時の
    /// `InvalidTransition` のメッセージに使う。
    pub fn name(&self) -> &'static str {
        match self {
            Trigger::Accept => "accept",
            Trigger::Dispatch => "dispatch",
            Trigger::WorkerDone => "worker_done",
            Trigger::WorkerQuestion => "worker_question",
            Trigger::WorkerError { .. } => "worker_error",
            Trigger::LeaseExpired => "lease_expired",
            Trigger::ReviewPass => "review_pass",
            Trigger::ReviewFail => "review_fail",
            Trigger::Answer => "answer",
            Trigger::Approve => "approve",
            Trigger::Reject => "reject",
            Trigger::Cancel => "cancel",
            Trigger::Requeue => "requeue",
            Trigger::DependencyFailed => "dependency_failed",
            Trigger::Aggregate => "aggregate",
            Trigger::ChildFailed => "child_failed",
            Trigger::Interrupt => "interrupt",
            Trigger::Reopen => "reopen",
            Trigger::Rereview => "rereview",
            Trigger::ProjectCancelled => "project_cancelled",
            Trigger::MilestoneCancelled => "milestone_cancelled",
            Trigger::ParentCancelled => "parent_cancelled",
            Trigger::Unroutable => "unroutable",
            Trigger::InfraRequeue => "infra_requeue",
            Trigger::Continue { why } => why.name(),
            Trigger::ReviewRepair => "review_repair",
            Trigger::PhaseGate { .. } => "awaiting_human",
            Trigger::PhaseResume { mode } => mode.name(),
            Trigger::BrowserWait { approval: false } => "waiting_for_auth",
            Trigger::BrowserWait { approval: true } => "waiting_for_approval",
            Trigger::BrowserResume => "browser_resume",
            Trigger::BrowserFail { expired: true } => "browser_wait_expired",
            Trigger::BrowserFail { expired: false } => "approval_denied",
            Trigger::PlanGate { .. } => "awaiting_plan_approval",
            Trigger::PlanComplete => "plan_complete",
            Trigger::ClusterJobWait => crate::cluster_job::REASON_WAITING,
            Trigger::ClusterJobResume => crate::cluster_job::REASON_RESUME,
        }
    }

    /// `Outcome::reason`（`Event::Transitioned.reason` に入る文字列）。ふつうは `name()` と同じだが、
    /// `Interrupt` だけは ADR-0044 D2 の表のとおり `"comment"`（「なぜ ready に戻ったか」を人が読む）。
    pub fn reason(&self) -> &'static str {
        match self {
            Trigger::Interrupt => "comment",
            other => other.name(),
        }
    }
}

/// 遷移が成功した場合の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub next: Status,
    pub attempts: u32,
    pub reason: &'static str,
}

/// 遷移が許可されていない場合のエラー。失敗した `(status, kind, trigger)` を
/// 保持し、メッセージから追跡できるようにする。
#[derive(Debug, thiserror::Error)]
#[error("invalid transition: status={status:?} kind={kind:?} trigger={trigger}")]
pub struct InvalidTransition {
    pub status: Status,
    pub kind: TaskKind,
    pub trigger: &'static str,
}

fn invalid(s: &StateView, t: &Trigger) -> InvalidTransition {
    InvalidTransition {
        status: s.status,
        kind: s.kind,
        trigger: t.name(),
    }
}

/// リトライ判定込みの `attempts` 更新: `attempts + 1` が `max_retries` を
/// 超えたら `Failed`、超えなければ `Ready` を返す。
fn retry_or_fail(s: &StateView, reason: &'static str) -> Outcome {
    let attempts = s.attempts + 1;
    let next = if attempts > s.max_retries {
        Status::Failed
    } else {
        Status::Ready
    };
    Outcome {
        next,
        attempts,
        reason,
    }
}

/// タスクの状態機械。DESIGN.md / ADR-0002 D2,D3,D8 の遷移表を実装する純粋関数。
pub fn transition(s: &StateView, t: &Trigger) -> Result<Outcome, InvalidTransition> {
    match t {
        // ADR-0010 D1（P-4 / P-9）: Cancel と DependencyFailed は非終端状態からのみ cancelled へ。
        // ADR-0044 D6（Phase 55）: 案件・途中目標の中止による連鎖も同じ遷移（理由だけが違う）。
        Trigger::Cancel
        | Trigger::DependencyFailed
        | Trigger::ProjectCancelled
        | Trigger::MilestoneCancelled
        | Trigger::ParentCancelled => {
            if s.status.is_terminal() {
                Err(invalid(s, t))
            } else {
                Ok(Outcome {
                    next: Status::Cancelled,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            }
        }

        // ADR-0010 D1（P-21）: 供給側失敗は attempts を消費せず ready に戻す。
        // ADR-0070 D3（Phase 116）: インフラ都合の失敗も同じ形（別のカウンタで数える。dispatcher 側）。
        Trigger::Requeue | Trigger::InfraRequeue => {
            if s.status == Status::Running {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0072 D6/D11（Phase E1）: 予算切れ・yield の続き。attempts は消費しない
        // （`retry_policy::attempt_history` は `reason` がここで返す静的な名前を試行に数えない）。
        Trigger::Continue { .. } => {
            if s.status == Status::Running {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0016 M1: 集約 run は attempts を消費せず ready に戻す（reviewing からのみ）。
        Trigger::Aggregate => {
            if s.status == Status::Reviewing {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0021 D1: 子の失敗は親が引き継がない。やり直せるなら ready、やり直せないなら人間に投げる（blocked）。
        Trigger::ChildFailed => {
            if s.status == Status::Reviewing {
                let attempts = s.attempts + 1;
                if attempts <= s.max_retries {
                    Ok(Outcome {
                        next: Status::Ready,
                        attempts,
                        reason: t.name(),
                    })
                } else {
                    // attempts は据え置き（人が答えたら、その回答を持って走り直せるようにする）。
                    Ok(Outcome {
                        next: Status::Blocked,
                        attempts: s.attempts,
                        reason: t.name(),
                    })
                }
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0044 D2: 人のコメントで走っている run を止める。attempts は据え置き、理由は `comment`。
        Trigger::Interrupt => {
            if matches!(s.status, Status::Running | Status::Reviewing) {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.reason(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0044 D2 / ADR-0054 Phase 113 D3 追記: 終端のタスクの再判定。`done` は無条件。`failed` は
        // 「直前の実装 run 自体は `done` で、review 判定だけが不合格だった」場合に限って許す
        // （`task_ops::comment::rereview` が、その判別（最後の遷移が `review_fail` かどうか）を event
        // 履歴で行ってから呼ぶ。この状態機械は純粋関数で event 履歴を持たないので、ここでは
        // `Status::Failed` からの遷移そのものを許すところまでしか見ない）。attempts は変えない
        // （やり直すのは判定だけなので、人の承認の `Approval` 子タスクの title（`attempts` を含む）も
        // 変わらず、既に承認済みの子がそのまま再利用される。ADR-0054 D2 と同じ「人の承認は保持する」
        // 考え方）。
        Trigger::Rereview => {
            if matches!(s.status, Status::Done | Status::Failed) && s.kind == TaskKind::Execute {
                // `done` からは attempts をそのまま引き継ぐ（`ReviewPass` は attempts を進めないので、
                // レビュー中だったときの値と同じ）。`failed` は、その `failed` にした
                // `Trigger::ReviewFail`（`retry_or_fail`）が attempts を 1 進めた**あと**の値なので、
                // 1 戻す。こうしないと、その直前の review 中に作られた `Human` 条件の
                // `Approval` 子タスクの title（`human_approval_title` が `attempts + 1` を含む）と、
                // 再判定時に探す title がずれて、既に承認済みの子が見つからず人に二度承認させることに
                // なる（ADR-0054 D2 と同じ「人の承認は保持する」を `failed` からの再判定でも保つ。
                // Phase 113 D3）。
                let attempts = if s.status == Status::Failed {
                    s.attempts.saturating_sub(1)
                } else {
                    s.attempts
                };
                Ok(Outcome {
                    next: Status::Reviewing,
                    attempts,
                    reason: t.reason(),
                })
            } else {
                Err(invalid(s, t))
            }
        }
        Trigger::Reopen => {
            if matches!(s.status, Status::Done | Status::Failed) {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: 0,
                    reason: t.reason(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::Accept => {
            if s.status == Status::Draft {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::Dispatch => {
            if s.status == Status::Ready && s.kind != TaskKind::Approval {
                Ok(Outcome {
                    next: Status::Running,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::WorkerDone => {
            if s.status == Status::Running {
                Ok(Outcome {
                    next: Status::Reviewing,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::WorkerQuestion => {
            if s.status == Status::Running {
                Ok(Outcome {
                    next: Status::Blocked,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0046 D5（Phase 59）: 担当が見つからないタスクは人に聞く（`ready → blocked`）。
        Trigger::Unroutable => {
            if s.status == Status::Ready {
                Ok(Outcome {
                    next: Status::Blocked,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::WorkerError { retryable } => {
            if s.status == Status::Running {
                let attempts = s.attempts + 1;
                let next = if *retryable && attempts <= s.max_retries {
                    Status::Ready
                } else {
                    Status::Failed
                };
                Ok(Outcome {
                    next,
                    attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::LeaseExpired => {
            if s.status == Status::Running {
                Ok(retry_or_fail(s, t.name()))
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::ReviewPass => {
            if s.status == Status::Reviewing {
                Ok(Outcome {
                    next: Status::Done,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::ReviewFail => {
            if s.status == Status::Reviewing {
                Ok(retry_or_fail(s, t.name()))
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0072 D16（Phase E4）: repair は attempts を消費しない（`ReviewFail` と違い
        // `retry_or_fail` を通さない）。
        Trigger::ReviewRepair => {
            if s.status == Status::Reviewing {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::Answer => {
            if s.status == Status::Blocked {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::Approve => {
            if s.kind == TaskKind::Approval && s.status == Status::Ready {
                Ok(Outcome {
                    next: Status::Done,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        Trigger::Reject => {
            if s.kind == TaskKind::Approval && s.status == Status::Ready {
                Ok(Outcome {
                    next: Status::Failed,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0074 D2.2（Phase F3 途中確認）: 停止点の工程の統合の後。attempts は変えない
        // （replay の attempts 計算に入らない。`retry_or_fail` を通さない）。
        Trigger::PhaseGate { .. } => {
            if s.status == Status::Running {
                Ok(Outcome {
                    next: Status::Blocked,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0079 D8（Phase R3b）: root の計画の承認待ち。`PhaseGate` と同じ形（attempts 不変）。
        Trigger::PlanGate { .. } => {
            if s.status == Status::Running {
                Ok(Outcome {
                    next: Status::Blocked,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0074「F5-fix8 実装時の明確化」: 完了済みの計画の最終レビュー（run なし、attempts 不変）。
        Trigger::PlanComplete => {
            if s.status == Status::Ready && s.kind == TaskKind::Execute {
                Ok(Outcome {
                    next: Status::Reviewing,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0074 D2.2/D2.4: 人の「続ける」/「replan」。attempts は変えない。
        Trigger::PhaseResume { .. } => {
            if s.status == Status::Blocked {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }

        // ADR-0080 D4: browser の人待ち。Status の variant は増やさず `Blocked` を使う。
        Trigger::BrowserWait { .. } => {
            if s.status == Status::Running {
                Ok(Outcome {
                    next: Status::Blocked,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }
        Trigger::BrowserResume => {
            if s.status == Status::Blocked {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }
        Trigger::BrowserFail { .. } => {
            if s.status == Status::Blocked {
                Ok(Outcome {
                    next: Status::Failed,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }
        // ADR-0090 D2: クラスタ job の durable wait。Status は増やさず `Blocked` を使う（browser の wait と同じ）。
        Trigger::ClusterJobWait => {
            if s.status == Status::Running {
                Ok(Outcome {
                    next: Status::Blocked,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }
        Trigger::ClusterJobResume => {
            if s.status == Status::Blocked {
                Ok(Outcome {
                    next: Status::Ready,
                    attempts: s.attempts,
                    reason: t.name(),
                })
            } else {
                Err(invalid(s, t))
            }
        }
    }
}

#[cfg(test)]
#[path = "transition/tests.rs"]
mod tests;
