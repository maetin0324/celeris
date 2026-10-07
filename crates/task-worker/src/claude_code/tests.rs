use std::sync::Mutex;
use std::time::Duration;

use task_core::{ArtifactRef, DelegateTask, RateLimitObservation};

use super::*;
use crate::protocol::{GenreContext, PROTOCOL_VERSION, RunContext};

#[path = "cos_chat_tests.rs"]
mod cos_chat_tests;

#[derive(Default)]
struct RecordingSink {
    progress: Mutex<Vec<String>>,
    /// ADR-0048 D2（Phase 60a）: 構造化した進行（`msg` と一緒に）。
    structured: Mutex<Vec<(String, task_core::ProgressFields)>>,
    delegated: Mutex<Vec<Vec<DelegateTask>>>,
    rate_limits: Mutex<Vec<RateLimitObservation>>,
    /// ADR-0054 D1（Phase 67）: `session_established` に報告された id。
    session_established: Mutex<Vec<String>>,
    /// ADR-0054 D1（Phase 67）: `session_resume_failed` に報告された理由。
    session_resume_failed: Mutex<Vec<String>>,
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D5: `policy_violation` の呼び出し。
    violations: Mutex<Vec<crate::tool_policy::ToolPolicyViolation>>,
}

impl EventSink for RecordingSink {
    fn policy_violation(&self, violation: &crate::tool_policy::ToolPolicyViolation) {
        self.violations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(violation.clone());
    }
    fn progress(&self, msg: &str) {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(msg.to_string());
    }
    fn progress_with(&self, msg: &str, fields: &task_core::ProgressFields) {
        self.progress(msg);
        self.structured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((msg.to_string(), fields.clone()));
    }
    fn artifact(&self, _artifact: &ArtifactRef) {}
    fn delegate(&self, tasks: &[DelegateTask]) {
        self.delegated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tasks.to_vec());
    }
    fn rate_limit(&self, obs: RateLimitObservation) {
        self.rate_limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(obs);
    }
    fn session_established(&self, session_id: &str) {
        self.session_established
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(session_id.to_string());
    }
    fn session_resume_failed(&self, reason: &str) {
        self.session_resume_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(reason.to_string());
    }
}

fn stub_claude(dir: &Path, script: &str) -> ClaudeCodeConfig {
    let path = dir.join("claude_stub.sh");
    // ETXTBSY 対策（ADR-0010 D10）: テストプロセス自身が書き込み fd を持たないよう別プロセスで書く。
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{script}\n"));
    ClaudeCodeConfig {
        command: path.to_string_lossy().into_owned(),
        ..ClaudeCodeConfig::default()
    }
}

fn sample_req(workspace: std::path::PathBuf) -> RunRequest {
    RunRequest {
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: crate::protocol::tests::sample_task(),
        artifacts_dir: workspace.join("artifacts"),
        workspace,
        work_dir: None,
        context: RunContext::default(),
    }
}

fn default_limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_secs(30),
        kill_grace: Duration::from_millis(200),
    }
}

#[test]
fn build_prompt_includes_objective_criteria_and_result_file_instructions() {
    let task = crate::protocol::tests::sample_task();
    let mut context = RunContext::default();
    context.prior_review.push(crate::protocol::PriorReview {
        criterion: 0,
        pass: false,
        reason: "cargo test exit 101".into(),
    });
    let prompt = build_prompt(&task, &context, "run-xyz", "artifacts");
    assert!(prompt.contains(&task.objective));
    assert!(prompt.contains("cargo test exit 101"));
    assert!(prompt.contains("artifacts/result.json"));
    assert!(prompt.contains("reviewer will independently re-run"));
    assert!(prompt.contains("run-xyz"));
    assert!(prompt.contains("attempt 1 of"));
}

/// ADR-0072 D9/D21（Phase E2）: `context.work_unit` があれば `## Objective` は WU の objective に
/// 差し替わり、Task 全体の目的は参考として、受け入れ条件は WU の `done_when` になる。
/// `context.work_unit` が無い run は前のテストのとおり 1 バイトも変わらない。
/// ADR-0074 D1.2（Phase F2b）: WU ごとの worktree で走る run には作業ブランチと「他の WU の
/// ファイルに触らない」を出す。`branch` の無い run（v1）には出さない。
#[test]
fn a_parallel_work_unit_prompt_names_its_branch_and_forbids_touching_siblings() {
    let task = crate::protocol::tests::sample_task();
    let mut wu = crate::protocol::WorkUnitPromptContext {
        key: "api".into(),
        title: "api".into(),
        objective: "add the api".into(),
        ..Default::default()
    };
    let plain = build_prompt(
        &task,
        &RunContext {
            work_unit: Some(wu.clone()),
            ..RunContext::default()
        },
        "run-1",
        "artifacts",
    );
    assert!(!plain.contains("作業ブランチ"));
    // ADR-0074 付記 2026-10-05 D3: 範囲 check の環境変数の一文も `branch` があるときだけ。
    assert!(!plain.contains("CELERIS_WU_BASE"));
    wu.branch = Some("celeris-wu/T/api".into());
    wu.parallel_siblings = vec!["store: store layer".into()];
    let prompt = build_prompt(
        &task,
        &RunContext {
            work_unit: Some(wu),
            ..RunContext::default()
        },
        "run-1",
        "artifacts",
    );
    assert!(prompt.contains("`celeris-wu/T/api`"));
    assert!(prompt.contains("commit してかまいません"));
    assert!(prompt.contains("他の WorkUnit が担当するファイルには触らない"));
    assert!(prompt.contains("store: store layer"));
    assert!(prompt.contains(
        "`CELERIS_WU_BASE`（この WorkUnit の base commit）と `CELERIS_WU_TARGET`（統合先のブランチ）"
    ));
}

/// ADR-0079 D7（Phase R3a）: leaf の前置きの「人の決定」節（回答があるときだけ、固定の書式の行）と、木の節点の
/// worker の run の「人への決定の要求」節（`decision_requests` のときだけ）。どちらも無ければプロンプトは変わらない。
#[test]
fn leaf_prompt_carries_human_decisions_and_the_decision_request_contract() {
    let task = crate::protocol::tests::sample_task();
    let wu = crate::protocol::WorkUnitPromptContext {
        key: "api".into(),
        title: "api".into(),
        objective: "add the api".into(),
        ..Default::default()
    };
    let plain = build_prompt(
        &task,
        &RunContext {
            work_unit: Some(wu.clone()),
            ..RunContext::default()
        },
        "run-1",
        "artifacts",
    );
    assert!(!plain.contains(crate::preamble::HUMAN_DECISIONS_HEADING));
    assert!(!plain.contains("人への決定の要求"));
    let with = build_prompt(
        &task,
        &RunContext {
            work_unit: Some(crate::protocol::WorkUnitPromptContext {
                human_decisions: vec![
                    "- h1 which backend: manual（推奨と異なる） — trial first".into(),
                ],
                ..wu
            }),
            decision_requests: true,
            ..RunContext::default()
        },
        "run-1",
        "artifacts",
    );
    assert!(
        with.contains(&format!(
            "{}\n- h1 which backend: manual（推奨と異なる） — trial first\n",
            crate::preamble::HUMAN_DECISIONS_HEADING
        )),
        "{with}"
    );
    assert!(with.contains("## 人への決定の要求（ADR-0079 D7）"));
    assert!(
        with.contains("`artifacts/result.json` に `decisions`"),
        "{with}"
    );
}

/// ADR-0079 付記 R7-5 D3: 前の run の check の不合格は次の run の前置きに出る（cwd・判定文・`plan_issue` の申告の仕方）。
/// 空ならプロンプトは変わらない。
#[test]
fn leaf_prompt_carries_the_previous_runs_failed_checks() {
    let task = crate::protocol::tests::sample_task();
    let wu = crate::protocol::WorkUnitPromptContext {
        key: "lan-verify".into(),
        title: "lan-verify".into(),
        objective: "verify over the LAN".into(),
        ..Default::default()
    };
    let plain = build_prompt(
        &task,
        &RunContext {
            work_unit: Some(wu.clone()),
            ..RunContext::default()
        },
        "run-1",
        "artifacts",
    );
    assert!(
        !plain.contains("## 前回の run の check の不合格"),
        "{plain}"
    );
    let detail = "cmd=\"bash check_lan.sh http://192.168.1.103:8000/\" exit=Some(1) expected=0 stdout_tail=\"FAIL not listening\" stderr_tail=\"\"";
    let with = build_prompt(
        &task,
        &RunContext {
            work_unit: Some(crate::protocol::WorkUnitPromptContext {
                previous_check_failures: vec!["cwd: /ws/01TASK".into(), detail.into()],
                ..wu
            }),
            ..RunContext::default()
        },
        "run-1",
        "artifacts",
    );
    assert!(with.contains("## 前回の run の check の不合格\n"), "{with}");
    assert!(with.contains("- cwd: /ws/01TASK\n"), "{with}");
    assert!(with.contains(&format!("- {detail}\n")), "{with}");
    assert!(
        with.contains(r#"`artifacts/result.json` に `{"yield": {"plan_issue": "#),
        "{with}"
    );
    // 節は WU の objective の後（目的を読んでから前回の不合格を読む）。
    let objective_at = with.find("verify over the LAN").unwrap_or(usize::MAX);
    let section_at = with.find("## 前回の run の check の不合格").unwrap_or(0);
    assert!(objective_at < section_at, "{with}");
}

/// ADR-0074 D1.1（Phase F2b）: `parallel = true` の planner run だけ v2 の書き方（工程・同じ工程 =
/// 並列可・工程内の依存は 1 つまで）を出す。`false` は従来のプロンプトのまま。
#[test]
fn the_planner_prompt_explains_v2_phases_only_when_parallel() {
    let task = crate::protocol::tests::sample_task();
    let base = crate::protocol::ExecutionPlannerContext {
        max_work_units: 8,
        ..Default::default()
    };
    let serial = build_prompt(
        &task,
        &RunContext {
            execution_planner: Some(base.clone()),
            ..RunContext::default()
        },
        "run-p",
        "artifacts",
    );
    assert!(serial.contains(r#"{"schema":"celeris.execution-plan/1","rationale""#));
    assert!(!serial.contains("Phases and parallel WorkUnits"));
    let parallel = build_prompt(
        &task,
        &RunContext {
            execution_planner: Some(crate::protocol::ExecutionPlannerContext {
                parallel: true,
                max_phases: 5,
                max_work_units: 10,
                ..base
            }),
            ..RunContext::default()
        },
        "run-p",
        "artifacts",
    );
    assert!(parallel.contains(r#"{"schema":"celeris.execution-plan/2","rationale""#));
    assert!(parallel.contains("same phase may run in parallel"));
    assert!(parallel.contains("at most one"));
    assert!(parallel.contains("1 to 5 phases"));
    // ADR-0074 D3.7（Phase F4b (f)）: v2 の planner には children の書き方がある（v1 には無い）。
    assert!(parallel.contains("\"children\":[{\"key\""), "{parallel}");
    assert!(parallel.contains("child:<key>"));
    assert!(!serial.contains("#### Child tasks"));
}

#[test]
fn build_prompt_replaces_the_objective_and_acceptance_with_the_work_unit_when_present() {
    let task = crate::protocol::tests::sample_task();
    let context = RunContext {
        work_unit: Some(crate::protocol::WorkUnitPromptContext {
            key: "core-model".into(),
            title: "core model".into(),
            objective: "add the WorkUnit data model".into(),
            done_when: vec!["cargo test -p task-core passes".into()],
            task_objective_excerpt: task.objective.clone(),
            dependency_summaries: vec!["survey: 完了".into()],
            plan_overview: vec!["survey done, core-model running, tests pending".into()],
            branch: None,
            parallel_siblings: Vec::new(),
            human_decisions: Vec::new(),
            previous_check_failures: Vec::new(),
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-wu", "artifacts");
    assert!(prompt.contains("add the WorkUnit data model"));
    assert!(prompt.contains("## このタスク全体の目的（参考）"));
    assert!(prompt.contains(&task.objective));
    assert!(prompt.contains("cargo test -p task-core passes"));
    assert!(prompt.contains("## 最終レビューで確かめる Task の受け入れ条件（参考）"));
    assert!(prompt.contains("survey: 完了"));
    assert!(prompt.contains("survey done, core-model running, tests pending"));
}

/// ADR-0072 D10（Phase E1）: 予算の予告と rolling checkpoint の指示は execute run（対話を除く）
/// に出るが、対話 run には出ない（D10 の除外表のとおり）。
#[test]
fn budget_preamble_appears_for_execute_runs_but_not_conversation_runs() {
    let task = crate::protocol::tests::sample_task();
    let ordinary = build_prompt(&task, &RunContext::default(), "run-budget", "artifacts");
    assert!(ordinary.contains("## 予算 (budget)"), "{ordinary}");
    assert!(ordinary.contains("checkpoint.json"), "{ordinary}");
    assert!(ordinary.contains(&format!("{}", task.budget.max_turns)));
    assert!(ordinary.contains(&format!("{}", task.budget.max_wall_secs)));

    let conversation_context = RunContext {
        conversation_addressee: Some(crate::protocol::ConversationAddressee::Other),
        ..RunContext::default()
    };
    let conversation = build_prompt(&task, &conversation_context, "run-conv", "artifacts");
    assert!(!conversation.contains("## 予算 (budget)"), "{conversation}");
}

/// ADR-0072 D9（Phase E1）: `request.json`/`prompt.txt` に続きの実行の節と checkpoint が載る。
/// `context.continuation` を持たない run のプロンプトは、D10 の追加分を除きバイト単位で同じ
/// （budget_preamble/continuation_section 以外の内容は変わらない）。
#[test]
fn build_prompt_carries_the_continuation_section_when_present() {
    let task = crate::protocol::tests::sample_task();
    let context = RunContext {
        continuation: Some(crate::protocol::ContinuationContext {
            run_seq: 2,
            previous_end: "budget_exhausted(turns)".into(),
            checkpoint: serde_json::json!({
                "completed": ["A"],
                "remaining": ["B"],
                "next_action": "do B",
            }),
            prior_runs: vec!["Run #1 budget_exhausted(turns)".into()],
            cluster_jobs: None,
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-cont", "artifacts");
    assert!(prompt.contains("## 続きの実行（Run #2）"), "{prompt}");
    assert!(
        prompt.contains("### checkpoint（Run #1 の終わり）"),
        "{prompt}"
    );
    assert!(prompt.contains("do B"), "{prompt}");
    // 前の run の会話全文は載らない。
    assert!(
        !prompt.contains("prior conversation transcript"),
        "{prompt}"
    );
}

/// Phase 98（ADR-0054 D2、実機障害 2026-09-22）: 対話 run（`conversation_addressee` が Some）の
/// 前置きには `artifacts/delegate.json` の段落が出ず、代わりに「返事だけを書く」1 文が入る。
/// 対話でない run の前置きは Phase 97 までと 1 バイトも変わらない。
#[test]
fn conversation_runs_do_not_get_the_delegate_json_paragraph() {
    let task = crate::protocol::tests::sample_task();
    let ordinary = build_prompt(&task, &RunContext::default(), "run-ord", "artifacts");
    assert!(ordinary.contains("artifacts/delegate.json"), "{ordinary}");
    assert!(
        !ordinary.contains("この run は返事だけを書く"),
        "{ordinary}"
    );

    let secretary_context = RunContext {
        conversation_addressee: Some(crate::protocol::ConversationAddressee::Secretary),
        ..RunContext::default()
    };
    let secretary = build_prompt(&task, &secretary_context, "run-cos", "artifacts");
    assert!(
        !secretary.contains("artifacts/delegate.json"),
        "{secretary}"
    );
    assert!(
        secretary.contains(
            "この run は返事だけを書く。仕事は返事の `actions` で作る（ファイルは書けない）。"
        ),
        "{secretary}"
    );

    let other_context = RunContext {
        conversation_addressee: Some(crate::protocol::ConversationAddressee::Other),
        ..RunContext::default()
    };
    let other = build_prompt(&task, &other_context, "run-other", "artifacts");
    assert!(!other.contains("artifacts/delegate.json"), "{other}");
    // ADR-0098 D6（Phase R7-10）: 後続の起票の段落も対話でない run だけ。
    assert!(ordinary.contains("artifacts/followups.json"), "{ordinary}");
    assert!(ordinary.contains("celerisctl add"), "{ordinary}");
    assert!(!secretary.contains("followups.json"), "{secretary}");
    assert!(!other.contains("followups.json"), "{other}");
    assert!(
        other.contains(
            "この run は返事だけを書く。仕事は返事の `actions` で作る（ファイルは書けない）。"
        ),
        "{other}"
    );

    // 対話でない run（Phase 97 までの構成）は前置きが 1 バイトも変わらない。
    assert_eq!(
        ordinary,
        build_prompt(&task, &RunContext::default(), "run-ord", "artifacts")
    );
}

/// `context.answers`（ADR-0010 D3, P-10）は Execute/Plan プロンプトに反映される。
#[test]
fn build_prompt_includes_answers_from_human_for_execute_and_plan() {
    let mut context = RunContext::default();
    context.answers.push(crate::protocol::Answer {
        question: "which crate version?".into(),
        answer: "1.0".into(),
    });

    let execute_task = crate::protocol::tests::sample_task();
    let execute_prompt = build_prompt(&execute_task, &context, "run-a1", "artifacts");
    assert!(execute_prompt.contains("## Answers from a human to your earlier questions"));
    assert!(execute_prompt.contains("- Q: which crate version?"));
    assert!(execute_prompt.contains("A: 1.0"));

    let mut plan_task = crate::protocol::tests::sample_task();
    plan_task.kind = task_core::TaskKind::Plan;
    let plan_prompt = build_prompt(&plan_task, &context, "run-a2", "artifacts");
    assert!(plan_prompt.contains("## Answers from a human to your earlier questions"));
    assert!(plan_prompt.contains("- Q: which crate version?"));

    // No answers: the section must not appear at all.
    let no_answers_prompt =
        build_prompt(&execute_task, &RunContext::default(), "run-a3", "artifacts");
    assert!(!no_answers_prompt.contains("Answers from a human"));
}

#[test]
fn build_prompt_for_plan_kind_includes_schema_and_plan_json_instructions() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Plan;
    let mut context = RunContext::default();
    context.prior_review.push(crate::protocol::PriorReview {
        criterion: 0,
        pass: false,
        reason: "tasks[2].depends_on[0] = 7 is out of range".into(),
    });
    let prompt = build_prompt(&task, &context, "run-plan-1", "artifacts");
    assert!(prompt.contains("artifacts/plan.json"));
    assert!(prompt.contains("\"tasks\""));
    assert!(prompt.contains("depends_on"));
    assert!(prompt.contains(&task_core::MAX_PLAN_DEPTH.to_string()));
    assert!(prompt.contains("PlanOutput") || prompt.contains("NewTask"));
    assert!(prompt.contains("tasks[2].depends_on[0] = 7 is out of range"));
    assert!(prompt.contains("artifacts/result.json"));
}

#[test]
fn build_prompt_for_review_kind_includes_review_json_and_context() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Review;
    let context = RunContext {
        review: Some(crate::protocol::ReviewRequest {
            summary: "added usage example".into(),
            evidence: vec![crate::protocol::Evidence {
                criterion: 0,
                command: Some("cargo test".into()),
                exit: Some(0),
                stdout_tail: Some("test result: ok".into()),
            }],
            criteria: vec![0],
            ..Default::default()
        }),
        inputs: vec![ArtifactRef {
            name: "readme.diff".into(),
            path: "artifacts/readme.diff".into(),
            sha256: "deadbeef".into(),
            kind: "diff".into(),
            declared: true,
        }],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-review-1", "artifacts");
    assert!(prompt.contains("artifacts/review.json"));
    assert!(prompt.contains("criterion 0"));
    assert!(prompt.contains("added usage example"));
    assert!(prompt.contains("cargo test"));
    assert!(prompt.contains("artifacts/readme.diff"));
    assert!(prompt.contains("read-only"));

    // context.review = None must not panic and still produces a usable prompt.
    let none_context = RunContext::default();
    let prompt_none = build_prompt(&task, &none_context, "run-review-2", "artifacts");
    assert!(prompt_none.contains("no review context"));
    assert!(prompt_none.contains("artifacts/review.json"));
}

#[test]
fn review_prompt_includes_human_decisions_and_check_results() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Review;
    let empty = build_prompt(
        &task,
        &RunContext::default(),
        "run-review-empty",
        "artifacts",
    );
    assert!(!empty.contains("## Human decisions and answers (authoritative)"));
    assert!(!empty.contains("## Deterministic checks already executed by celeris"));

    let context = RunContext {
        review: Some(crate::protocol::ReviewRequest {
            decisions: vec![crate::protocol::ReviewDecision {
                task_id: task.id,
                key: "adr-place".into(),
                question: "Where should the ADR go?".into(),
                option: "docs".into(),
                option_label: "Place in docs/adr".into(),
                note: Some("Include the new ADR in the scope".into()),
            }],
            answers: vec![crate::protocol::Answer {
                question: "Use the new location?".into(),
                answer: "Yes, use docs/adr".into(),
            }],
            checks: vec![crate::protocol::ReviewCheckResult {
                criterion: Some(0),
                kind: "command".into(),
                cmd: Some("cargo test -p task-worker".into()),
                pass: true,
                reason: "exit 0; test result: ok".into(),
            }],
            ..Default::default()
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-review-context", "artifacts");
    for expected in [
        "## Human decisions and answers (authoritative)",
        "adr-place",
        "Where should the ADR go?",
        "Place in docs/adr",
        "Include the new ADR in the scope",
        "Yes, use docs/adr",
        "expanded scope",
        "differs from the criterion's strict wording",
        "## Deterministic checks already executed by celeris (authoritative)",
        "cargo test -p task-worker",
        "pass=true",
        "exit 0; test result: ok",
        "run the command yourself now",
        "include your command and relevant output",
        "treat the passing check as authoritative",
    ] {
        assert!(prompt.contains(expected), "missing {expected:?}: {prompt}");
    }

    let context = RunContext {
        review: Some(crate::protocol::ReviewRequest::default()),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-review-empty", "artifacts");
    assert!(!prompt.contains("## Human decisions and answers (authoritative)"));
    assert!(!prompt.contains("## Deterministic checks already executed by celeris"));
}

/// ADR-0048 D2（Phase 60a）: stream-json の実物に近い標本（`tests/fixtures/claude-code-stream.jsonl`）を
/// 1 行ずつ `handle_line` に通し、`tool_use` / `tool_result` / `text` / `thinking` の写像を確かめる。
/// 外部ネットワークには出ない（ファイルを読むだけ）。
#[test]
fn stream_json_maps_to_structured_progress() {
    use task_core::ProgressKind;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude-code-stream.jsonl"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let sink = RecordingSink::default();
    let mut last_result = None;
    let mut background = BackgroundTasks::default();
    let mut exploration = ExplorationTracker::default();
    for line in text.lines() {
        handle_line(
            line,
            &sink,
            &mut last_result,
            &mut background,
            &mut exploration,
        );
    }
    let items = sink
        .structured
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let kinds: Vec<Option<ProgressKind>> = items.iter().map(|(_, f)| f.kind).collect();
    assert_eq!(
        kinds,
        vec![
            Some(ProgressKind::Thinking),
            Some(ProgressKind::Text),
            Some(ProgressKind::ToolUse),
            Some(ProgressKind::ToolResult),
            Some(ProgressKind::ToolUse),
            Some(ProgressKind::ToolResult),
            Some(ProgressKind::ToolUse),
            Some(ProgressKind::ToolResult),
            Some(ProgressKind::ToolUse),
        ],
        "{items:#?}"
    );

    // thinking は要約だけ（本文は流さない）。
    assert_eq!(
        items[0].1.summary.as_deref(),
        Some("まず現状のテストを確かめる それから直す")
    );
    assert!(items[0].1.detail.is_none());
    // 1 つの assistant メッセージの本文は 1 件にまとまる。
    assert_eq!(
        items[1].1.summary.as_deref(),
        Some("まずテストを回します。 結果を見てから直します。")
    );
    assert_eq!(
        items[1].0,
        "まずテストを回します。\n結果を見てから直します。"
    );
    // Bash は入力のコマンドが 1 行要約、`detail` は入力そのもの。
    assert_eq!(items[2].1.tool.as_deref(), Some("Bash"));
    assert_eq!(
        items[2].1.summary.as_deref(),
        Some("cargo test --workspace")
    );
    assert!(
        items[2]
            .1
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("run the tests")
    );
    assert!(items[2].0.starts_with("tool_use: Bash"), "{}", items[2].0);
    // tool_result は先頭 200 文字の要約と失敗の印。
    assert_eq!(
        items[3].1.summary.as_deref(),
        Some("test result: ok. 812 passed; 0 failed")
    );
    assert!(!items[3].1.error);
    // Read はパス、Grep は模様。配列の `content` も読める。
    assert_eq!(
        items[4].1.summary.as_deref(),
        Some("/repo/crates/task-api/src/console.rs")
    );
    assert_eq!(
        items[5].1.summary.as_deref(),
        Some("//! Console の読み取り側")
    );
    assert_eq!(items[6].1.summary.as_deref(), Some("fn console"));
    // `is_error` は `error` に写る。
    assert!(items[7].1.error, "{:?}", items[7]);
    assert_eq!(items[7].1.summary.as_deref(), Some("No files found"));
    assert!(items[7].0.contains("(error)"));
    // 知らない道具は入力そのものの先頭（120 文字）。
    assert_eq!(items[8].1.tool.as_deref(), Some("WebFetch"));
    assert!(
        items[8]
            .1
            .summary
            .as_deref()
            .unwrap_or("")
            .contains("example.invalid")
    );
    // `result` は進行ではない（終端の合成に使う）。
    assert!(last_result.is_some());
}

#[tokio::test]
async fn browser_cli_result_errors_are_redacted_before_normalized_result_write() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = ClaudeCodeAdapter::new(stub_claude(
        dir.path(),
        r#"echo '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"401 Unauthorized rpc-secret-sentinel"}'
"#,
    ));
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.browser = Some(crate::browser::BrowserContext {
        credential_used: false,
        run: task_core::BrowserRun {
            task_id: req.task.id,
            run_id: "browser-error-test".into(),
            session_id: "isolated-test".into(),
            state: task_core::BrowserRunState::Running,
            live_view_url: None,
            policy: None,
        },
        cli: dir.path().join("celeris-browser.py"),
    });
    let sink = RecordingSink::default();
    let result = adapter
        .run(req, "browser-error-test", default_limits(), &sink)
        .await;
    let error = result.unwrap_err();
    assert!(!error.to_string().contains("rpc-secret-sentinel"));
    assert!(matches!(error, AdapterError::AuthFailed(_)));
    let run = dir.path().join("runs/browser-error-test");
    for entry in std::fs::read_dir(run).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(path).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("rpc-secret-sentinel"));
        }
    }
}

#[tokio::test]
async fn browser_run_discards_raw_logs_but_still_parses_completion() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
echo 'raw-browser-secret-sentinel'
echo 'raw-browser-secret-sentinel' >&2
printf '%s' '{"summary":"safe browser result","evidence":[]}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.browser = Some(crate::browser::BrowserContext {
        credential_used: false,
        run: task_core::BrowserRun {
            task_id: req.task.id,
            run_id: "browser-log-test".into(),
            session_id: "isolated-test".into(),
            state: task_core::BrowserRunState::Running,
            live_view_url: None,
            policy: None,
        },
        cli: dir.path().join("celeris-browser.py"),
    });
    let sink = RecordingSink::default();
    let result = adapter
        .run(req, "browser-log-test", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(result.terminal, Terminal::Done { .. }));
    let run = dir.path().join("runs/browser-log-test");
    assert!(!run.join("stdout.jsonl").exists());
    assert!(!run.join("stderr.log").exists());
    for entry in std::fs::read_dir(run).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(path).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("raw-browser-secret-sentinel"));
        }
    }
}

#[tokio::test]
async fn happy_path_progress_and_done_from_result_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"working on it"}]}}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}'
printf '%s' '{"summary":"added usage example","evidence":[]}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":10,"output_tokens":20}}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-1", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done {
            summary,
            evidence,
            usage,
        } => {
            assert_eq!(summary, "added usage example");
            assert!(evidence.is_empty());
            assert_eq!(
                usage,
                Some(Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(20),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: None,
                    duplicate_reads: Some(0),
                    session_resumed: Some(false),
                })
            );
        }
        other => panic!("expected done, got {other:?}"),
    }
    let progress = sink.progress.lock().unwrap();
    assert!(progress.iter().any(|m| m == "working on it"));
    assert!(progress.iter().any(|m| m.starts_with("tool_use: Bash")));
    assert!(dir.path().join("runs/run-1/stdout.jsonl").is_file());

    // P-26 (ADR-0010 D10): the terminal is also normalized into `runs/<run_id>/result.json`,
    // readable by task-dispatch as a `WorkerMessage::Done`.
    let result_json = std::fs::read_to_string(dir.path().join("runs/run-1/result.json")).unwrap();
    match serde_json::from_str::<crate::protocol::WorkerMessage>(result_json.trim()).unwrap() {
        crate::protocol::WorkerMessage::Done { summary, .. } => {
            assert_eq!(summary, "added usage example")
        }
        other => panic!("expected done in result.json, got {other:?}"),
    }
}

/// ADR-0056 D3（Phase 79）: `context.skills` に乗った skill は、run 開始時に
/// `.claude/skills/<name>/SKILL.md`（＋付属ファイル）として作業場所に写る。
#[tokio::test]
async fn mounted_skills_are_copied_into_dot_claude_skills() {
    let dir = tempfile::tempdir().unwrap();
    let kb = tempfile::tempdir().unwrap();
    let skill_dir = kb.path().join("rust-review");
    std::fs::create_dir_all(skill_dir.join("refs")).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: rust-review\ndescription: d\n---\n\nbody\n",
    )
    .unwrap();
    std::fs::write(skill_dir.join("refs/checklist.md"), "1. fmt\n").unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.skills = vec![crate::protocol::SkillMount {
        name: "rust-review".into(),
        path: skill_dir.display().to_string(),
        description: "d".into(),
    }];
    let sink = RecordingSink::default();
    adapter
        .run(req, "run-skills", default_limits(), &sink)
        .await
        .unwrap();
    let delivered = dir.path().join(".claude/skills/rust-review");
    assert!(
        std::fs::read_to_string(delivered.join("SKILL.md"))
            .unwrap()
            .contains("body")
    );
    assert_eq!(
        std::fs::read_to_string(delivered.join("refs/checklist.md")).unwrap(),
        "1. fmt\n"
    );
}

/// ADR-0036 D1/D2/D3: 共有 workspace のタスクは `.taskd/artifacts/<task_id>/result.json` を読み書きし、
/// プロンプトにもその相対パスが出る。隣（兄弟）が共有 `artifacts/` に置いた結果ファイルは読まない。
#[tokio::test]
async fn a_shared_workspace_task_uses_its_own_artifacts_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p .taskd/artifacts/T1
printf '%s' '{"summary":"mine","evidence":[]}' > .taskd/artifacts/T1/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    std::fs::create_dir_all(dir.path().join("artifacts")).unwrap();
    std::fs::write(
        dir.path().join("artifacts/result.json"),
        r#"{"summary":"sibling"}"#,
    )
    .unwrap();
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.artifacts_dir = dir.path().join(".taskd/artifacts/T1");
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-shared", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "mine"),
        other => panic!("expected done, got {other:?}"),
    }
    let prompt = std::fs::read_to_string(dir.path().join("runs/run-shared/prompt.txt")).unwrap();
    assert!(
        prompt.contains(".taskd/artifacts/T1/result.json"),
        "{prompt}"
    );
    assert!(!prompt.contains("`artifacts/result.json`"), "{prompt}");
    // 兄弟のファイルは消していない（自分のディレクトリだけを掃除する）。
    assert_eq!(
        std::fs::read_to_string(dir.path().join("artifacts/result.json")).unwrap(),
        r#"{"summary":"sibling"}"#
    );
}

/// ADR-0006 Phase 115 D1（本番障害 01M3915FARENW8M0JM11XVF6W0）: `work_dir != workspace`
/// （部署のリポジトリの git worktree で走るタスク）だけ、プロンプト冒頭に cwd と成果物ディレクトリの
/// 絶対パスの注意が 2 行出る（D4(a)）。`work_dir` が無い他の全テストの文面は変わらない。
#[tokio::test]
async fn work_dir_note_appears_in_the_prompt_when_work_dir_differs_from_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("repos/agent-platform");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let mut req = sample_req(dir.path().to_path_buf());
    req.work_dir = Some(work_dir.clone());
    let sink = RecordingSink::default();
    let adapter = ClaudeCodeAdapter::new(config);
    let outcome = adapter
        .run(req, "run-wd-1", default_limits(), &sink)
        .await
        .unwrap();
    assert!(
        matches!(outcome.terminal, Terminal::Done { .. }),
        "{:?}",
        outcome.terminal
    );
    let prompt = std::fs::read_to_string(dir.path().join("runs/run-wd-1/prompt.txt")).unwrap();
    assert!(
        prompt.contains(&format!("cwd は `{}`", work_dir.display())),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!(
            "成果物ディレクトリは `{}`",
            dir.path().join("artifacts").display()
        )),
        "{prompt}"
    );
    assert!(
        prompt.contains("相対 `artifacts/` はリポジトリの中を指すので使わない"),
        "{prompt}"
    );
}

/// ADR-0006 Phase 115 D2（本番障害 01M3915FARENW8M0JM11XVF6W0 / 01M38T8N17MEWPTJQXGX1TNYJD）:
/// 偽 claude スタブが（指示を読み違えて）cwd 相対の `artifacts/result.json`（=
/// `<work_dir>/artifacts/result.json`）に書いても、正しい置き場（`<artifacts_dir>/result.json`）へ
/// 移して採用し `Done` になる。worktree 側には残らない（D4(b)）。
#[tokio::test]
async fn a_result_json_written_under_work_dir_is_adopted_and_not_left_behind() {
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("repos/agent-platform");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"wrote to the worktree by mistake","evidence":[]}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let mut req = sample_req(dir.path().to_path_buf());
    req.work_dir = Some(work_dir.clone());
    let sink = RecordingSink::default();
    let adapter = ClaudeCodeAdapter::new(config);
    let outcome = adapter
        .run(req, "run-wd-2", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => {
            assert_eq!(summary, "wrote to the worktree by mistake")
        }
        other => panic!("expected done, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("artifacts/result.json")).unwrap(),
        r#"{"summary":"wrote to the worktree by mistake","evidence":[]}"#
    );
    assert!(
        !work_dir.join("artifacts").exists(),
        "the stray artifacts/ dir under work_dir should be gone"
    );
}

/// ADR-0072 E2（P-E0-2 の修正）: `result` メッセージは観測できたのに `result.json` が無いのは
/// `Err(AdapterError::Other)`（`ProviderFailure` 無し）になる。従来は `Ok(Terminal::Error{retryable:
/// true})` になり、`WorkerError{true}` として attempts を消費していた（ADR-0070 D3 の想定と
/// 食い違っていた）。この `Err` はディスパッチャの `provider_failure_outcome` が分類できない
/// 失敗として扱い、`InfraRequeue`（attempts を消費しない）に倒す。
#[tokio::test]
async fn success_without_result_file_is_an_infra_failure_not_a_retryable_worker_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"echo '{"type":"result","subtype":"success","is_error":false}'"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = adapter
        .run(req, "run-2", default_limits(), &sink)
        .await
        .unwrap_err();
    match err {
        AdapterError::Other(message) => {
            assert!(message.contains("artifacts/result.json"), "{message}");
        }
        other => panic!("expected AdapterError::Other, got {other:?}"),
    }
    // `provider_failure_reason`/`provider_failure_outcome`（task-dispatch）はこれを分類できない
    // 失敗として扱う（`None`）。task-worker からは直接呼べないので、`AdapterError::Other` である
    // ことの確認をもって代える（`dispatcher.rs` 側の網羅テストが `None` → `InfraRequeue` を見る）。
}

#[tokio::test]
async fn question_in_result_file_blocks_task() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"question":"which crate version?"}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-3", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Question { text } => assert_eq!(text, "which crate version?"),
        other => panic!("expected question, got {other:?}"),
    }
}

/// ADR-0072 D7（Phase E1）: `error_max_turns` は `result.json` が書けていても
/// `Terminal::BudgetExhausted{kind: Turns}` になる（result.json より優先。継続の対象で、
/// `Error` ではない）。usage があれば運ぶ（従来は捨てていた）。
#[tokio::test]
async fn error_max_turns_subtype_wins_and_becomes_budget_exhausted_with_usage() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"claimed done","evidence":[]}' > artifacts/result.json
echo '{"type":"result","subtype":"error_max_turns","is_error":true,"usage":{"input_tokens":100,"output_tokens":50}}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-4", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::BudgetExhausted {
            kind,
            message,
            usage,
        } => {
            assert_eq!(kind, task_core::BudgetKind::Turns);
            assert!(message.contains("error_max_turns"), "{message}");
            let usage = usage.expect("usage carried through");
            assert_eq!(usage.input_tokens, Some(100));
            assert_eq!(usage.output_tokens, Some(50));
        }
        other => panic!("expected budget_exhausted, got {other:?}"),
    }
}

/// ADR-0072 D9（Phase E1）: `result.json` の `{"yield": {...}}` が `Terminal::Yielded` になる。
#[tokio::test]
async fn result_yield_becomes_terminal_yielded() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"yield":{"completed":["A"],"remaining":["B"],"next_action":"do B"}}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":10,"output_tokens":20}}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-yield", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Yielded { checkpoint, usage } => {
            assert_eq!(checkpoint["next_action"], "do B");
            let usage = usage.expect("usage carried through");
            assert_eq!(usage.input_tokens, Some(10));
        }
        other => panic!("expected yielded, got {other:?}"),
    }
}

/// ADR-0090 D1: `result.json` の `{"type": "wait", "kind": "cluster_job", ...}` が `Terminal::Waiting` になり、
/// 生の JSONL の `result.json`（run ディレクトリ）にも `{"type":"wait",...}` の 1 行が残る。不正な wait は
/// `Error{retryable: true}`、`question` は wait より優先する。
#[tokio::test]
async fn result_wait_becomes_terminal_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"type":"wait","kind":"cluster_job","cluster":"sirius","jobs":["42634","42635"],"scheduler":"pbs","poll_secs":300,"timeout_secs":43200,"checkpoint":{"completed":["submitted"],"next_action":"collect"},"summary":"submitted 2 jobs"}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":10,"output_tokens":20}}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-wait", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Waiting {
            request,
            checkpoint,
            usage,
        } => {
            assert_eq!(request.cluster.as_deref(), Some("sirius"));
            assert_eq!(request.jobs, vec!["42634".to_string(), "42635".to_string()]);
            assert_eq!(
                request.scheduler,
                task_core::cluster_job::ClusterScheduler::Pbs
            );
            assert_eq!(request.poll_secs, Some(300));
            assert_eq!(request.timeout_secs, Some(43200));
            assert_eq!(request.summary, "submitted 2 jobs");
            assert_eq!(checkpoint.expect("checkpoint")["next_action"], "collect");
            assert_eq!(usage.expect("usage").input_tokens, Some(10));
        }
        other => panic!("expected waiting, got {other:?}"),
    }
    let raw = std::fs::read_to_string(dir.path().join("runs/run-wait/result.json")).unwrap();
    let line: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(line["type"], "wait");
    assert_eq!(line["jobs"][1], "42635");

    // 不正な wait（job id にシェルの文字）は retryable な error。
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"type":"wait","kind":"cluster_job","jobs":["1;rm"]}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let outcome = adapter
        .run(
            sample_req(dir.path().to_path_buf()),
            "run-bad-wait",
            default_limits(),
            &sink,
        )
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { message, retryable } => {
            assert!(retryable);
            assert!(message.contains("invalid cluster job wait"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// `result.is_error`（or `subtype != "success"`) のとき `result` テキストを分類する（ADR-0010 D5）。
/// `Throttled` が当たれば `AdapterError::Throttled` として返り、result.json は書かれる。
#[tokio::test]
async fn result_text_classified_as_throttled_surfaces_as_adapter_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"echo '{"type":"result","subtype":"success","is_error":true,"result":"API Error: 429 rate limit exceeded"}'"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = adapter
        .run(req, "run-4b", default_limits(), &sink)
        .await
        .expect_err("expected a provider failure");
    assert!(matches!(err, AdapterError::Throttled { .. }), "{err:?}");
    assert!(dir.path().join("runs/run-4b/result.json").is_file());
}

/// 同じく `AuthFailed` の分類（ADR-0010 D5）。
#[tokio::test]
async fn result_text_classified_as_auth_failed_surfaces_as_adapter_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"echo '{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"}'"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = adapter
        .run(req, "run-4c", default_limits(), &sink)
        .await
        .expect_err("expected a provider failure");
    assert!(matches!(err, AdapterError::AuthFailed(_)), "{err:?}");
    assert!(dir.path().join("runs/run-4c/result.json").is_file());
}

/// `result` メッセージを一度も観測できずに exit した場合も、stderr の末尾を分類する（ADR-0010 D5）。
#[tokio::test]
async fn crash_with_matching_stderr_is_classified_as_provider_failure() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), "echo 'fatal: 401 Unauthorized' 1>&2; exit 9");
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = adapter
        .run(req, "run-4d", default_limits(), &sink)
        .await
        .expect_err("expected a provider failure");
    assert!(matches!(err, AdapterError::AuthFailed(_)), "{err:?}");
}

#[tokio::test]
async fn invalid_result_file_json_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf 'not json' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-5", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("not valid JSON"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// ADR-0072 D7（Phase E1）: wall-clock の打ち切りは `Terminal::BudgetExhausted{kind: WallClock}`
/// になる（continuation の対象。従来の `Error` ではない）。
#[tokio::test]
async fn wall_clock_exceeded_kills_and_reports_budget_exhausted() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), "sleep 30");
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: Duration::from_millis(300),
        idle_timeout: Duration::from_secs(30),
        kill_grace: Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = adapter.run(req, "run-6", limits, &sink).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    match outcome.terminal {
        Terminal::BudgetExhausted {
            kind,
            message,
            usage,
        } => {
            assert_eq!(kind, task_core::BudgetKind::WallClock);
            assert!(message.contains("wall clock exceeded"), "{message}");
            assert!(usage.is_none(), "wall-clock 打ち切りでは usage は取れない");
        }
        other => panic!("expected budget_exhausted, got {other:?}"),
    }
}

#[tokio::test]
async fn idle_timeout_kills_and_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"echo '{"type":"assistant","message":{"content":[{"type":"text","text":"start"}]}}'
sleep 30
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_millis(300),
        kill_grace: Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = adapter.run(req, "run-7", limits, &sink).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("idle timeout"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

#[tokio::test]
async fn crash_without_result_message_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), "exit 9");
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-8", default_limits(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.exit_code, Some(9));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("exit=9"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// クラッシュ前に（あるいは前回の run の名残として）`artifacts/result.json` が存在していても、
/// `result` メッセージを一度も観測できなければ絶対に信用しない（監査で発見した不具合の回帰テスト。
/// ADR-0006 D4）。
#[tokio::test]
async fn stale_result_file_without_result_message_is_not_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"looks done but crashed before saying so","evidence":[]}' > artifacts/result.json
exit 9
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-9", default_limits(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.exit_code, Some(9));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("exit=9"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// 前回の run が残した `artifacts/result.json` は、今回の run 開始時に消される
/// （監査で発見した不具合の回帰テスト。ADR-0006 D3）。
#[tokio::test]
async fn stale_result_file_from_previous_run_is_cleared_before_this_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("artifacts")).unwrap();
    std::fs::write(
        dir.path().join("artifacts/result.json"),
        r#"{"summary":"stale from a previous attempt","evidence":[]}"#,
    )
    .unwrap();
    let config = stub_claude(
        dir.path(),
        r#"echo '{"type":"result","subtype":"success","is_error":false}'"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    // ADR-0072 E2（P-E0-2 の修正）: `result` は観測できたが `result.json` が無い（= 消された
    // stale file が再利用されていない証拠）ので、いまは `Err(AdapterError::Other)` になる
    // （InfraRequeue。attempts を消費しない）。
    let err = adapter
        .run(req, "run-10", default_limits(), &sink)
        .await
        .unwrap_err();
    match err {
        AdapterError::Other(message) => {
            assert!(message.contains("artifacts/result.json"), "{message}");
        }
        other => {
            panic!("expected error (stale file must be cleared, not reused), got {other:?}")
        }
    }
}

/// Phase 5 ドッグフードの回帰: `evidence` が文字列の配列など不正な形でも、`summary` があれば `done`
/// として扱い、読めない要素は捨てる（ADR-0006 D3）。
#[tokio::test]
async fn malformed_evidence_in_result_file_does_not_fail_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"all good","evidence":["cargo test: 4 passed",{"criterion":0,"command":"cargo test","exit":0,"stdout_tail":""},42]}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-11", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done {
            summary, evidence, ..
        } => {
            assert_eq!(summary, "all good");
            assert_eq!(evidence.len(), 1);
            assert_eq!(evidence[0].command.as_deref(), Some("cargo test"));
        }
        other => panic!("expected done, got {other:?}"),
    }
    let prompt = build_prompt(
        &crate::protocol::tests::sample_task(),
        &RunContext::default(),
        "r",
        "artifacts",
    );
    assert!(prompt.contains("plain strings are not accepted"));
}

/// ADR-0033 D4 / D6（Phase 24）: 前置き（役職と brief・記憶・直近のやり取り）がプロンプトに入り、
/// 並びは `## Task` の直後・`## Objective` の前。`RunContext` が Phase 23 までの中身なら出力は変わらない。
#[test]
fn build_prompt_puts_the_person_preamble_between_the_run_line_and_the_objective() {
    let task = crate::protocol::tests::sample_task();
    let bare = build_prompt(&task, &RunContext::default(), "run-p0", "artifacts");

    let context = RunContext {
        node: Some(crate::protocol::NodeContext {
            id: "research-survey".into(),
            name: "関連研究調査課".into(),
            brief: "関連研究を洗う。".into(),
        }),
        memory: Some(crate::protocol::MemoryContext {
            notes: "- 2026-09-10: pegasus は pjsub".into(),
            project: String::new(),
        }),
        conversation: vec![crate::protocol::ConversationTurn {
            role: task_core::MessageRole::User,
            text: "先週の続き".into(),
        }],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-p1", "artifacts");
    let at = |n: &str| {
        prompt
            .find(n)
            .unwrap_or_else(|| panic!("missing {n:?} in\n{prompt}"))
    };
    assert!(at("(run run-p1") < at("## あなた: 関連研究調査課 (research-survey)"));
    assert!(at("## あなた:") < at("## 覚えていること"));
    assert!(at("## 覚えていること") < at("## 直近のやり取り"));
    assert!(at("## 直近のやり取り") < at("## Objective"));
    assert!(prompt.contains("memory.notes"), "記憶の書き方の指示が付く");

    // Phase 23 までの `RunContext` では 1 バイトも変わらない。
    assert!(!bare.contains("## あなた"));
    assert!(!bare.contains("覚えておくこと"));
    assert_eq!(
        bare,
        build_prompt(&task, &RunContext::default(), "run-p0", "artifacts")
    );
}

/// ADR-0033 D4（Phase 24）: 組織図を渡した run には `## 組織図` と `assignee` の指示が入る。
/// 渡していない run（Phase 23 までの構成）では出ない。
#[test]
fn build_prompt_includes_the_org_chart_and_the_assignee_instruction_only_when_present() {
    let mut task = crate::protocol::tests::sample_task();
    let org = vec![
        crate::protocol::OrgNodeContext {
            harnesses: Vec::new(),
            skills: Vec::new(),
            tools: Vec::new(),
            id: "research".into(),
            name: "研究部".into(),
            kind: task_core::OrgKind::Department,
            parent_id: Some("secretary".into()),
            brief: "課に振り分ける".into(),
            genre: None,
        },
        crate::protocol::OrgNodeContext {
            harnesses: Vec::new(),
            skills: Vec::new(),
            tools: Vec::new(),
            id: "research-survey".into(),
            name: "関連研究調査課".into(),
            kind: task_core::OrgKind::Section,
            parent_id: Some("research".into()),
            brief: "関連研究を洗う".into(),
            genre: Some("literature".into()),
        },
    ];
    let context = RunContext {
        organization: org,
        ..RunContext::default()
    };

    let execute = build_prompt(&task, &context, "run-o1", "artifacts");
    assert!(execute.contains("## 組織図 (who you can assign work to)"));
    assert!(execute.contains(
        "- research-survey [課] 関連研究調査課 (親: research, 分野: literature) — 関連研究を洗う"
    ));
    // ADR-0069 D1（Phase 114）: 委譲でも担当とモデルは選ばせない（書いても使われない）。
    assert!(execute.contains("**担当（`assignee`）と"), "{execute}");
    assert!(!execute.contains("\"assignee\":\"<optional org node id>\""));

    task.kind = task_core::TaskKind::Plan;
    let plan = build_prompt(&task, &context, "run-o2", "artifacts");
    // ADR-0046 D5（Phase 59）: 計画は人選をしない。組織図も渡さない。
    assert!(
        plan.contains("**担当（`assignee`）とモデル（`tier`）は選ぶな。**"),
        "{plan}"
    );
    assert!(
        plan.contains("`harness`: その仕事の実行契約の id"),
        "{plan}"
    );
    assert!(plan.contains("`mode`: `prototype`"), "{plan}");
    assert!(
        !plan.contains("## 組織図 (who you can assign work to)"),
        "{plan}"
    );

    assert!(!build_prompt(&task, &RunContext::default(), "run-o3", "artifacts").contains("組織図"));
}

/// ADR-0016 D1 / M3: `context.role` があれば `## Role: <id>` と指示文がプロンプトに入る。無ければ入らない。
#[test]
fn build_prompt_includes_role_header_when_present_and_omits_it_when_absent() {
    let task = crate::protocol::tests::sample_task();
    let context = RunContext {
        role: Some(crate::protocol::RoleContext {
            id: "lead".into(),
            instructions: "You coordinate the work of others.".into(),
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-role-1", "artifacts");
    assert!(prompt.contains("## Role: lead"));
    assert!(prompt.contains("You coordinate the work of others."));

    let no_role_prompt = build_prompt(&task, &RunContext::default(), "run-role-2", "artifacts");
    assert!(!no_role_prompt.contains("## Role"));
}

/// ADR-0027 D1: `task.genre` があり、その分野が `context.available_genres` に載っていれば
/// `## Genre: <id>` と説明がプロンプトに入る。載っていなければ（委譲できない run など）出ない。
#[test]
fn build_prompt_includes_genre_header_only_when_the_genre_is_in_available_genres() {
    let mut task = crate::protocol::tests::sample_task();
    task.genre = Some("literature".into());
    let genre_spec = task_core::GenreSpec {
        id: "literature".into(),
        description: "related work survey and novelty checks".into(),
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-reader".into()],
        ..task_core::GenreSpec::default()
    };
    let context = RunContext {
        available_genres: vec![GenreContext::from(&genre_spec)],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-genre-1", "artifacts");
    assert!(prompt.contains("## Genre: literature"));
    assert!(prompt.contains("related work survey and novelty checks"));

    // available_genres が task.genre を含まない（あるいは空）なら Genre 見出しは出ない。
    let empty_prompt = build_prompt(&task, &RunContext::default(), "run-genre-2", "artifacts");
    assert!(!empty_prompt.contains("## Genre"));
}

/// ADR-0027 D1: `context.available_genres` が非空なら「使える専門家」節が Execute プロンプトに入り、
/// 空なら入らない。
#[test]
fn build_prompt_includes_available_genres_section_only_when_present() {
    let task = crate::protocol::tests::sample_task();
    let genre_spec = task_core::GenreSpec {
        id: "literature".into(),
        description: "related work survey".into(),
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-scout".into(), "literature-reader".into()],
        ..task_core::GenreSpec::default()
    };
    let context = RunContext {
        available_genres: vec![GenreContext::from(&genre_spec)],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-avail-1", "artifacts");
    assert!(prompt.contains("使える専門家"));
    assert!(prompt.contains("literature-scout"));
    assert!(prompt.contains("literature-reader"));

    let no_genres_prompt = build_prompt(&task, &RunContext::default(), "run-avail-2", "artifacts");
    assert!(!no_genres_prompt.contains("使える専門家"));
}

/// ADR-0028 D2: `capabilities` / `input_artifacts` / `output_artifacts` があれば「できること」と
/// 「渡すもの…返るもの」の行が、ADR に書かれた通りの形で出る。
#[test]
fn available_genres_section_renders_the_adr_0028_d2_shape() {
    let task = crate::protocol::tests::sample_task();
    let genre_spec = task_core::GenreSpec {
        id: "related-research".into(),
        description: "先行研究の確認・新規性の検討".into(),
        capabilities: vec![
            "学術文献の検索".into(),
            "引用グラフの探索".into(),
            "PDF 全文からの根拠抽出".into(),
        ],
        input_artifacts: vec!["question".into(), "pdf".into(), "bibliography".into()],
        output_artifacts: vec!["answer.md".into(), "citations.json".into()],
        default_role: Some("literature-reader".into()),
        roles: vec![
            "literature-scout".into(),
            "literature-reader".into(),
            "novelty-skeptic".into(),
        ],
    };
    let context = RunContext {
        available_genres: vec![GenreContext::from(&genre_spec)],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-avail-shape", "artifacts");
    assert!(
        prompt.contains(
            "- related-research: 先行研究の確認・新規性の検討\n\
                 \u{20}\u{20}できること: 学術文献の検索 / 引用グラフの探索 / PDF 全文からの根拠抽出\n\
                 \u{20}\u{20}渡すもの: question, pdf, bibliography → 返るもの: answer.md, citations.json\n\
                 \u{20}\u{20}役割: literature-scout, literature-reader, novelty-skeptic\n"
        ),
        "{prompt}"
    );
}

/// ADR-0028 D1: `capabilities` / `input_artifacts` / `output_artifacts` が空なら、それぞれの行を
/// 出さない（既存設定との互換）。
#[test]
fn available_genres_section_omits_lines_whose_list_is_empty() {
    let task = crate::protocol::tests::sample_task();
    let genre_spec = task_core::GenreSpec {
        id: "coding".into(),
        description: "write and fix code".into(),
        roles: vec!["implementer".into()],
        ..task_core::GenreSpec::default()
    };
    let context = RunContext {
        available_genres: vec![GenreContext::from(&genre_spec)],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-avail-omit", "artifacts");
    assert!(!prompt.contains("できること"));
    assert!(!prompt.contains("渡すもの"));
    assert!(prompt.contains("- coding: write and fix code\n  役割: implementer\n"));
}

/// ADR-0028 D3: Plan run のプロンプトにも「使える専門家」節が入る（今までは Execute/Approval だけ）。
#[test]
fn build_plan_prompt_includes_available_genres_section_when_present() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Plan;
    let genre_spec = task_core::GenreSpec {
        id: "literature".into(),
        description: "related work survey".into(),
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-reader".into()],
        ..task_core::GenreSpec::default()
    };
    let context = RunContext {
        available_genres: vec![GenreContext::from(&genre_spec)],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-plan-avail-1", "artifacts");
    assert!(prompt.contains("使える専門家"));
    assert!(prompt.contains("literature-reader"));
    assert!(prompt.contains("artifacts/plan.json"));

    let no_genres_prompt = build_prompt(
        &task,
        &RunContext::default(),
        "run-plan-avail-2",
        "artifacts",
    );
    assert!(!no_genres_prompt.contains("使える専門家"));
}

/// ADR-0074 D3.3（Phase F4a (b)）: `MILESTONES_PLAN_LABEL` の印がある Plan タスクは
/// `project-plan.json`（`celeris.project-plan/1`）用のプロンプトを選ぶ（`plan.json` の分解プロンプト
/// とは別物）。
#[test]
fn build_prompt_selects_the_project_plan_prompt_for_a_labelled_plan_task() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Plan;
    task.labels = vec![task_core::MILESTONES_PLAN_LABEL.to_string()];
    let prompt = build_prompt(
        &task,
        &RunContext::default(),
        "run-project-plan-1",
        "artifacts",
    );
    assert!(prompt.contains("artifacts/project-plan.json"), "{prompt}");
    assert!(prompt.contains("celeris.project-plan/1"), "{prompt}");
    assert!(prompt.contains("reach_criteria"), "{prompt}");
    assert!(!prompt.contains("artifacts/plan.json"), "{prompt}");

    // ラベルの無い Plan タスクは従来どおり `plan.json` の分解プロンプト。
    let mut unlabeled = task.clone();
    unlabeled.labels = Vec::new();
    let plain = build_prompt(
        &unlabeled,
        &RunContext::default(),
        "run-plan-plain",
        "artifacts",
    );
    assert!(plain.contains("artifacts/plan.json"), "{plain}");
    assert!(!plain.contains("project-plan.json"), "{plain}");
}

/// 案件計画のプロンプトにも「使える専門家」節が入るが、milestone は `role` を持たないので
/// `role` の指示は出さない。
/// ADR-0074 D3.4（Phase F4b (e)）: replan の印がある案件計画の Plan タスクは差分のプロンプトになる。
#[test]
fn project_replan_task_gets_the_delta_prompt() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Plan;
    task.labels = vec![
        task_core::MILESTONES_PLAN_LABEL.to_string(),
        task_core::MILESTONES_REPLAN_LABEL.to_string(),
    ];
    let prompt = build_prompt(&task, &RunContext::default(), "run-1", "/tmp/a");
    assert!(prompt.contains("celeris.project-plan-delta/1"), "{prompt}");
    assert!(prompt.contains("cancel"));
    assert!(!prompt.contains("\"schema\":\"celeris.project-plan/1\""));
    task.labels = vec![task_core::MILESTONES_PLAN_LABEL.to_string()];
    let prompt = build_prompt(&task, &RunContext::default(), "run-1", "/tmp/a");
    assert!(!prompt.contains("celeris.project-plan-delta/1"));
}

#[test]
fn build_project_plan_prompt_lists_genres_without_role_instructions() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Plan;
    task.labels = vec![task_core::MILESTONES_PLAN_LABEL.to_string()];
    let genre_spec = task_core::GenreSpec {
        id: "literature".into(),
        description: "related work survey".into(),
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-reader".into()],
        ..task_core::GenreSpec::default()
    };
    let context = RunContext {
        available_genres: vec![GenreContext::from(&genre_spec)],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-project-plan-genres", "artifacts");
    assert!(prompt.contains("使える専門家"));
    assert!(prompt.contains("literature"));
    assert!(prompt.contains("Do not set `role`"), "{prompt}");
}

/// ADR-0072 D14（Phase E3）: `context.execution_planner` があれば、`task.kind` に関わらず
/// planner 用プロンプトを選ぶ（compound と判定された Task は常に `kind == Execute` のまま）。
/// 計画の schema・gate の根拠・上限・使える genre の一覧が載る。
#[test]
fn build_prompt_selects_the_execution_plan_prompt_when_execution_planner_is_present() {
    let task = crate::protocol::tests::sample_task();
    assert_eq!(task.kind, task_core::TaskKind::Execute);
    let genre_spec = task_core::GenreSpec {
        id: "coding".into(),
        description: "write and fix code".into(),
        roles: vec!["implementer".into()],
        ..task_core::GenreSpec::default()
    };
    let planner_ctx = crate::protocol::ExecutionPlannerContext {
        gate_rule_id: "compound/score".to_string(),
        gate_score: 6,
        gate_signals: vec!["F2: expected_length=high (+2)".to_string()],
        max_work_units: 5,
        work_unit_max_turns: 60,
        work_unit_max_wall_secs: 1800,
        default_max_turns: 30,
        default_max_wall_secs: 1800,
        replan: false,
        replan_reason: String::new(),
        current_plan_version: None,
        work_unit_summaries: Vec::new(),
        preserve_done_keys: Vec::new(),
        parallel: false,
        max_phases: 0,
        ..Default::default()
    };
    let context = RunContext {
        available_genres: vec![GenreContext::from(&genre_spec)],
        execution_planner: Some(planner_ctx),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-planner-1", "artifacts");

    // 通常の execute プロンプト（Objective の後の委譲節など）ではなく、計画作成の指示になる。
    assert!(prompt.contains("artifacts/execution-plan.json"));
    assert!(prompt.contains(task_core::EXECUTION_PLAN_SCHEMA));
    assert!(prompt.contains("\"work_units\""));
    // D18 の上限（テストの `ExecutionPlannerContext` の値）が文面に出る。
    assert!(prompt.contains("Plan at most 5 WorkUnits"));
    assert!(prompt.contains("capped at 60"));
    // gate の根拠。
    assert!(prompt.contains("compound/score"));
    assert!(prompt.contains("F2: expected_length=high (+2)"));
    // 使える genre の一覧（WU の harness に使える id）。
    assert!(prompt.contains("Available genres"));
    assert!(prompt.contains("coding: write and fix code"));
    // `assignee`/`tier`/`model`/`lane` を書くなと明示している。
    assert!(prompt.contains("Do not write `assignee`, `tier`, `model`, or `lane`"));
    // ADR-0072 D17（Phase E4b 項目1）: `replan = false`（初回 planning）には replan 節が出ない。
    assert!(!prompt.contains("REPLANNING an existing execution plan"));
    // ADR-0074 D5.1（Phase F1）: WU ごとの `features` の説明と例、5 軸の名前がすべて出る。
    assert!(prompt.contains("per-WorkUnit routing hints"));
    for axis in [
        "judgment",
        "ambiguity",
        "verifiability",
        "reversibility",
        "consequence",
    ] {
        assert!(prompt.contains(axis), "missing axis {axis} in prompt");
    }
    assert!(prompt.contains("\"features\""));
    assert!(prompt.contains("Do not write `lane`"));

    // `execution_planner` が無ければ従来どおりの execute プロンプト。
    let normal_prompt = build_prompt(&task, &RunContext::default(), "run-planner-2", "artifacts");
    assert!(!normal_prompt.contains("artifacts/execution-plan.json"));
}

/// ADR-0074 §6 F1 (a): planner プロンプトが WU ごとの `features`（`TaskFeatureHints` の 5 軸）を
/// 求める（説明・例・JSON 例のスナップショット）。上のテストの一部と重なるが、ADR の受け入れ
/// 条件そのものを指す独立したテストとして残す。
#[test]
fn execution_plan_prompt_asks_for_per_unit_features() {
    let task = crate::protocol::tests::sample_task();
    let planner_ctx = crate::protocol::ExecutionPlannerContext {
        gate_rule_id: "compound/score".to_string(),
        gate_score: 6,
        gate_signals: Vec::new(),
        max_work_units: 8,
        work_unit_max_turns: 80,
        work_unit_max_wall_secs: 3600,
        default_max_turns: 30,
        default_max_wall_secs: 1800,
        replan: false,
        replan_reason: String::new(),
        current_plan_version: None,
        work_unit_summaries: Vec::new(),
        preserve_done_keys: Vec::new(),
        parallel: false,
        max_phases: 0,
        ..Default::default()
    };
    let context = RunContext {
        execution_planner: Some(planner_ctx),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-planner-features", "artifacts");
    assert!(prompt.contains("### `features`"));
    assert!(prompt.contains("per-WorkUnit routing hints"));
    for axis in [
        "judgment",
        "ambiguity",
        "verifiability",
        "reversibility",
        "consequence",
    ] {
        assert!(prompt.contains(axis), "missing axis {axis} in prompt");
    }
    // JSON 例に `features` の欄がある。
    assert!(prompt.contains("\"features\":{"));
}

/// ADR-0072 D17（Phase E4b 項目1）: replan のときは「今の計画（版・WU の状態・完了/失敗の要約）」
/// 「起こした理由」「保持すべき done の WU の key」が文面に載り、v2 は done の WU を変えてはならない
/// と明示する。初回 planning（`replan = false`）のプロンプトは、この節を出さない限り 1 バイトも
/// 変わらない（上のテストで確認済み）。
#[test]
fn build_execution_plan_prompt_replan_includes_current_plan_and_reason() {
    let task = crate::protocol::tests::sample_task();
    let planner_ctx = crate::protocol::ExecutionPlannerContext {
        gate_rule_id: "compound/score".to_string(),
        gate_score: 6,
        gate_signals: Vec::new(),
        max_work_units: 8,
        work_unit_max_turns: 80,
        work_unit_max_wall_secs: 3600,
        default_max_turns: 30,
        default_max_wall_secs: 1800,
        replan: true,
        replan_reason: "work unit b failed: boom again".to_string(),
        current_plan_version: Some(1),
        work_unit_summaries: vec![
            "a (implement) status=done: implemented the core model".to_string(),
            "b (test) status=failed: work unit b failed: boom again".to_string(),
            "c (release) status=blocked (dependency_failed): waiting on b".to_string(),
        ],
        preserve_done_keys: vec!["a".to_string()],
        parallel: false,
        max_phases: 0,
        ..Default::default()
    };
    let context = RunContext {
        execution_planner: Some(planner_ctx),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-planner-2", "artifacts");

    assert!(prompt.contains("REPLANNING an existing execution plan"));
    assert!(prompt.contains("Current (superseded) plan version: v1."));
    assert!(prompt.contains("Why this replan was triggered: work unit b failed: boom again"));
    assert!(prompt.contains("a (implement) status=done: implemented the core model"));
    assert!(prompt.contains("b (test) status=failed: work unit b failed: boom again"));
    assert!(prompt.contains("c (release) status=blocked (dependency_failed): waiting on b"));
    assert!(prompt.contains("MUST appear unchanged"));
    assert!(prompt.contains("will be rejected: a."));
}

/// Phase F5-fix3: planner のプロンプトは検証が使う上限をすべて、run の context（= dispatcher の
/// `ExecutionLimits`）の値のまま出す（既定と違う値で確かめる）。
#[test]
fn the_planner_prompt_states_every_plan_limit_from_the_context() {
    let task = crate::protocol::tests::sample_task();
    let planner_ctx = crate::protocol::ExecutionPlannerContext {
        max_work_units: 7,
        work_unit_max_turns: 55,
        work_unit_max_wall_secs: 1234,
        default_max_turns: 30,
        default_max_wall_secs: 1800,
        parallel: true,
        max_phases: 4,
        max_title_chars: 99,
        max_objective_chars: 1777,
        max_done_when_items: 5,
        max_done_when_chars: 222,
        max_checks: 3,
        max_rationale_chars: 1111,
        max_plan_json_bytes: 20000,
        max_children: 6,
        ..Default::default()
    };
    for replan in [false, true] {
        let context = RunContext {
            execution_planner: Some(crate::protocol::ExecutionPlannerContext {
                replan,
                ..planner_ctx.clone()
            }),
            ..RunContext::default()
        };
        let prompt = build_prompt(&task, &context, "run-limits", "artifacts");
        assert!(prompt.contains("### Plan limits"), "replan={replan}");
        for needle in [
            "1 to 7 WorkUnits",
            "1 to 4 phases",
            "at most 6 child tasks",
            "`title` at most 99 characters",
            "`objective` at most 1777 characters",
            "at most 5 `done_when` items, each at most 222 characters",
            "**at most 3 `checks`**",
            "`rationale` at most 1111 characters",
            "at most 20000 bytes",
            "`max_turns` at 55, `max_wall_secs` at 1234",
        ] {
            assert!(
                prompt.contains(needle),
                "replan={replan}: missing {needle:?}"
            );
        }
        // 上限の節は計画の形の説明と schema の間（schema の指示のすぐ隣）にある。
        let limits_at = prompt.find("### Plan limits").unwrap_or(usize::MAX);
        let schema_at = prompt
            .find("### Schema for the `artifacts/execution-plan.json` object")
            .unwrap_or(0);
        assert!(limits_at < schema_at, "replan={replan}");
        assert_eq!(
            prompt.contains("after the diff is applied"),
            replan,
            "the diff note is only for replans"
        );
        // 最初の試行には「拒否された」節が出ない。
        assert!(!prompt.contains("Your previous plan was REJECTED"));
    }
}

/// Phase F5-fix3: 前の planner run の計画が拒否されていたら、その検証エラーをそのまま渡し、
/// 拒否されたファイルを再提出しないよう指示する。
#[test]
fn the_retry_planner_prompt_contains_the_previous_validation_error() {
    let task = crate::protocol::tests::sample_task();
    let context = RunContext {
        execution_planner: Some(crate::protocol::ExecutionPlannerContext {
            max_work_units: 10,
            replan: true,
            previous_attempt_errors: vec![
                "work unit sync-main: too many checks: 8 > 6".to_string(),
            ],
            ..Default::default()
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-retry", "artifacts");
    assert!(prompt.contains("### Your previous plan was REJECTED"));
    assert!(prompt.contains("- work unit sync-main: too many checks: 8 > 6\n"));
    assert!(prompt.contains("artifacts/execution-plan.rejected.json"));
    assert!(prompt.contains("Do NOT resubmit it"));
    // 欄の無い古い context（0）は既定の上限に倒す。
    assert!(prompt.contains("**at most 6 `checks`**"));
}

/// Phase 38（ADR-0028 追記）テスト用: ハーネス系の `literature`（`default_role` が `paperqa`）と、
/// ハーネスでない `coding`（`claude-code`）。`output_artifacts` は `名前: 説明` の形を混ぜる。
fn harness_genre_contexts() -> (GenreContext, GenreContext) {
    let roles = vec![
        task_core::RoleSpec {
            id: "literature-reader".into(),
            adapter: Some("paperqa".into()),
            ..task_core::RoleSpec::default()
        },
        task_core::RoleSpec {
            id: "implementer".into(),
            adapter: Some("claude-code".into()),
            ..task_core::RoleSpec::default()
        },
    ];
    let literature = task_core::GenreSpec {
        id: "literature".into(),
        description: "関連研究の調査".into(),
        output_artifacts: vec![
            "answer.md: 引用付きの答え".into(),
            "papers.json: 検索した論文の一覧（コーパス）".into(),
            "sources.json".into(),
        ],
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-reader".into()],
        ..task_core::GenreSpec::default()
    };
    let coding = task_core::GenreSpec {
        id: "coding".into(),
        description: "コードを書く".into(),
        output_artifacts: vec!["diff".into()],
        default_role: Some("implementer".into()),
        roles: vec!["implementer".into()],
        ..task_core::GenreSpec::default()
    };
    (
        GenreContext::from_spec(&literature, &roles),
        GenreContext::from_spec(&coding, &roles),
    )
}

/// Phase 38（ADR-0028 追記。実機のレビュー不合格から）: Plan run のプロンプトに、ハーネスで動く分野の
/// 成果物の規約（固定の名前と `名前: 説明` の説明、`artifact_exists` にはこの名前だけ、内容は
/// objective とレビュアー条件で）が出る。ハーネスでない分野は載らない。
#[test]
fn build_plan_prompt_states_the_artifact_convention_for_harness_genres() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Plan;
    let (literature, coding) = harness_genre_contexts();
    let context = RunContext {
        available_genres: vec![literature, coding],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-plan-harness-1", "artifacts");
    assert!(
        prompt.contains(
            "## ハーネスで動く分野の成果物（名前は固定）\n\
                 - literature: この分野の担当は**ハーネス**で動く。成果物は次の名前で固定され、担当が別のファイルを書くことはできない。\n\
                 \u{20}\u{20}- `answer.md`: 引用付きの答え\n\
                 \u{20}\u{20}- `papers.json`: 検索した論文の一覧（コーパス）\n\
                 \u{20}\u{20}- `sources.json`\n"
        ),
        "{prompt}"
    );
    assert!(
        prompt.contains("受け入れ条件（`artifact_exists`）にはこの名前だけを使うこと。"),
        "{prompt}"
    );
    assert!(
        prompt.contains("レビュアー条件（`{\"type\":\"reviewer\"}`）で判定させること"),
        "{prompt}"
    );
    // ハーネスでない分野（coding）は規約の節に出ない（「使える専門家」節には出る）。
    assert!(
        !prompt.contains("- coding: この分野の担当は**ハーネス**で動く"),
        "{prompt}"
    );
    assert!(prompt.contains("- coding: コードを書く"), "{prompt}");
}

/// Phase 38: ハーネスでない分野しか無い設定（coding だけ、あるいは `harness` が無い旧プロトコルの
/// ワーカー）では規約の節は**空**で、Plan / Execute プロンプトは Phase 37 までと 1 バイトも変わらない。
#[test]
fn the_artifact_convention_is_absent_without_a_harness_genre() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Plan;
    let (literature, coding) = harness_genre_contexts();
    let coding_only = RunContext {
        available_genres: vec![coding],
        ..RunContext::default()
    };
    assert_eq!(harness_artifacts_section_for_plan(&coding_only), "");
    let prompt = build_prompt(&task, &coding_only, "run-plan-harness-2", "artifacts");
    // Phase 59（ADR-0046 D3）: 計画の JSON スキーマには `harness` の説明が入るので、"ハーネス" の
    // 文字だけでは判定できない。規約の節そのものが無いことを見る。
    assert!(
        !prompt.contains("## ハーネスで動く分野の成果物"),
        "{prompt}"
    );

    // `output_artifacts` を書いていないハーネス系の分野も、出す名前が無いので節は出ない。
    let bare = RunContext {
        available_genres: vec![GenreContext {
            output_artifacts: Vec::new(),
            ..literature
        }],
        ..RunContext::default()
    };
    assert_eq!(harness_artifacts_section_for_plan(&bare), "");

    // Execute プロンプト（委譲側）には元から出さない。
    let mut execute = crate::protocol::tests::sample_task();
    execute.kind = task_core::TaskKind::Execute;
    let (literature, _) = harness_genre_contexts();
    let context = RunContext {
        available_genres: vec![literature],
        ..RunContext::default()
    };
    let execute_prompt = build_prompt(&execute, &context, "run-exec-harness", "artifacts");
    assert!(
        !execute_prompt.contains("## ハーネスで動く分野の成果物"),
        "{execute_prompt}"
    );
}

/// Phase 43（ADR-0039 D3）: 案件が作業場所を決めている run では、前置きに「## 作業場所」が出て
/// 「`ssh` で直接書くな」が入る。委譲できる run には「子は同じ作業場所を継ぐ」も足す。
#[test]
fn build_prompt_states_the_project_workspace_when_the_project_has_one() {
    let task = crate::protocol::tests::sample_task();
    let context = RunContext {
        workspace_note: Some(crate::preamble::workspace_note(
            &task_core::WorkspaceSpec::Remote {
                cluster: "pegasus".into(),
                path: std::path::PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
                mode: None,
            },
        )),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-ws-1", "artifacts");
    assert!(
        prompt.contains("## 作業場所 (where this project's code lives)"),
        "{prompt}"
    );
    assert!(
        prompt.contains("この案件のコードはクラスタ pegasus の `/work/NBB/rmaeda/workspace/rust/benchfs` にある。"),
        "{prompt}"
    );
    assert!(
        prompt.contains("`ssh` で直接書き込んではいけない"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Children you delegate inherit this project's workspace"),
        "{prompt}"
    );

    // Local の案件では「クラスタ」とは言わない。
    let local = RunContext {
        workspace_note: Some(crate::preamble::workspace_note(
            &task_core::WorkspaceSpec::Local {
                path: std::path::PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
                mode: None,
            },
        )),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &local, "run-ws-2", "artifacts");
    assert!(
        prompt.contains("この案件のコードは `/home/rmaeda/workspace/rust/pluvio-poc` にある。"),
        "{prompt}"
    );
}

/// Phase 43（ADR-0039 D3）: 計画 run には「子タスクの作業場所」と `plan.json` の `workspace` の
/// 使いどころが出る。作業場所を決めていない案件のプロンプトは Phase 42 までと**バイト単位で同じ**。
#[test]
fn build_plan_prompt_states_the_workspace_children_inherit_only_when_the_project_has_one() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Plan;
    let bare = build_prompt(&task, &RunContext::default(), "run-ws-3", "artifacts");
    // 節そのものは出ない（`plan.json` のスキーマには `workspace` の説明が元から載っている）。
    assert!(!bare.contains("## 子タスクの作業場所"), "{bare}");
    assert!(!bare.contains("## 作業場所"), "{bare}");

    let context = RunContext {
        workspace_note: Some(crate::preamble::workspace_note(
            &task_core::WorkspaceSpec::Local {
                path: std::path::PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
                mode: None,
            },
        )),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-ws-3", "artifacts");
    assert!(
        prompt.contains("## 子タスクの作業場所 (the workspace child tasks inherit)"),
        "{prompt}"
    );
    assert!(
        prompt.contains("分解した子タスクはこの作業場所をそのまま継ぐ"),
        "{prompt}"
    );
    assert!(
        prompt.contains("`{\"kind\":\"remote\",\"cluster\":\"...\",\"path\":\"...\"}`"),
        "{prompt}"
    );
    // 作業場所の 2 節（と、両方に共通する前置き）を取り除けば、Phase 42 までのプロンプトと
    // バイト単位で一致する（ADR-0067 D1: 前置きに常に「成果物の置き場所」の節が付くようになったので、
    // `bare` 側の前置きも同じだけ取り除いて比べる）。
    let preamble = crate::preamble::render(&context, "artifacts");
    assert!(!preamble.is_empty());
    let stripped = prompt
        .replace(&workspace_section_for_plan(&context), "")
        .replace(&preamble, "");
    let bare_preamble = crate::preamble::render(&RunContext::default(), "artifacts");
    let bare_stripped = bare.replace(&bare_preamble, "");
    assert_eq!(stripped.len(), bare_stripped.len());
    assert!(
        stripped == bare_stripped,
        "作業場所の節・共通の前置き以外は 1 バイトも変わらない"
    );
}

/// Phase 43: 案件が作業場所を決めていない run（既存のタスク）は、Execute プロンプトも従来どおり
/// （ADR-0067 D1 で前置きに常に付く「成果物の置き場所」の節を除けば Phase 42 までと変わらない）。
#[test]
fn a_project_without_a_workspace_keeps_the_previous_prompt_byte_for_byte() {
    let task = crate::protocol::tests::sample_task();
    let before = build_prompt(&task, &RunContext::default(), "run-ws-4", "artifacts");
    assert!(!before.contains("## 作業場所"), "{before}");
    assert_eq!(delegate_workspace_instruction(&RunContext::default()), "");
    assert_eq!(
        crate::preamble::render(&RunContext::default(), "artifacts"),
        format!(
            "{}{}{}{}",
            crate::preamble::deliverables_placement_note(),
            crate::preamble::production_host_note(),
            crate::preamble::tool_launch_policy_note(),
            crate::preamble::run_tmpdir_note()
        )
    );
}

/// Phase 38（ADR-0028 追記）: レビュアーのプロンプトにも同じ規約が出る（対象タスクの分野が
/// ハーネス系のときだけ）。`papers.json` は答えではなく、内容は `answer.md` で判定させる。
#[test]
fn build_review_prompt_states_the_artifact_convention_only_for_harness_genres() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Review;
    let (literature, coding) = harness_genre_contexts();
    let context = RunContext {
        subject_genre: Some(literature),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-review-harness-1", "artifacts");
    assert!(
        prompt.contains("## この担当の成果物（名前は固定。判定はこの前提で行う）"),
        "{prompt}"
    );
    assert!(
        prompt.contains("  - `papers.json`: 検索した論文の一覧（コーパス）\n"),
        "{prompt}"
    );
    assert!(
        prompt
            .contains("`papers.json` は検索したコーパスで\nあって答えではない。答えは `answer.md`"),
        "{prompt}"
    );
    assert!(
        prompt.contains("ファイル名の不一致だけを理由に不合格にはせず"),
        "{prompt}"
    );

    // ハーネスでない分野・分野が渡っていないレビューでは何も出ない。
    let coding_context = RunContext {
        subject_genre: Some(coding),
        ..RunContext::default()
    };
    assert_eq!(harness_artifacts_section_for_review(&coding_context), "");
    let none = build_prompt(
        &task,
        &RunContext::default(),
        "run-review-harness-2",
        "artifacts",
    );
    assert!(!none.contains("この担当の成果物"), "{none}");
}

/// ADR-0016 D3 / M4: `context.children` が非空なら集約 run の節が入り、成果物のまとめ方の指示が付く。
/// 無ければ節自体が出ない。
#[test]
fn build_prompt_includes_children_section_only_when_present() {
    let task = crate::protocol::tests::sample_task();
    let context = RunContext {
        children: vec![crate::protocol::ChildSummary {
            id: task_core::TaskId::new(),
            title: "implement parser".into(),
            role: Some("implementer".into()),
            status: task_core::Status::Done,
            branch: None,
            outcome: Some("done".into()),
            artifacts: vec![],
            workspace: Some(std::path::PathBuf::from("/tmp/child-ws")),
        }],
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-agg-1", "artifacts");
    assert!(prompt.contains("## Delegated child tasks (this is the aggregate run)"));
    assert!(prompt.contains("implement parser"));
    assert!(prompt.contains("artifacts/summary.md"));

    let no_children_prompt = build_prompt(&task, &RunContext::default(), "run-agg-2", "artifacts");
    assert!(!no_children_prompt.contains("Delegated child tasks"));
    assert!(!no_children_prompt.contains("artifacts/summary.md"));
}

/// ADR-0016 M8: run の終わりに `artifacts/delegate.json` があれば、`sink.delegate` が 1 回呼ばれる。
#[tokio::test]
async fn delegate_json_written_by_worker_is_forwarded_to_sink() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"delegated two subtasks","evidence":[]}' > artifacts/result.json
printf '%s' '{"tasks":[{"title":"a","objective":"do a","acceptance":[{"text":"c","check":{"type":"human"}}]},{"title":"b","objective":"do b","acceptance":[{"text":"c","check":{"type":"human"}}]}]}' > artifacts/delegate.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-delegate-1", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let delegated = sink.delegated.lock().unwrap();
    assert_eq!(delegated.len(), 1);
    assert_eq!(delegated[0].len(), 2);
}

/// 壊れた `artifacts/delegate.json` は `progress` に警告を残すだけで run は失敗させない（ADR-0016 M8）。
#[tokio::test]
async fn malformed_delegate_json_is_ignored_and_run_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"done, but wrote bad delegate.json","evidence":[]}' > artifacts/result.json
printf 'not json' > artifacts/delegate.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-delegate-2", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { .. } => {}
        other => panic!("expected done, got {other:?}"),
    }
    assert!(sink.delegated.lock().unwrap().is_empty());
    let progress = sink.progress.lock().unwrap();
    assert!(
        progress.iter().any(|m| m.contains("delegate.json ignored")),
        "{progress:?}"
    );
}

/// ADR-0024 D4: `rate_limit_event` を解析すると `sink.rate_limit` に観測値が渡る。ADR に載っている
/// 実測の行そのものを使う。
#[tokio::test]
async fn rate_limit_event_line_is_forwarded_to_the_sink() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
echo '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1789605600,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.14,"resetsAt":1789605600},"seven_day":{"utilization":0.24,"resetsAt":1790031600}}}}'
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-rate-1", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let observed = sink.rate_limits.lock().unwrap();
    assert_eq!(observed.len(), 1);
    let obs = &observed[0];
    assert_eq!(obs.five_hour.map(|w| w.utilization), Some(0.14));
    assert_eq!(obs.seven_day.map(|w| w.utilization), Some(0.24));
    assert_eq!(obs.status.as_deref(), Some("allowed"));
}

/// ADR-0024 D2: `with_env` の追加分は、既存の同名キーより後に環境を組み立てるので勝つ。
#[tokio::test]
async fn with_env_overrides_a_same_name_key_already_in_config_env() {
    let dir = tempfile::tempdir().unwrap();
    let out_file = dir.path().join("env-seen.txt");
    let mut config = stub_claude(
        dir.path(),
        &format!(
            r#"mkdir -p artifacts
printf '%s' "$CLAUDE_SECURESTORAGE_CONFIG_DIR" > {out}
printf '%s' '{{"summary":"ok","evidence":[]}}' > artifacts/result.json
echo '{{"type":"result","subtype":"success","is_error":false}}'
"#,
            out = out_file.display()
        ),
    );
    config.env.push((
        "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_string(),
        "old-account-dir".to_string(),
    ));
    let base = ClaudeCodeAdapter::new(config);
    let with_env = base
        .with_env(&[(
            "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_string(),
            "new-account-dir".to_string(),
        )])
        .expect("claude-code supports with_env");

    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = with_env
        .run(req, "run-env-1", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let seen = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(seen, "new-account-dir");
}

/// ADR-0072 D14（Phase E4b 項目3）: `with_permission_mode` の複製は `--permission-mode` の値を
/// 上書きする（`[execution.planner].permission_mode` を実際の CLI 引数に反映するための実行時配線。
/// E3 実装時に見送っていたフック）。既定のアダプタ（`bypassPermissions`）の CLI 引数には出ない。
#[tokio::test]
async fn with_permission_mode_overrides_the_permission_mode_argument() {
    let dir = tempfile::tempdir().unwrap();
    let out_file = dir.path().join("args-seen.txt");
    let config = stub_claude(
        dir.path(),
        &format!(
            r#"mkdir -p artifacts
printf '%s' "$*" > {out}
printf '%s' '{{"summary":"ok","evidence":[]}}' > artifacts/result.json
echo '{{"type":"result","subtype":"success","is_error":false}}'
"#,
            out = out_file.display()
        ),
    );
    assert_eq!(config.permission_mode, "bypassPermissions");
    let base = ClaudeCodeAdapter::new(config);
    let planner_mode = base
        .with_permission_mode("plan")
        .expect("claude-code supports with_permission_mode");

    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = planner_mode
        .run(req, "run-permission-1", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let seen = std::fs::read_to_string(&out_file).unwrap();
    assert!(
        seen.contains("--permission-mode plan"),
        "expected the overridden permission mode in the CLI args: {seen}"
    );
    assert!(
        !seen.contains("bypassPermissions"),
        "the adapter's default permission mode must not leak through: {seen}"
    );
}
#[tokio::test]
async fn tier_binding_reaches_cli_model_argument_and_preserves_account_env() {
    use task_core::{Tier, model_routing::ModelBinding};
    for tier in [Tier::Frontier, Tier::Standard, Tier::Cheap] {
        let dir = tempfile::tempdir().unwrap();
        let config = stub_claude(
            dir.path(),
            r#"
for a in "$@"; do printf '%s\0' "$a" >> args.log; done
printf '%s' "$ROUTING_ACCOUNT" > account.log
mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"turn.completed"}'
"#,
        );
        let expected = format!("explicit-{tier:?}");
        let adapter = crate::tiered::TieredAdapter {
            base: Arc::new(ClaudeCodeAdapter::new(config)),
            account_id: Some("account-a".into()),
            credential_error: None,
            models: [(
                tier,
                ModelBinding {
                    name: "requested-name".into(),
                    model_id: Some(expected.clone()),
                    unavailable_reason: None,
                    reasoning_effort: None,
                },
            )]
            .into(),
        };
        let adapter = adapter
            .with_env(&[("ROUTING_ACCOUNT".into(), "account-a".into())])
            .unwrap();
        assert_eq!(adapter.account_id(), Some("account-a"));
        let mut req = sample_req(dir.path().to_path_buf());
        req.task.worker_hint.tier = tier;
        let _ = adapter
            .run(req, "tier-run", default_limits(), &RecordingSink::default())
            .await
            .unwrap();
        let args = std::fs::read_to_string(dir.path().join("args.log")).unwrap();
        let args: Vec<_> = args.split('\0').collect();
        let model = args.windows(2).find(|pair| pair[0] == "--model").unwrap()[1];
        assert_eq!(model, expected);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("account.log")).unwrap(),
            "account-a"
        );
    }
}

/// ADR-0069 Phase 118 D1: claude-code には effort 相当の CLI 引数・環境変数が無い
/// （`claude --help` を実行して確認できる本物の CLI が無いこの環境では、この判断は運用側の
/// 実測メモに基づく）。`WorkerAdapter` の既定（`supports_reasoning_effort() == false`、
/// `with_reasoning_effort` は `None`）のままなので、設定に `reasoning_effort` を書いても argv には
/// 一切現れない（`--model` は従来どおり渡る）。
#[tokio::test]
async fn tier_reasoning_effort_does_not_reach_claude_code_argv() {
    use task_core::{Tier, model_routing::ModelBinding};
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let base = ClaudeCodeAdapter::new(config);
    assert!(!base.supports_reasoning_effort());
    assert!(base.with_reasoning_effort("high").is_none());
    let adapter = crate::tiered::TieredAdapter {
        base: Arc::new(base),
        account_id: None,
        credential_error: None,
        models: [(
            Tier::Standard,
            ModelBinding {
                name: "requested-name".into(),
                model_id: Some("claude-sonnet-5".into()),
                unavailable_reason: None,
                reasoning_effort: Some("medium".into()),
            },
        )]
        .into(),
    };
    let mut req = sample_req(dir.path().to_path_buf());
    req.task.worker_hint.tier = Tier::Standard;
    let _ = adapter
        .run(
            req,
            "tier-run-1",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(
        !args
            .iter()
            .any(|a| a.to_ascii_lowercase().contains("reasoning") || a == "medium"),
        "{args:?}"
    );
    let model = args.windows(2).find(|pair| pair[0] == "--model").unwrap()[1].clone();
    assert_eq!(model, "claude-sonnet-5");
}

fn args_log_script() -> &'static str {
    r#"
for a in "$@"; do printf '%s\0' "$a" >> args.log; done
mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"turn.completed"}'
"#
}

fn captured_args(dir: &std::path::Path) -> Vec<String> {
    let args = std::fs::read_to_string(dir.join("args.log")).unwrap();
    args.split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// ADR-0054 D1（Phase 67）: `context.session` が無ければ Phase 66 までと同じ
/// `--no-session-persistence`。
#[tokio::test]
async fn without_a_session_the_cli_keeps_no_session_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let _ = adapter
        .run(req, "run-1", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(args.contains(&"--no-session-persistence".to_string()));
    assert!(!args.contains(&"--session-id".to_string()));
    assert!(!args.contains(&"--resume".to_string()));
}

/// ADR-0054 D1（Phase 67）: 継続セッションの**最初の run**（`resume: false`）は
/// `--session-id <id>`（これから使う id を固定）で、`--no-session-persistence` は付かない。
#[tokio::test]
async fn a_fresh_session_passes_session_id_not_no_session_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        resume: false,
    });
    let _ = adapter
        .run(req, "run-1", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"--no-session-persistence".to_string()));
    let idx = args
        .iter()
        .position(|a| a == "--session-id")
        .expect("--session-id present");
    assert_eq!(args[idx + 1], "550e8400-e29b-41d4-a716-446655440000");
    assert!(!args.contains(&"--resume".to_string()));
}

/// ADR-0054 D1（Phase 67）: 継続セッションの**2 回目以降**（`resume: true`）は `--resume <id>`。
#[tokio::test]
async fn a_continuing_session_passes_resume() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(req, "run-2", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"--no-session-persistence".to_string()));
    assert!(!args.contains(&"--session-id".to_string()));
    let idx = args
        .iter()
        .position(|a| a == "--resume")
        .expect("--resume present");
    assert_eq!(args[idx + 1], "550e8400-e29b-41d4-a716-446655440000");
}

/// `context.session` が別アダプタ向けなら無視する（渡り歩きは無い）。
#[tokio::test]
async fn a_session_for_another_adapter_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: "codex".to_string(),
        session_id: "codex-session".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(req, "run-3", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(args.contains(&"--no-session-persistence".to_string()));
}

/// ADR-0054 D2（Phase 68）: CoS の対話 run（`conversation_addressee = Secretary`）だけ
/// `--allowedTools` に読み取り専用の `celerisctl` サブコマンドが渡る。それ以外の run には
/// 付かない（対話 run は道具を使わないという ADR-0033 D4 の原則のまま）。
#[tokio::test]
async fn the_cos_conversation_run_gets_a_readonly_tool_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    let _ = adapter
        .run(req, "run-cos", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    let idx = args
        .iter()
        .position(|a| a == "--allowedTools")
        .expect("--allowedTools present");
    let allowed = &args[idx + 1];
    assert!(
        allowed.contains("Bash(celerisctl knowledge search:*)"),
        "{allowed}"
    );
    assert!(
        allowed.contains("Bash(celerisctl knowledge get:*)"),
        "{allowed}"
    );
    assert!(allowed.contains("Bash(celerisctl ls:*)"), "{allowed}");
    assert!(allowed.contains("Bash(celerisctl show:*)"), "{allowed}");
    assert!(
        allowed.contains("Bash(celerisctl projects ls:*)"),
        "{allowed}"
    );
    assert!(
        allowed.contains("Bash(celerisctl projects show:*)"),
        "{allowed}"
    );
}

/// 対話でない run・CoS 以外の対話（`Other`）には `--allowedTools` は付かない。
#[tokio::test]
async fn non_cos_runs_get_no_tool_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    assert_eq!(req.context.conversation_addressee, None);
    let _ = adapter
        .run(
            req,
            "run-plain",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"--allowedTools".to_string()));

    let dir2 = tempfile::tempdir().unwrap();
    let config2 = stub_claude(dir2.path(), args_log_script());
    let adapter2 = ClaudeCodeAdapter::new(config2);
    let mut req2 = sample_req(dir2.path().to_path_buf());
    req2.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Other);
    let _ = adapter2
        .run(
            req2,
            "run-other",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args2 = captured_args(dir2.path());
    assert!(!args2.contains(&"--allowedTools".to_string()));
}

/// `runs/<run_id>/request.json` に `context.session` がそのまま残る（実装依頼の受け入れ条件:
/// resume の有無が `request.json` から読み取れること）。
#[tokio::test]
async fn request_json_records_the_session_handle() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(req, "run-4", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let request_json =
        std::fs::read_to_string(dir.path().join("runs").join("run-4").join("request.json"))
            .unwrap();
    let value: serde_json::Value = serde_json::from_str(&request_json).unwrap();
    assert_eq!(value["context"]["session"]["resume"], true);
    assert_eq!(
        value["context"]["session"]["session_id"],
        "550e8400-e29b-41d4-a716-446655440000"
    );
    assert_eq!(value["context"]["session"]["adapter"], "claude-code");
}

/// ADR-0054 D1（Phase 67）: `--session-id` で spawn できたら、resume していなくても
/// `session_established` を報告する（次の run から確実に resume できるよう、celeris 自身が
/// 選んだ id をそのまま確認させる）。
#[tokio::test]
async fn a_fresh_session_reports_session_established() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        resume: false,
    });
    let sink = RecordingSink::default();
    let _ = adapter
        .run(req, "run-5", default_limits(), &sink)
        .await
        .unwrap();
    assert_eq!(
        sink.session_established.lock().unwrap().as_slice(),
        ["550e8400-e29b-41d4-a716-446655440000".to_string()]
    );
    assert!(sink.session_resume_failed.lock().unwrap().is_empty());
}

/// ADR-0054 D1（Phase 67）: `--resume` を頼んだ run が、既知の「セッションが見つからない」文言を
/// 含む stderr で crash したら `session_resume_failed` を報告する（実機での文言は未確認。
/// `provider::looks_like_resume_rejection` のコメント参照）。
#[tokio::test]
async fn a_rejected_resume_reports_session_resume_failed() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        "echo 'Error: No conversation found for session 01ARZ3' 1>&2; exit 1",
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        resume: true,
    });
    let sink = RecordingSink::default();
    let outcome = adapter.run(req, "run-6", default_limits(), &sink).await;
    // 供給側失敗としては分類されない文面なので run 自体は retryable な通常のエラーで返る。
    match outcome {
        Ok(o) => assert!(matches!(
            o.terminal,
            Terminal::Error {
                retryable: true,
                ..
            }
        )),
        Err(e) => panic!("expected Ok(Terminal::Error), got {e:?}"),
    }
    let failed = sink.session_resume_failed.lock().unwrap();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("No conversation found"), "{failed:?}");
}

/// resume していない run が同じ文言で crash しても `session_resume_failed` は報告しない
/// （resume を頼んでいない run には関係が無い判断のため）。
#[tokio::test]
async fn a_crash_without_resuming_does_not_report_session_resume_failed() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        "echo 'Error: No conversation found for session 01ARZ3' 1>&2; exit 1",
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let _ = adapter.run(req, "run-7", default_limits(), &sink).await;
    assert!(sink.session_resume_failed.lock().unwrap().is_empty());
}

/// Phase 113 D1/D4(a)（ADR-0054 追記。本番のタスク 01M35X86XTK84F97QW0CN5PGMR / reviewer run
/// 01M388BENASH3JEBWFS03KEQYT の再現）: `result` メッセージを一度観測できた run（＝上の
/// `a_rejected_resume_reports_session_resume_failed` が使う「result を一度も観測できずクラッシュ」
/// 経路ではない）でも、`subtype: error_during_execution` かつ `is_error` で、stderr の末尾が
/// resume 拒否の文言なら `session_resume_failed` を報告する。Phase 67 時点はこの経路をまったく
/// チェックしておらず、本番ではこの形（`error_during_execution` の `result` が出た上で stderr に
/// 理由が 1 行だけ）で self-heal が働かなかった。
#[tokio::test]
async fn phase_113_a_rejected_resume_with_an_observed_result_message_reports_session_resume_failed()
{
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        "echo '{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true}'; \
             echo 'No conversation found with session ID: 01a0d017-e32a-4cad-b10c-0cb63869ae13' 1>&2; \
             exit 1",
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        resume: true,
    });
    let sink = RecordingSink::default();
    let outcome = adapter.run(req, "run-113a", default_limits(), &sink).await;
    match outcome {
        Ok(o) => assert!(matches!(
            o.terminal,
            Terminal::Error {
                retryable: true,
                ..
            }
        )),
        Err(e) => panic!("expected Ok(Terminal::Error), got {e:?}"),
    }
    let failed = sink.session_resume_failed.lock().unwrap();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("No conversation found"), "{failed:?}");
}

/// Phase 113: 同じ `error_during_execution`/`is_error` でも resume を頼んでいない run では
/// `session_resume_failed` を報告しない（resume していない run には関係の無い判断のため）。
#[tokio::test]
async fn phase_113_an_error_during_execution_without_resuming_does_not_report_session_resume_failed()
 {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        "echo '{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true}'; \
             echo 'No conversation found with session ID: 01a0d017-e32a-4cad-b10c-0cb63869ae13' 1>&2; \
             exit 1",
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let _ = adapter.run(req, "run-113b", default_limits(), &sink).await;
    assert!(sink.session_resume_failed.lock().unwrap().is_empty());
}

/// Phase 113: `error_during_execution`/`is_error` の resume run でも、stderr が resume 拒否の
/// 文言を含まなければ `session_resume_failed` は報告しない（他のインフラ都合の失敗まで
/// resume 拒否として誤検出しない）。
#[tokio::test]
async fn phase_113_an_error_during_execution_without_resume_wording_does_not_report_session_resume_failed()
 {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        "echo '{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true}'; \
             echo 'internal error: unexpected panic' 1>&2; \
             exit 1",
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        resume: true,
    });
    let sink = RecordingSink::default();
    let _ = adapter.run(req, "run-113c", default_limits(), &sink).await;
    assert!(sink.session_resume_failed.lock().unwrap().is_empty());
}

/// ADR-0054 Phase 67b 追記: `--resume` に渡す id が UUID でなければ、spawn する**前**に拒否する
/// （本番で ULID を渡してすべての CoS 対話・部門長レビュー run が失敗した事故の再発防止。
/// `resolve_node_session` 側の自己修復（Phase 67b）を将来の回帰が回避しても、ここでテストが
/// 静かに ULID を通さず落ちるようにする）。
#[tokio::test]
async fn a_non_uuid_resume_id_is_refused_without_spawning() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        resume: true,
    });
    let err = adapter
        .run(req, "run-8", default_limits(), &RecordingSink::default())
        .await
        .expect_err("a non-UUID resume id must be refused, not spawned");
    assert!(format!("{err}").contains("not a valid UUID"), "{err}");
    assert!(
        !dir.path().join("args.log").exists(),
        "the claude stub must not have been spawned"
    );
}

/// 同じ拒否を、初回の `--session-id`（`resume: false`）でも見る。
#[tokio::test]
async fn a_non_uuid_fresh_session_id_is_refused_without_spawning() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(dir.path(), args_log_script());
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        resume: false,
    });
    let err = adapter
        .run(req, "run-9", default_limits(), &RecordingSink::default())
        .await
        .expect_err("a non-UUID session id must be refused, not spawned");
    assert!(format!("{err}").contains("not a valid UUID"), "{err}");
    assert!(
        !dir.path().join("args.log").exists(),
        "the claude stub must not have been spawned"
    );
}

/// F5-fix5: 本番 run 01M3KF2HFMHPJR7YEB5HMT38MQ の stdout.jsonl の形（`task_started`〈background〉→
/// `result`〈success / end_turn〉→ `task_updated`〈killed〉→ `task_notification`〈stopped〉、result.json 無し）
/// を流す fake claude。`extra` は fixture を流す前に実行するシェル（checkpoint.json を置くなど）。
fn headless_background_stub(dir: &Path, extra: &str) -> ClaudeCodeConfig {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude-code-headless-background.jsonl"
    );
    stub_claude(dir, &format!("{extra}\ncat '{fixture}'"))
}

/// F5-fix5: execute run では continuation（`Terminal::Yielded`）になり、checkpoint の `next_action` が
/// 殺された command を foreground で再実行するよう申し送る。worker の checkpoint.json の欄は保つ。
#[tokio::test]
async fn f5_fix5_background_task_killed_at_end_turn_becomes_a_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let config = headless_background_stub(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"completed":["cargo fmt"],"next_action":"run clippy"}' > artifacts/checkpoint.json"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-headless-1", default_limits(), &sink)
        .await
        .expect("a killed background task is a continuation, not an adapter error");
    match outcome.terminal {
        Terminal::Yielded { checkpoint, usage } => {
            let next = checkpoint["next_action"].as_str().unwrap_or_default();
            assert!(next.contains("foreground"), "{next}");
            assert!(
                next.contains("timeout 1800 cargo test --workspace"),
                "{next}"
            );
            assert!(
                next.contains("run clippy"),
                "previous next_action kept: {next}"
            );
            assert_eq!(checkpoint["completed"][0], "cargo fmt");
            let failure = checkpoint["known_failures"][0]["what"]
                .as_str()
                .unwrap_or_default();
            assert!(
                failure.starts_with(HEADLESS_BACKGROUND_TASK_CLASS),
                "{failure}"
            );
            // dispatcher が読む形（`WorkerCheckpointInput`）として解釈できる。
            let parsed: task_core::WorkerCheckpointInput =
                serde_json::from_value(checkpoint.clone()).expect("checkpoint schema");
            assert_eq!(parsed.completed, vec!["cargo fmt".to_string()]);
            assert_eq!(usage.and_then(|u| u.output_tokens), Some(2277));
        }
        other => panic!("expected yielded, got {other:?}"),
    }
    let progress = sink.progress.lock().unwrap().clone();
    assert!(
        progress
            .iter()
            .any(|p| p.starts_with(HEADLESS_BACKGROUND_TASK_CLASS)),
        "the classification is recorded as a progress event: {progress:?}"
    );
    // run dir の result.json（celeris の記録）も yield を残す。
    let recorded =
        std::fs::read_to_string(dir.path().join("runs/run-headless-1/result.json")).unwrap();
    assert!(recorded.contains("yield"), "{recorded}");
}

/// F5-fix5: continuation の仕組みを持たない run（レビュー）は、従来どおり result.json 不在の失敗
/// （`AdapterError::Other`）のまま、文言に分類名を足す。
#[tokio::test]
async fn f5_fix5_a_review_run_keeps_the_missing_result_error_with_the_class() {
    let dir = tempfile::tempdir().unwrap();
    let config = headless_background_stub(dir.path(), "");
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.task.kind = TaskKind::Review;
    let err = adapter
        .run(
            req,
            "run-headless-2",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap_err();
    match err {
        AdapterError::Other(message) => {
            assert!(message.starts_with(RESULT_JSON_MISSING_MARKER), "{message}");
            assert!(
                message.contains(HEADLESS_BACKGROUND_TASK_CLASS),
                "{message}"
            );
        }
        other => panic!("expected AdapterError::Other, got {other:?}"),
    }
}

/// F5-fix5: `result` より前に終わった background task は数えない（従来の result.json 不在の失敗）。
#[tokio::test]
async fn f5_fix5_a_background_task_finished_before_the_result_is_not_misclassified() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"echo '{"type":"system","subtype":"task_started","task_id":"t1","description":"cargo build","is_backgrounded":true}'
echo '{"type":"system","subtype":"task_notification","task_id":"t1","status":"completed"}'
echo '{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn"}'"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let err = adapter
        .run(
            req,
            "run-headless-3",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap_err();
    match err {
        AdapterError::Other(message) => {
            assert!(message.starts_with(RESULT_JSON_MISSING_MARKER), "{message}");
            assert!(
                !message.contains(HEADLESS_BACKGROUND_TASK_CLASS),
                "{message}"
            );
        }
        other => panic!("expected AdapterError::Other, got {other:?}"),
    }
}

/// F5-fix5: 予防。全 run の CLI 引数に headless の system prompt を足し、background task を無効にし、
/// foreground の Bash の上限を壁時計に合わせる。`config.env` の同名はそちらが勝つ。
#[tokio::test]
async fn f5_fix5_every_run_gets_the_headless_system_prompt_and_background_off() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        "{}\nprintf '%s|%s' \"$CLAUDE_CODE_DISABLE_BACKGROUND_TASKS\" \"$BASH_MAX_TIMEOUT_MS\" > env.log",
        args_log_script()
    );
    let config = stub_claude(dir.path(), &script);
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.task.kind = TaskKind::Review;
    let limits = RunLimits {
        wall_clock: Duration::from_secs(3600),
        ..default_limits()
    };
    let _ = adapter
        .run(req, "run-headless-4", limits, &RecordingSink::default())
        .await;
    let args = captured_args(dir.path());
    let i = args
        .iter()
        .position(|a| a == "--append-system-prompt")
        .expect("--append-system-prompt present");
    assert_eq!(args[i + 1], crate::preamble::HEADLESS_RUN_NOTE);
    // F5-fix10: `-p` は値を取らないフラグのまま、プロンプト本文はどの引数にも載らない（stdin で渡す）。
    assert_eq!(args[0], "-p", "{args:?}");
    assert_eq!(args[1], "--output-format", "{args:?}");
    assert!(
        args.iter().all(|a| !a.contains("# Task:")),
        "the prompt must not be passed via argv: {args:?}"
    );
    let env = std::fs::read_to_string(dir.path().join("env.log")).unwrap();
    assert_eq!(env, "1|3600000");

    // `config.env` が同名を持てば、そちらが勝つ。
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_claude(dir.path(), &script);
    config.env = vec![(
        "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".to_string(),
        "0".to_string(),
    )];
    let _ = ClaudeCodeAdapter::new(config)
        .run(
            sample_req(dir.path().to_path_buf()),
            "run-headless-5",
            default_limits(),
            &RecordingSink::default(),
        )
        .await;
    let env = std::fs::read_to_string(dir.path().join("env.log")).unwrap();
    assert_eq!(env, "0|600000", "short wall clocks keep the CLI default");
}

/// F5-fix10（本番障害: task 01M3MS2JRDJ4GM0D9VN9PJCB6B の planner run 01M3Q21Z9JQWWANGHXJPNH1F8X。
/// 135,644 バイトの replan プロンプトが `-p <prompt>` で MAX_ARG_STRLEN を超え E2BIG）: 200 KiB の
/// プロンプトでも spawn は失敗せず、stdin から欠けずに届く（fake の claude が stdin をファイルに写す）。
#[tokio::test]
async fn f5_fix10_a_200_kib_prompt_reaches_claude_intact_through_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!("cat > stdin.log\n{}", args_log_script());
    let config = stub_claude(dir.path(), &script);
    let mut req = sample_req(dir.path().to_path_buf());
    let filler = "0123456789abcdef".repeat(200 * 1024 / 16);
    req.task.objective = format!("BEGIN-OBJECTIVE {filler} END-OBJECTIVE");
    ClaudeCodeAdapter::new(config)
        .run(
            req,
            "run-f5fix10",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .expect("a 200 KiB prompt must not fail to spawn");
    let got = std::fs::read_to_string(dir.path().join("stdin.log")).unwrap();
    let recorded = std::fs::read_to_string(dir.path().join("runs/run-f5fix10/prompt.txt")).unwrap();
    assert!(got.len() > crate::subprocess::MAX_SINGLE_ARG_BYTES);
    assert_eq!(got, recorded, "stdin carries exactly the recorded prompt");
    assert!(got.contains(&filler));
    let args = captured_args(dir.path());
    assert_eq!(args[0], "-p", "{args:?}");
    assert!(
        args.iter()
            .all(|a| a.len() < crate::subprocess::MAX_SINGLE_ARG_BYTES
                && !a.contains("BEGIN-OBJECTIVE")),
        "no argv element carries the prompt"
    );
}

/// F5-fix10: 回帰の歯止め。それでも 1 つの引数が 128 KiB 以上になれば、spawn せず（`os error 7` ではなく）
/// アダプタ名と大きさを名指しした読めるエラーで落ちる。
#[tokio::test]
async fn f5_fix10_an_oversized_single_argument_fails_with_a_readable_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_claude(dir.path(), "touch spawned.marker");
    config.model = Some("m".repeat(crate::subprocess::MAX_SINGLE_ARG_BYTES));
    let err = ClaudeCodeAdapter::new(config)
        .run(
            sample_req(dir.path().to_path_buf()),
            "run-f5fix10-guard",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .expect_err("an oversized argument must be refused before spawning");
    let message = err.to_string();
    assert!(message.contains("claude-code"), "{message}");
    assert!(
        message.contains(&crate::subprocess::MAX_SINGLE_ARG_BYTES.to_string()),
        "{message}"
    );
    assert!(message.contains("MAX_ARG_STRLEN"), "{message}");
    assert!(!dir.path().join("spawned.marker").exists());
}

/// ADR-0079 §7 R2b (a) `planner_prompt_carries_depth_and_leaf_criteria`: 木の節点の planner（`context.tree`）の
/// プロンプトは /3 の形・深さ・残りの深さ・祖先・leaf の基準・決定の書き方・計画と木の上限の残り・人の段階の
/// 名指しを出し、「収まらない unit は子 task か決定にし、leaf に押し込まない」と書く。`remaining_depth = 0` では
/// kind task を書くなと出る。replan は差分でなく全体を書かせ、失敗した子の扱いを出す。`tree = None` の
/// プロンプトには /3 の語が出ない（/1・/2 は従来どおり）。
#[test]
fn planner_prompt_carries_depth_and_leaf_criteria() {
    let task = crate::protocol::tests::sample_task();
    let tree = crate::protocol::TreePlannerContext {
        depth: 1,
        max_depth: 3,
        remaining_depth: 2,
        max_stages: 4,
        max_units_per_stage: 5,
        max_child_tasks_per_plan: 3,
        max_decisions_per_plan: 7,
        max_parallel_child_tasks: 2,
        leaves_left: 37,
        runs_left: 111,
        replans_left: 9,
        node_replans_left: 3,
        tokens_left: Some(500_000),
        open_decisions_left: 11,
        ancestors: Vec::new(),
        stages_hint: vec![
            task_core::StageHint {
                title: "Phase 1".to_string(),
                scope: "MVP of the browser capability".to_string(),
            },
            task_core::StageHint {
                title: "Phase 2".to_string(),
                scope: String::new(),
            },
        ],
    };
    let planner_ctx = crate::protocol::ExecutionPlannerContext {
        gate_rule_id: "human/explicit".to_string(),
        max_work_units: 8,
        work_unit_max_turns: 80,
        work_unit_max_wall_secs: 3600,
        default_max_turns: 30,
        default_max_wall_secs: 1800,
        parallel: true,
        max_phases: 5,
        tree: Some(tree.clone()),
        ..Default::default()
    };
    let context = RunContext {
        execution_planner: Some(planner_ctx.clone()),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-planner-tree", "artifacts");
    for needle in [
        "\"schema\":\"celeris.execution-plan/3\"",
        "Depth: 1 (task levels; the root task is depth 1). Max depth: 3. **Remaining depth: 2**.",
        "You may declare units with `\"kind\":\"task\"`",
        "Leaf criteria (ADR-0079 D4)",
        "\"gate\":\"compound\"|\"atomic\" (optional)",
        "`\"gate\":\"compound\"` — the child writes its own plan",
        "`\"gate\":\"atomic\"` — the child runs as a single node without a plan",
        "否定の grep（`! grep …`）を check に書くときは、自分が書く説明文や ADR の本文に当たらないか確かめる",
        "`budget.max_turns` ≤ 80 and `budget.max_wall_secs` ≤ 3600",
        "`context.repo` names at most one repository",
        "`checks` has at least one deterministic command",
        "must be declared as a child task or raised as a decision — never squeezed into a leaf",
        "Decisions for a human (`decisions`, ADR-0079 D7)",
        "`needed_before`",
        "`needs_decisions`",
        "At most 7 decisions per plan",
        "at most 4 stages, 5 units per stage, 3 child-task units, 7 decisions",
        "leaves left: 37, runs left: 111, replans left: 9 (this task: 3), tokens left: 500000, open decisions left: 11",
        "At most 2 child tasks of this task run at the same time",
        "Stages the human named (stages_hint, ADR-0079 D12)",
        "- \"Phase 1\": MVP of the browser capability",
        "- \"Phase 2\"\n",
        "- `stages`: 1 to 4 stages. At most 5 units per stage",
        "is **rejected** (not capped)",
    ] {
        assert!(prompt.contains(needle), "missing {needle:?} in:\n{prompt}");
    }
    // /1・/2 の形と /2 の工程の節は出さない（/3 の形だけ）。
    assert!(!prompt.contains("\"schema\":\"celeris.execution-plan/1\""));
    assert!(!prompt.contains("Phases and parallel WorkUnits"));
    assert!(!prompt.contains("- `work_units`: 1 to"));

    // 深さ 3（残り 0）: kind task を書くなと出る。祖先の題名と段階が出る。
    let deep = crate::protocol::TreePlannerContext {
        depth: 3,
        remaining_depth: 0,
        ancestors: vec![
            crate::protocol::TreeAncestorContext {
                title: "browser capability".to_string(),
                stage: Some("phase-2".to_string()),
                objective_excerpt: "ship the browser".to_string(),
            },
            crate::protocol::TreeAncestorContext {
                title: "policy contract".to_string(),
                stage: Some("build".to_string()),
                objective_excerpt: "define the policy".to_string(),
            },
        ],
        stages_hint: Vec::new(),
        ..tree.clone()
    };
    let context = RunContext {
        execution_planner: Some(crate::protocol::ExecutionPlannerContext {
            tree: Some(deep),
            ..planner_ctx.clone()
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-planner-deep", "artifacts");
    assert!(prompt.contains("**Remaining depth: 0**"), "{prompt}");
    assert!(prompt.contains("**do NOT write any unit with `\"kind\":\"task\"`**"));
    assert!(!prompt.contains("You may declare units with"));
    assert!(
        prompt.contains("depth 1: \"browser capability\" (stage `phase-2`) — ship the browser")
    );
    assert!(prompt.contains("depth 2: \"policy contract\" (stage `build`) — define the policy"));
    assert!(!prompt.contains("stages_hint"));

    // replan: 差分でなく全体、done の unit は持ち越し、失敗した子の扱い。
    let context = RunContext {
        execution_planner: Some(crate::protocol::ExecutionPlannerContext {
            replan: true,
            replan_reason: "child task \"Child c\" (unit c, attempt 1) failed (work): tests fail"
                .to_string(),
            current_plan_version: Some(1),
            work_unit_summaries: vec!["c (task) status=failed: child task x is failed".to_string()],
            preserve_done_keys: vec!["a".to_string()],
            ..planner_ctx.clone()
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-planner-replan", "artifacts");
    assert!(prompt.contains("REPLANNING an existing execution plan"));
    assert!(prompt.contains("the diff schema is not available for this shape"));
    assert!(prompt.contains("Why this replan was triggered: child task \"Child c\""));
    assert!(prompt.contains("carry over unchanged"));
    assert!(prompt.contains("keep the same unit key (celeris then creates a new child task"));
    assert!(!prompt.contains("execution-plan-delta/1"));

    // tree = None: /3 の語は出ない（/2 のプロンプトは従来どおり）。
    let context = RunContext {
        execution_planner: Some(crate::protocol::ExecutionPlannerContext {
            tree: None,
            ..planner_ctx
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-planner-v2", "artifacts");
    assert!(prompt.contains("\"schema\":\"celeris.execution-plan/2\""));
    assert!(prompt.contains("Phases and parallel WorkUnits"));
    assert!(!prompt.contains("Leaf criteria"));
    assert!(!prompt.contains("Remaining depth"));
}

/// agent-docs/adr/2026-10-05-browser-department-web-live-view.md D2.0 (e): /3（木の子 task unit）の
/// planner プロンプトに「browser 子 task の origin は最小・親を超えない」規則と例
/// `https://billing.example.com` が出る。
#[test]
fn browser_allowed_domains_prompt_tree_planner_has_minimal_origin_rule() {
    let task = crate::protocol::tests::sample_task();
    let tree = crate::protocol::TreePlannerContext {
        remaining_depth: 2,
        max_depth: 3,
        ..crate::protocol::TreePlannerContext::default()
    };
    let context = RunContext {
        execution_planner: Some(crate::protocol::ExecutionPlannerContext {
            tree: Some(tree),
            ..crate::protocol::ExecutionPlannerContext::default()
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-planner-browser-origin", "artifacts");
    assert!(
        prompt.contains("requirements.browser.allowed_domains"),
        "{prompt}"
    );
    assert!(
        prompt.contains("https://billing.example.com"),
        "missing example origin in:\n{prompt}"
    );
    assert!(
        prompt.contains("never wider than this task's own `allowed_domains`"),
        "missing minimal/parent-scope rule in:\n{prompt}"
    );
}

/// agent-docs/adr/2026-10-05-browser-department-web-live-view.md D2.0 (e): /2（`children`）の planner
/// プロンプトにも同じ規則と例が出る。
#[test]
fn browser_allowed_domains_prompt_v2_planner_has_minimal_origin_rule() {
    let task = crate::protocol::tests::sample_task();
    let context = RunContext {
        execution_planner: Some(crate::protocol::ExecutionPlannerContext {
            parallel: true,
            ..crate::protocol::ExecutionPlannerContext::default()
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(
        &task,
        &context,
        "run-planner-browser-origin-v2",
        "artifacts",
    );
    assert!(prompt.contains("Phases and parallel WorkUnits"));
    assert!(
        prompt.contains("requirements.browser.allowed_domains"),
        "{prompt}"
    );
    assert!(
        prompt.contains("https://billing.example.com"),
        "missing example origin in:\n{prompt}"
    );
}

/// ADR-0079 R7-2/R7-10: /2 と /3 の planner のプロンプトに「check の書き方」の節が出る。/3 は子を作る unit
/// だけを数える上限の説明と、dispatcher が渡す /3 の JSON の大きさの上限を出す。
#[test]
fn planner_prompt_has_the_check_writing_section() {
    let task = crate::protocol::tests::sample_task();
    let needles = [
        "### check の書き方",
        "`agent-docs/progress/`, `agent-docs/adr/` (its ADR), every path this plan itself says the unit may write",
        "files regenerated by its change (schemas under `docs/protocol/` and `docs/api/v1/`, generated types under `gui/` and `web/`)",
        // ADR-0074 付記 2026-10-05: 範囲 check は `scope: true`・`$CELERIS_WU_BASE` 基点・path を出してから非 0。
        "`{\"cmd\":\"...\",\"expect_exit\":0,\"scope\":true}`. It runs only at work-unit time",
        "is NOT rerun after stage integration or in a child task's final review, so never write it as task acceptance",
        "`out=$({ git diff --name-only \"${CELERIS_WU_BASE:-HEAD}\"; git ls-files --others --exclude-standard; } | sort -u | grep -vE '^(<allowed path regex>)'); [ -z \"$out\" ] || { echo \"out of scope:\"; echo \"$out\"; exit 1; }`",
        "never compare a scope check with a hard-coded sha or `$(git merge-base HEAD main)`",
        "`git log --format= --name-only \"$CELERIS_WU_BASE..HEAD\" --not \"$CELERIS_WU_TARGET\"`",
        "never write the silent `test -z \"$(...)\"` form",
        // ADR-0128 D3・D5・D7: 記録の置き場所と land 系 check の 3 本。
        "a new ADR is `agent-docs/adr/YYYY-MM-DD-<slug>.md` (no new ADR numbers)",
        "`agent-docs/progress/YYYY-MM-DD-<slug>/<unit key>.md`. Never append to `agent-docs/PROGRESS.md` (frozen).",
        "`sh scripts/dev/check-doc-links.sh`, `sh scripts/dev/check-adr-numbers.sh` and `sh scripts/dev/progress-index.sh --check`",
        "Do not pass extra positional arguments to `pnpm -C <dir> test` or `cargo test`",
        "corepack pnpm@<version from package.json packageManager> -C <dir>",
        "A non-scope diff check may compare against `$(git merge-base HEAD main)`",
        "scope checks use `$CELERIS_WU_BASE` instead.",
        "A negated grep (`! grep ...`) must not match text the unit itself writes",
        "a check that runs a script another unit creates belongs to a unit that `depends_on` the creating unit",
        // ADR-0079 付記 R7-5 D5: check の走る所（git の worktree が無い task）と、unit 自身が作るスクリプトの呼び出し方。
        "or the task's directory (where `artifacts/` is) when the task has no git worktree",
        "write the exact invocation (the arguments the check passes) in the unit's objective",
        "Checks run with `/bin/sh` (dash), so do not use bash-only syntax such as `${s:0:12}`, `[[ ]]`, or arrays.",
        "run them with CELERIS_USERNS_TESTS=1 in the daemon's integration check (release gate), not in a leaf.",
        "Keep each leaf small enough for one run, and do not pack implementation work into a recording or close-out leaf.",
        "replace mandatory `cargo test --workspace` with a check that `crates/` has no diff",
        "Include the planned ADR and recording locations from the start in acceptance criteria and diff-check path scopes.",
        "Do not run CPU-burning load scripts (busy loops, stress-ng, parallel cargo load) in checks or acceptance; reproduce timing bugs deterministically (paused or injected clock, event waits, SIGSTOP/SIGCONT, test-only delay hooks; see agent-docs/guides/testing.md).",
    ];
    let v2 = crate::protocol::ExecutionPlannerContext {
        gate_rule_id: "human/explicit".to_string(),
        max_work_units: 8,
        parallel: true,
        max_phases: 5,
        max_plan_json_bytes: 24 * 1024,
        ..Default::default()
    };
    let v3 = crate::protocol::ExecutionPlannerContext {
        max_plan_json_bytes: 64 * 1024,
        tree: Some(crate::protocol::TreePlannerContext {
            depth: 1,
            max_depth: 3,
            remaining_depth: 2,
            max_stages: 5,
            max_units_per_stage: 6,
            max_child_tasks_per_plan: 6,
            ..Default::default()
        }),
        ..v2.clone()
    };
    for (name, planner) in [("v2", v2), ("v3", v3)] {
        let context = RunContext {
            execution_planner: Some(planner.clone()),
            ..RunContext::default()
        };
        let prompt = build_prompt(&task, &context, "run-planner-checks", "artifacts");
        for needle in needles {
            assert_eq!(prompt.matches(needle).count(), 1, "{name}: {needle:?}");
        }
        assert_eq!(prompt.matches("### check の書き方").count(), 1, "{name}");
        let bytes = if planner.tree.is_some() { 65536 } else { 24576 };
        assert!(
            prompt.contains(&format!("the whole plan JSON at most {bytes} bytes")),
            "{name}"
        );
        if planner.tree.is_some() {
            assert!(prompt.contains(
                "that will create a child (units already done and `adopt` units do not count)"
            ));
        }
    }
}

/// ADR-0095 付記 D-d: /2 と /3 の planner のプロンプトに「本番 host の操作は人が実行する手順として書く」の
/// 節が出て、`systemctl --user` / `systemd-run` / `~/.config/celeris` / `~/.local/celeris/releases` に
/// 触れる WorkUnit を計画しない旨が明示される。
#[test]
fn planner_prompt_declares_production_host_changes_as_a_human_procedure() {
    let task = crate::protocol::tests::sample_task();
    let needles = [
        "### 本番 host の操作",
        "systemctl --user",
        "systemd-run",
        "~/.config/celeris",
        "~/.local/celeris/releases",
        "`/local`",
        "/local/celeris/state/releases",
    ];
    let v2 = crate::protocol::ExecutionPlannerContext {
        gate_rule_id: "human/explicit".to_string(),
        max_work_units: 8,
        parallel: true,
        max_phases: 5,
        max_plan_json_bytes: 24 * 1024,
        ..Default::default()
    };
    let v3 = crate::protocol::ExecutionPlannerContext {
        max_plan_json_bytes: 64 * 1024,
        tree: Some(crate::protocol::TreePlannerContext {
            depth: 1,
            max_depth: 3,
            remaining_depth: 2,
            max_stages: 5,
            max_units_per_stage: 6,
            max_child_tasks_per_plan: 6,
            ..Default::default()
        }),
        ..v2.clone()
    };
    for (name, planner) in [("v2", v2), ("v3", v3)] {
        let context = RunContext {
            execution_planner: Some(planner),
            ..RunContext::default()
        };
        let prompt = build_prompt(&task, &context, "run-planner-production-host", "artifacts");
        for needle in needles {
            assert!(prompt.contains(needle), "{name}: missing {needle:?}");
        }
        assert_eq!(prompt.matches("### 本番 host の操作").count(), 1, "{name}");
    }
}

/// ADR-0079 付記「R7-3」D1 / D5: replan の planner（/2・/3）に「done の unit で直せるのは `checks` だけ（統合で再実行）」
/// を伝え、/3 の段階あたりの上限の行は done と `adopt` を数えないと書く。
#[test]
fn replan_prompt_allows_rewriting_only_the_checks_of_done_units() {
    let task = crate::protocol::tests::sample_task();
    let v2 = crate::protocol::ExecutionPlannerContext {
        gate_rule_id: "human/explicit".to_string(),
        max_work_units: 8,
        parallel: true,
        max_phases: 5,
        max_plan_json_bytes: 24 * 1024,
        replan: true,
        current_plan_version: Some(1),
        preserve_done_keys: vec!["merge-old-tip".to_string()],
        ..Default::default()
    };
    let v3 = crate::protocol::ExecutionPlannerContext {
        max_plan_json_bytes: 64 * 1024,
        tree: Some(crate::protocol::TreePlannerContext {
            depth: 2,
            max_depth: 3,
            remaining_depth: 1,
            max_stages: 5,
            max_units_per_stage: 6,
            max_child_tasks_per_plan: 6,
            ..Default::default()
        }),
        ..v2.clone()
    };
    for (name, planner, needles) in [
        (
            "v2",
            v2,
            vec![
                "The one exception is `checks`: you may replace a done WorkUnit's `checks`",
                "The done WorkUnit is not re-run; its checks without `\"scope\":true` are re-run at the phase integration.",
            ],
        ),
        (
            "v3",
            v3,
            vec![
                "The only thing you may change in a done unit is its `checks`",
                "The done unit is not re-run; its checks without `\"scope\":true` are re-run at the stage integration.",
                "At most 6 units per stage (leaves + child tasks; units already done, `adopt` units, and \
                     celeris-added integration steps and repairs do not count)",
            ],
        ),
    ] {
        let context = RunContext {
            execution_planner: Some(planner),
            ..RunContext::default()
        };
        let prompt = build_prompt(&task, &context, "run-planner-replan", "artifacts");
        assert!(prompt.contains("merge-old-tip"), "{name}");
        for needle in needles {
            assert!(prompt.contains(needle), "{name}: missing {needle:?}");
        }
    }
}

/// ADR-0140 D4: stream-json の行を `handle_line` に通し、最後の `result` の usage に載る
/// `duplicate_reads` を返す。
fn duplicate_reads_after(root: &Path, resumed: bool, lines: &[String]) -> Option<Usage> {
    let sink = RecordingSink::default();
    let mut last_result = None;
    let mut background = BackgroundTasks::default();
    let mut exploration = ExplorationTracker::new(Some(root), resumed);
    for line in lines {
        handle_line(
            line,
            &sink,
            &mut last_result,
            &mut background,
            &mut exploration,
        );
    }
    last_result.and_then(|meta| meta.usage)
}

fn tool_use_line(name: &str, input: serde_json::Value) -> String {
    serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": name, "input": input}]}
    })
    .to_string()
}

fn result_line() -> String {
    r#"{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":1,"output_tokens":2}}"#
        .to_string()
}

/// ADR-0140 D4: 同じ path の 2 回目の `Read` は 1 件の重複（絶対 path と cwd 相対・`./`・`..` を
/// 同じ正規化 path として数える）。
#[test]
fn duplicate_read_same_path_twice_counts_one() {
    let root = Path::new("/work/repo");
    let lines = vec![
        tool_use_line(
            "Read",
            serde_json::json!({"file_path": "/work/repo/src/lib.rs"}),
        ),
        tool_use_line(
            "Read",
            serde_json::json!({"file_path": "./src/../src/lib.rs"}),
        ),
        result_line(),
    ];
    let usage = duplicate_reads_after(root, false, &lines).expect("usage");
    assert_eq!(usage.duplicate_reads, Some(1));
    assert_eq!(usage.session_resumed, Some(false));
    assert_eq!(usage.input_tokens, Some(1));
}

/// ADR-0140 D4: 別 path の `Read`、別 path の同じ pattern の `Grep`、`Glob`・他の道具は重複にしない。
#[test]
fn duplicate_read_distinct_paths_count_zero() {
    let root = Path::new("/work/repo");
    let lines = vec![
        tool_use_line(
            "Read",
            serde_json::json!({"file_path": "/work/repo/src/lib.rs"}),
        ),
        tool_use_line(
            "Read",
            serde_json::json!({"file_path": "/work/repo/src/main.rs"}),
        ),
        tool_use_line(
            "Grep",
            serde_json::json!({"pattern": "fn main", "path": "src"}),
        ),
        tool_use_line(
            "Grep",
            serde_json::json!({"pattern": "fn main", "path": "tests"}),
        ),
        tool_use_line("Glob", serde_json::json!({"pattern": "**/*.rs"})),
        tool_use_line("Bash", serde_json::json!({"command": "cat src/lib.rs"})),
        tool_use_line("Bash", serde_json::json!({"command": "cat src/lib.rs"})),
        result_line(),
    ];
    let usage = duplicate_reads_after(root, false, &lines).expect("usage");
    assert_eq!(usage.duplicate_reads, Some(0));
}

/// ADR-0140 D4: `Grep`/`Glob` は pattern + path が同じなら重複（path の書き方の違いは正規化する）。
#[test]
fn duplicate_read_counts_repeated_grep_and_glob() {
    let root = Path::new("/work/repo");
    let lines = vec![
        tool_use_line(
            "Grep",
            serde_json::json!({"pattern": "Usage", "path": "crates"}),
        ),
        tool_use_line(
            "Grep",
            serde_json::json!({"pattern": "Usage", "path": "/work/repo/crates"}),
        ),
        tool_use_line("Glob", serde_json::json!({"pattern": "**/*.rs"})),
        tool_use_line("Glob", serde_json::json!({"pattern": "**/*.rs"})),
        tool_use_line("Read", serde_json::json!({"file_path": "crates"})),
        result_line(),
    ];
    let usage = duplicate_reads_after(root, true, &lines).expect("usage");
    assert_eq!(usage.duplicate_reads, Some(2));
    assert_eq!(usage.session_resumed, Some(true));
}

/// ADR-0140 D4: `--resume` で起動した run は終了結果の usage に `session_resumed = true` が載り、
/// stream の再 Read も数えられる（スタブの claude。外部ネットワーク・実 claude は使わない）。
#[tokio::test]
async fn duplicate_read_and_resume_mark_reach_the_terminal_usage() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"mkdir -p artifacts
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/lib.rs"}}]}}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/lib.rs"}}]}}'
printf '%s' '{"summary":"continued","evidence":[]}' > artifacts/result.json
echo '{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":10,"output_tokens":20}}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        resume: true,
    });
    let outcome = adapter
        .run(
            req,
            "run-resume",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { usage, .. } => {
            let usage = usage.expect("usage");
            assert_eq!(usage.session_resumed, Some(true));
            assert_eq!(usage.duplicate_reads, Some(1));
        }
        other => panic!("expected done, got {other:?}"),
    }
}

/// ADR-0140 D4: resume を頼んでも拒否された run（`error_during_execution` + 拒否の文言）は
/// `session_resumed = false`（resume した run に数えない）。
#[tokio::test]
async fn duplicate_read_rejected_resume_is_not_marked_resumed() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_claude(
        dir.path(),
        r#"echo 'No conversation found with session ID: 550e8400-e29b-41d4-a716-446655440000' >&2
echo '{"type":"result","subtype":"error_during_execution","is_error":true,"usage":{"input_tokens":1,"output_tokens":0}}'
"#,
    );
    let adapter = ClaudeCodeAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: ClaudeCodeAdapter::ID.to_string(),
        session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        resume: true,
    });
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-rejected", default_limits(), &sink)
        .await;
    let usage = match outcome {
        Ok(RunOutcome { mut terminal, .. }) => terminal_usage_mut(&mut terminal).copied(),
        Err(_) => None,
    };
    if let Some(usage) = usage {
        assert_eq!(usage.session_resumed, Some(false));
    }
    let result_json =
        std::fs::read_to_string(dir.path().join("runs/run-rejected/result.json")).unwrap();
    assert!(
        !result_json.contains("\"session_resumed\":true"),
        "{result_json}"
    );
}

/// ADR-0124 D4: 直行経路の run だけ `## Acceptance criteria` の直後に「直行経路（planner なし）」の節が出る。
#[test]
fn direct_route_prompt_has_the_section_after_the_acceptance_criteria() {
    let task = crate::protocol::tests::sample_task();
    let context = RunContext {
        direct_route: Some(crate::protocol::DirectRouteContext {
            policy_version: "direct-route/1".into(),
            overrode_gate: true,
            reasons: vec!["direct/single-repo: repos=1, multi_environment=false".into()],
        }),
        ..RunContext::default()
    };
    let prompt = build_prompt(&task, &context, "run-direct", "artifacts");
    let section = prompt
        .find("## 直行経路（planner なし）")
        .expect("direct route section");
    let acceptance = prompt.find("## Acceptance criteria").expect("acceptance");
    let instructions = prompt.find("## Instructions").expect("instructions");
    assert!(acceptance < section && section < instructions, "{prompt}");
    assert!(
        prompt.contains("調査 → 編集 → テスト → 局所修正を、この run の中で完結させる。"),
        "{prompt}"
    );
    assert!(prompt.contains("落ちたら同じ run の中で直して再実行する"));
    assert!(prompt.contains("判定の根拠（direct-route/1）:"));
    assert!(prompt.contains("- direct/single-repo: repos=1, multi_environment=false"));
}

/// ADR-0124 D4: `direct_route` が無い run のプロンプトは 1 バイトも変わらない（節を除けば同一）。
#[test]
fn direct_route_prompt_absent_leaves_the_prompt_unchanged() {
    let task = crate::protocol::tests::sample_task();
    let without = build_prompt(&task, &RunContext::default(), "run-x", "artifacts");
    assert!(!without.contains("直行経路"));
    let context = RunContext {
        direct_route: Some(crate::protocol::DirectRouteContext {
            policy_version: "direct-route/1".into(),
            ..Default::default()
        }),
        ..RunContext::default()
    };
    let with = build_prompt(&task, &context, "run-x", "artifacts");
    let section = super::prompt::direct_route_section(&context);
    assert!(!section.is_empty());
    assert!(!section.contains("判定の根拠"), "{section}");
    assert_eq!(with.replacen(&section, "", 1), without);
    assert!(super::prompt::direct_route_section(&RunContext::default()).is_empty());
}

#[test]
fn compact_boundaries_report_completed_compactions_only() {
    let sink = RecordingSink::default();
    let mut result = None;
    let mut background = BackgroundTasks::default();
    let mut exploration = ExplorationTracker::default();
    for line in [
        r#"{"type":"system","subtype":"status","status":"compacting"}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"auto-compact compaction"}]}}"#,
        r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto"}}"#,
        r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"manual"}}"#,
    ] {
        handle_line(line, &sink, &mut result, &mut background, &mut exploration);
    }
    let fields = sink.structured.lock().unwrap();
    assert_eq!(
        fields
            .iter()
            .filter(|(_, f)| f.kind == Some(task_core::ProgressKind::Status)
                && f.tool.as_deref() == Some(task_core::tree::CONTEXT_COMPACTION_TOOL))
            .count(),
        2
    );
}

// ---- ADR 2026-10-07-worker-no-subagents-no-llm-cli: subagent の既定禁止と別 LLM の起動の検出 ----

/// D1: 既定で `--disallowedTools Agent,Task,Workflow` が付き、運用側の `extra_args`（`--allowedTools Agent` /
/// `--tools default` / 本番の応急処置 `--disallowedTools=Agent,Task`）の**後ろ**に来る（最後の語が勝つ）。
#[tokio::test]
async fn subagent_tools_are_disallowed_by_default_after_any_extra_args() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_claude(dir.path(), args_log_script());
    config.extra_args = vec![
        "--allowedTools".into(),
        "Agent".into(),
        "--tools".into(),
        "default".into(),
        "--disallowedTools=Agent,Task".into(),
    ];
    assert_eq!(config.subagents, crate::tool_policy::SubagentPolicy::Deny);
    let adapter = ClaudeCodeAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let _ = adapter
        .run(req, "run-deny", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    let deny = args
        .iter()
        .position(|a| a == "--disallowedTools")
        .expect("--disallowedTools present");
    assert_eq!(args[deny + 1], "Agent,Task,Workflow", "{args:?}");
    let allow = args.iter().position(|a| a == "--allowedTools").unwrap();
    let legacy = args
        .iter()
        .position(|a| a == "--disallowedTools=Agent,Task")
        .unwrap();
    assert!(deny > allow && deny > legacy, "{args:?}");
    assert_eq!(
        args.iter().filter(|a| *a == "--disallowedTools").count(),
        1,
        "{args:?}"
    );
}

/// D1/D7: 外せるのは明示の設定だけ。`subagents = "allow"` では付かない。`"allow_cos"` は CoS の対話 run にだけ
/// 効き、それ以外の run には deny が残る。
#[tokio::test]
async fn only_an_explicit_subagents_setting_lifts_the_deny() {
    use crate::tool_policy::SubagentPolicy;
    async fn args_for(
        policy: SubagentPolicy,
        addressee: Option<crate::protocol::ConversationAddressee>,
    ) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let mut config = stub_claude(dir.path(), args_log_script());
        config.subagents = policy;
        let mut req = sample_req(dir.path().to_path_buf());
        req.context.conversation_addressee = addressee;
        let _ = ClaudeCodeAdapter::new(config)
            .run(
                req,
                "run-policy",
                default_limits(),
                &RecordingSink::default(),
            )
            .await
            .unwrap();
        captured_args(dir.path())
    }
    let has_deny = |args: &[String]| args.iter().any(|a| a == "--disallowedTools");
    assert!(!has_deny(&args_for(SubagentPolicy::Allow, None).await));
    assert!(has_deny(&args_for(SubagentPolicy::Deny, None).await));
    assert!(has_deny(&args_for(SubagentPolicy::AllowCos, None).await));
    assert!(has_deny(
        &args_for(
            SubagentPolicy::AllowCos,
            Some(crate::protocol::ConversationAddressee::Other)
        )
        .await
    ));
    assert!(!has_deny(
        &args_for(
            SubagentPolicy::AllowCos,
            Some(crate::protocol::ConversationAddressee::Secretary)
        )
        .await
    ));
    assert!(has_deny(
        &args_for(
            SubagentPolicy::Deny,
            Some(crate::protocol::ConversationAddressee::Secretary)
        )
        .await
    ));
}

/// D5: stream-json の `tool_use` から、subagent 道具（`Agent`）と shell からの別 LLM CLI（`claude -p`）・
/// API（`curl api.anthropic.com`）の起動を検出し、sink の `policy_violation` と `error = true` の `status` 進行に
/// 残す。普通の `Bash` / `Read` は何も出さない。
#[test]
fn tool_uses_that_launch_subagents_or_other_llms_are_reported_to_the_sink() {
    use task_core::{ProgressKind, ToolPolicyKind};
    let sink = RecordingSink::default();
    let mut last_result = None;
    let mut background = BackgroundTasks::default();
    let mut exploration = ExplorationTracker::default();
    for line in [
        tool_use_line("Read", serde_json::json!({"file_path": "src/lib.rs"})),
        tool_use_line(
            "Bash",
            serde_json::json!({"command": "cargo test -p task-worker claude_code"}),
        ),
        tool_use_line(
            "Agent",
            serde_json::json!({"prompt": "review the plan", "subagent_type": "general-purpose"}),
        ),
        tool_use_line(
            "Bash",
            serde_json::json!({"command": "cd repo && claude -p 'summarize the diff'"}),
        ),
        tool_use_line(
            "Bash",
            serde_json::json!({"command": "curl -s https://api.anthropic.com/v1/messages -d @req.json"}),
        ),
    ] {
        handle_line(
            &line,
            &sink,
            &mut last_result,
            &mut background,
            &mut exploration,
        );
    }
    let violations = sink
        .violations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    assert_eq!(
        violations
            .iter()
            .map(|v| (v.kind, v.tool.as_str(), v.matched.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (ToolPolicyKind::SubagentTool, "Agent", "Agent"),
            (ToolPolicyKind::LlmCli, "Bash", "claude"),
            (ToolPolicyKind::LlmApi, "Bash", "api.anthropic.com"),
        ],
        "{violations:#?}"
    );
    assert!(violations[0].command.contains("review the plan"));
    assert_eq!(
        violations[1].command,
        "cd repo && claude -p 'summarize the diff'"
    );
    // 人が Console で読む行: `status`・`error = true`・`policy:` で始まる。
    let flagged: Vec<_> = sink
        .structured
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, f)| f.kind == Some(ProgressKind::Status) && f.error)
        .map(|(m, _)| m.clone())
        .collect();
    assert_eq!(flagged.len(), 3, "{flagged:#?}");
    assert!(
        flagged.iter().all(|m| m.starts_with("policy: ")),
        "{flagged:#?}"
    );
    assert!(
        flagged[0].contains("subagent tool `Agent`"),
        "{}",
        flagged[0]
    );
    assert!(flagged[1].contains("LLM CLI `claude`"), "{}", flagged[1]);
    assert!(
        flagged[2].contains("LLM API `api.anthropic.com`"),
        "{}",
        flagged[2]
    );
}

/// D6: reviewer のプロンプトに、celeris が記録した検出の節が出る（無ければ出ない）。
#[test]
fn the_review_prompt_lists_recorded_tool_policy_violations() {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = task_core::TaskKind::Review;
    let mut review = crate::protocol::ReviewRequest {
        summary: "done".into(),
        criteria: vec![0],
        ..Default::default()
    };
    let without = build_prompt(
        &task,
        &RunContext {
            review: Some(review.clone()),
            ..RunContext::default()
        },
        "run-rv",
        "artifacts",
    );
    assert!(
        !without.contains("## Tool policy violations recorded by celeris"),
        "{without}"
    );

    review.policy_violations = vec![
        crate::protocol::ReviewPolicyViolation {
            kind: "subagent_tool".into(),
            tool: "Agent".into(),
            matched: "Agent".into(),
            command: "{\"prompt\":\"review the plan\"}".into(),
        },
        crate::protocol::ReviewPolicyViolation {
            kind: "llm_cli".into(),
            tool: "Bash".into(),
            matched: "codex".into(),
            command: "codex exec 'fix tests'".into(),
        },
    ];
    let with = build_prompt(
        &task,
        &RunContext {
            review: Some(review),
            ..RunContext::default()
        },
        "run-rv",
        "artifacts",
    );
    assert!(
        with.contains("## Tool policy violations recorded by celeris (authoritative)"),
        "{with}"
    );
    assert!(
        with.contains(
            "- subagent_tool via `Agent` (matched `Agent`): {\"prompt\":\"review the plan\"}"
        ),
        "{with}"
    );
    assert!(
        with.contains("- llm_cli via `Bash` (matched `codex`): codex exec 'fix tests'"),
        "{with}"
    );
    assert!(with.contains("fail that criterion"), "{with}");
    let section = with.find("## Tool policy violations").unwrap();
    let summary = with.find("## Worker's self-reported summary").unwrap();
    assert!(section < summary, "{with}");
}
