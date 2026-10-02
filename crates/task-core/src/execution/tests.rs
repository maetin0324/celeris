use super::*;

fn ctx(end: CheckpointEnd) -> CheckpointContext {
    CheckpointContext {
        task_id: "01TASK".into(),
        work_unit: None,
        run_id: "01RUN".into(),
        run_seq: 2,
        end,
        created_at: "2026-09-24T12:00:00Z".into(),
    }
}

#[test]
fn missing_worker_checkpoint_falls_back_to_mechanical_defaults() {
    let mechanical = MechanicalCheckpoint {
        repo_state: Some(RepoState {
            branch: "celeris/x".into(),
            base: "main@abc".into(),
            head: "def".into(),
            uncommitted: true,
            diff_stat: "1 file changed".into(),
        }),
        files_changed: vec![CheckpointFileChange {
            path: "src/lib.rs".into(),
            change: "modified".into(),
            note: None,
        }],
        tests_run: vec![],
        recent_activity: vec!["Bash: cargo test".into()],
    };
    let cp = merge_checkpoint(None, mechanical, ctx(CheckpointEnd::BudgetExhausted));
    assert_eq!(cp.source, CheckpointSource::Mechanical);
    assert_eq!(cp.remaining, vec![NO_CHECKPOINT_REMAINING.to_string()]);
    assert_eq!(cp.next_action, NO_CHECKPOINT_NEXT_ACTION);
    assert_eq!(cp.files_changed.len(), 1);
    assert_eq!(cp.schema, CHECKPOINT_SCHEMA);
}

#[test]
fn schema_violation_is_treated_like_missing_checkpoint() {
    // 型が合わない（`completed` が配列でない）。
    assert!(parse_worker_checkpoint(r#"{"completed": "not-an-array"}"#).is_none());
    // 壊れた JSON。
    assert!(parse_worker_checkpoint("{not json").is_none());
    // 空オブジェクトは schema 違反ではない（全欄が既定値で読める）。
    assert!(parse_worker_checkpoint("{}").is_some());
}

#[test]
fn worker_checkpoint_wins_semantic_fields_mechanical_wins_factual_fields() {
    let worker = WorkerCheckpointInput {
        completed: vec!["store に execution.rs を追加".into()],
        remaining: vec!["dispatcher の配線".into()],
        next_action: Some("dispatcher.rs の on_worker_finished を直す".into()),
        files_changed: vec![CheckpointFileChange {
            path: "crates/task-core/src/execution.rs".into(),
            change: "added".into(),
            note: Some("型と純粋関数".into()),
        }],
        ..Default::default()
    };
    let mechanical = MechanicalCheckpoint {
        files_changed: vec![
            CheckpointFileChange {
                path: "crates/task-core/src/execution.rs".into(),
                change: "added".into(),
                note: None,
            },
            CheckpointFileChange {
                path: "crates/task-core/src/model.rs".into(),
                change: "modified".into(),
                note: None,
            },
        ],
        ..Default::default()
    };
    let cp = merge_checkpoint(
        Some(worker),
        mechanical,
        ctx(CheckpointEnd::BudgetExhausted),
    );
    assert_eq!(cp.source, CheckpointSource::Merged);
    assert_eq!(
        cp.completed,
        vec!["store に execution.rs を追加".to_string()]
    );
    assert_eq!(cp.next_action, "dispatcher.rs の on_worker_finished を直す");
    // mechanical が正（git が見つけた 2 ファイルとも残る）。worker の note は path で付く。
    assert_eq!(cp.files_changed.len(), 2);
    let annotated = cp
        .files_changed
        .iter()
        .find(|f| f.path.ends_with("execution.rs"))
        .unwrap();
    assert_eq!(annotated.note.as_deref(), Some("型と純粋関数"));
    let other = cp
        .files_changed
        .iter()
        .find(|f| f.path.ends_with("model.rs"))
        .unwrap();
    assert_eq!(other.note, None);
}

#[test]
fn tests_run_is_a_union_deduplicated_by_command() {
    let worker = WorkerCheckpointInput {
        tests_run: vec![
            CheckpointTestRun {
                command: "cargo test -p task-core".into(),
                exit: Some(0),
                summary: Some("12 passed".into()),
            },
            CheckpointTestRun {
                command: "cargo clippy".into(),
                exit: Some(0),
                summary: None,
            },
        ],
        ..Default::default()
    };
    let mechanical = MechanicalCheckpoint {
        tests_run: vec![CheckpointTestRun {
            command: "cargo test -p task-core".into(),
            exit: None,
            summary: None,
        }],
        ..Default::default()
    };
    let cp = merge_checkpoint(
        Some(worker),
        mechanical,
        ctx(CheckpointEnd::BudgetExhausted),
    );
    assert_eq!(cp.tests_run.len(), 2, "{:?}", cp.tests_run);
    // mechanical のコマンドが先（重複除去は worker 側を捨てる）。
    assert_eq!(cp.tests_run[0].command, "cargo test -p task-core");
    assert_eq!(cp.tests_run[0].exit, None);
    assert_eq!(cp.tests_run[1].command, "cargo clippy");
}

#[test]
fn truncate_checkpoint_enforces_item_and_string_caps() {
    let mut cp = Checkpoint {
        schema: CHECKPOINT_SCHEMA.into(),
        task_id: "t".into(),
        work_unit: None,
        run_id: "r".into(),
        run_seq: 1,
        end: CheckpointEnd::BudgetExhausted,
        source: CheckpointSource::Mechanical,
        completed: (0..50).map(|i| format!("item {i}")).collect(),
        remaining: vec!["x".repeat(1000)],
        decisions: vec![],
        files_changed: vec![],
        tests_run: vec![],
        known_failures: vec![],
        artifact_refs: vec![],
        next_action: String::new(),
        open_questions: vec![],
        plan_issue: None,
        repo_state: None,
        recent_activity: vec![],
        created_at: "2026-09-24T00:00:00Z".into(),
    };
    truncate_checkpoint(&mut cp);
    assert_eq!(cp.completed.len(), CHECKPOINT_MAX_ITEMS);
    assert!(cp.remaining[0].chars().count() <= CHECKPOINT_MAX_STRING_CHARS + 1);
    assert!(cp.remaining[0].ends_with('…'));
}

#[test]
fn truncate_checkpoint_shrinks_to_the_overall_byte_cap() {
    let mut cp = Checkpoint {
        schema: CHECKPOINT_SCHEMA.into(),
        task_id: "t".into(),
        work_unit: None,
        run_id: "r".into(),
        run_seq: 1,
        end: CheckpointEnd::BudgetExhausted,
        source: CheckpointSource::Merged,
        completed: (0..CHECKPOINT_MAX_ITEMS)
            .map(|i| format!("completed item number {i} ").repeat(5))
            .collect(),
        remaining: (0..CHECKPOINT_MAX_ITEMS)
            .map(|i| format!("remaining item number {i} ").repeat(5))
            .collect(),
        decisions: (0..CHECKPOINT_MAX_ITEMS)
            .map(|i| CheckpointDecision {
                what: format!("decision {i}").repeat(5),
                why: "because".repeat(5),
            })
            .collect(),
        files_changed: (0..CHECKPOINT_MAX_ITEMS)
            .map(|i| CheckpointFileChange {
                path: format!("crates/some/very/long/path/file_{i}.rs"),
                change: "modified".into(),
                note: Some("a fairly long note about the change".repeat(3)),
            })
            .collect(),
        tests_run: (0..CHECKPOINT_MAX_ITEMS)
            .map(|i| CheckpointTestRun {
                command: format!("cargo test -p crate_{i}"),
                exit: Some(0),
                summary: Some("passed".repeat(10)),
            })
            .collect(),
        known_failures: vec![],
        artifact_refs: vec![],
        next_action: "next".repeat(50),
        open_questions: vec![],
        plan_issue: None,
        repo_state: None,
        recent_activity: (0..CHECKPOINT_MAX_ITEMS)
            .map(|i| format!("Bash: cargo test -p crate_{i}").repeat(3))
            .collect(),
        created_at: "2026-09-24T00:00:00Z".into(),
    };
    truncate_checkpoint(&mut cp);
    let bytes = checkpoint_byte_len(&cp);
    assert!(bytes <= CHECKPOINT_MAX_BYTES, "got {bytes} bytes");
}

#[test]
fn progress_definition_matches_d18() {
    let base = Checkpoint {
        schema: CHECKPOINT_SCHEMA.into(),
        task_id: "t".into(),
        work_unit: None,
        run_id: "r1".into(),
        run_seq: 1,
        end: CheckpointEnd::BudgetExhausted,
        source: CheckpointSource::Mechanical,
        completed: vec!["a".into()],
        remaining: vec!["b".into(), "c".into()],
        decisions: vec![],
        files_changed: vec![CheckpointFileChange {
            path: "a.rs".into(),
            change: "modified".into(),
            note: None,
        }],
        tests_run: vec![],
        known_failures: vec![],
        artifact_refs: vec![],
        next_action: "n".into(),
        open_questions: vec![],
        plan_issue: None,
        repo_state: Some(RepoState {
            branch: "b".into(),
            base: "base".into(),
            head: "h1".into(),
            uncommitted: true,
            diff_stat: "1 file".into(),
        }),
        recent_activity: vec![],
        created_at: "2026-09-24T00:00:00Z".into(),
    };
    // 最初の checkpoint は常に進捗あり。
    assert!(checkpoint_shows_progress(None, &base));

    let mut same = base.clone();
    same.run_id = "r2".into();
    assert!(
        !checkpoint_shows_progress(Some(&base), &same),
        "何も変わっていなければ進捗なし"
    );

    let mut new_file = base.clone();
    new_file.files_changed.push(CheckpointFileChange {
        path: "b.rs".into(),
        change: "added".into(),
        note: None,
    });
    assert!(checkpoint_shows_progress(Some(&base), &new_file));

    let mut new_head = base.clone();
    new_head.repo_state.as_mut().unwrap().head = "h2".into();
    assert!(checkpoint_shows_progress(Some(&base), &new_head));

    let mut more_completed = base.clone();
    more_completed.completed.push("d".into());
    assert!(checkpoint_shows_progress(Some(&base), &more_completed));

    let mut less_remaining = base.clone();
    less_remaining.remaining.pop();
    assert!(checkpoint_shows_progress(Some(&base), &less_remaining));
}

#[test]
fn context_exceeded_phrases_are_recognized() {
    assert!(looks_like_context_exceeded(
        "Prompt is too long: 250000 tokens"
    ));
    assert!(looks_like_context_exceeded(
        "Error: context_length_exceeded"
    ));
    assert!(!looks_like_context_exceeded("wall clock exceeded"));
}

// ADR-0072 D16（Phase E4）: classify_review_failure の分類表。

fn command(cmd: &str, reason: &str) -> FailedCheck {
    FailedCheck {
        check: crate::model::Check::Command {
            cmd: cmd.to_string(),
            expect_exit: 0,
        },
        reason: reason.to_string(),
        repair_hint: None,
    }
}

fn reviewer(reason: &str, hint: Option<(&str, &str)>) -> FailedCheck {
    FailedCheck {
        check: crate::model::Check::Reviewer,
        reason: reason.to_string(),
        repair_hint: hint.map(|(scope, class)| ReviewRepairHint {
            scope: scope.to_string(),
            class: class.to_string(),
        }),
    }
}

#[test]
fn classifies_a_fmt_check_command_as_format() {
    let d = classify_review_failure(&[command(
        "cargo fmt --all -- --check",
        "cmd=\"cargo fmt --all -- --check\" exit=Some(1) expected=0",
    )]);
    assert_eq!(d, RepairDecision::Repairable(RepairClass::Format));
    assert_eq!(RepairClass::Format.bucket(), "format");
    assert_eq!(RepairClass::Format.budget(), (12, 600));
}

#[test]
fn classifies_a_clippy_command_as_lint() {
    let d = classify_review_failure(&[command(
        "cargo clippy --workspace -- -D warnings",
        "cmd=... exit=Some(1)",
    )]);
    assert_eq!(d, RepairDecision::Repairable(RepairClass::Lint));
    assert_eq!(RepairClass::Lint.budget(), (20, 1200));
}

#[test]
fn classifies_a_small_test_failure_as_test_small() {
    let d = classify_review_failure(&[command(
        "cargo test --workspace",
        "stdout_tail=\"test result: FAILED. 12 passed; 3 failed; 0 ignored\"",
    )]);
    assert_eq!(d, RepairDecision::Repairable(RepairClass::TestSmall));
    assert_eq!(RepairClass::TestSmall.budget(), (30, 1800));
}

#[test]
fn a_large_test_failure_is_not_test_small() {
    let d = classify_review_failure(&[command(
        "cargo test --workspace",
        "stdout_tail=\"test result: FAILED. 2 passed; 40 failed; 0 ignored\"",
    )]);
    assert_eq!(d, RepairDecision::Substantive);
}

#[test]
fn a_test_command_without_a_failed_count_is_substantive() {
    let d = classify_review_failure(&[command(
        "cargo test --workspace",
        "exec failed: no such file or directory",
    )]);
    assert_eq!(d, RepairDecision::Substantive);
}

#[test]
fn classifies_an_unrelated_command_as_substantive() {
    let d = classify_review_failure(&[command("./scripts/deploy.sh", "exit=Some(1)")]);
    assert_eq!(d, RepairDecision::Substantive);
}

#[test]
fn classifies_a_reviewer_repair_hint_as_reviewer_local() {
    let d = classify_review_failure(&[reviewer(
        "reviewer(r1): fmt is off",
        Some(("local", "format")),
    )]);
    assert_eq!(
        d,
        RepairDecision::Repairable(RepairClass::ReviewerLocal(ReviewerRepairKind::Format))
    );
    assert_eq!(
        RepairClass::ReviewerLocal(ReviewerRepairKind::Format).bucket(),
        "reviewer_local"
    );
}

#[test]
fn classifies_a_reviewer_reason_lexically_when_no_hint_is_given() {
    let d = classify_review_failure(&[reviewer(
        "reviewer(r1): please run cargo fmt before merging",
        None,
    )]);
    assert_eq!(
        d,
        RepairDecision::Repairable(RepairClass::ReviewerLocal(ReviewerRepairKind::Format))
    );
}

/// SD-2 追記（nextest の flake）: `reviewer(<run_id>)` や成果物のパスの ULID に `FMT` が
/// 含まれても、fmt の語としては読まない（中身の不合格は substantive のまま）。
#[test]
fn ulid_ids_in_a_reviewer_reason_are_not_read_as_fmt() {
    for reason in [
        // gate で実際に出た run id（`...ZFMTS...`）。
        "reviewer(01M3MA0HNXEZFMTSDF4BZ9HDFH): r",
        "reviewer(01M3M9XFFMTADBQ0865122NCCK): r",
        // 成果物のパスに入る task id（`...WXFMTS...`）。
        "reviewer(01M3M9HHREV0B8AK6C6WH5R66D): .taskd/artifacts/01M3M9HHKY97MQ5WXFMTS0W8SX/review.json is not a valid ReviewOutput: EOF while parsing a value at line 1 column 0",
    ] {
        assert_eq!(
            classify_review_failure(&[reviewer(reason, None)]),
            RepairDecision::Substantive,
            "{reason}"
        );
    }
    // id を外しても、reviewer 自身の文の `fmt` はこれまでどおり読む。
    assert_eq!(
        classify_review_failure(&[reviewer(
            "reviewer(01M3M9XFFMTADBQ0865122NCCK): run cargo fmt",
            None
        )]),
        RepairDecision::Repairable(RepairClass::ReviewerLocal(ReviewerRepairKind::Format))
    );
}

#[test]
fn without_ulid_tokens_keeps_everything_but_ulid_shaped_words() {
    assert_eq!(
        without_ulid_tokens("reviewer(01M3M9XFFMTADBQ0865122NCCK): a/01M3M9HHKY97MQ5WXFMTS0W8SX/b"),
        "reviewer(): a//b"
    );
    // 26 文字でも小文字・Crockford に無い文字（`L`/`I`/`O`/`U`）・先頭が 8 以上なら残す。
    for keep in [
        "01m3m9xffmtadbq0865122ncck",
        "01M3M9XFFMTADBQ0865122NCCL",
        "81M3M9XFFMTADBQ0865122NCCK",
        "01M3M9XFFMTADBQ0865122NCC",
        "日本語 fmt ✓",
    ] {
        assert_eq!(without_ulid_tokens(keep), keep);
    }
}

#[test]
fn a_reviewer_hint_with_a_non_local_scope_is_substantive() {
    let d = classify_review_failure(&[reviewer(
        "reviewer(r1): design is wrong",
        Some(("design", "other")),
    )]);
    assert_eq!(d, RepairDecision::Substantive);
}

#[test]
fn a_human_check_failure_is_always_substantive() {
    let d = classify_review_failure(&[FailedCheck {
        check: crate::model::Check::Human,
        reason: "rejected".to_string(),
        repair_hint: None,
    }]);
    assert_eq!(d, RepairDecision::Substantive);
}

#[test]
fn mixing_a_repairable_and_a_substantive_failure_is_substantive() {
    let d = classify_review_failure(&[
        command("cargo fmt --all -- --check", "stdout_tail=\"diff\""),
        FailedCheck {
            check: crate::model::Check::Human,
            reason: "needs a human call".to_string(),
            repair_hint: None,
        },
    ]);
    assert_eq!(d, RepairDecision::Substantive);
}

#[test]
fn build_repair_objective_does_not_include_the_full_original_objective() {
    let long_objective = "x".repeat(3000);
    let obj = build_repair_objective(
        RepairClass::Format,
        &["cmd=\"cargo fmt --check\" exit=Some(1)".to_string()],
        "some task",
        &long_objective,
        Some("1 file changed"),
        None,
    );
    assert!(!obj.contains(&long_objective));
    assert!(obj.contains(&"x".repeat(600)));
    assert!(!obj.contains(&"x".repeat(601)));
    assert!(obj.contains("cargo fmt --check"));
    assert!(obj.contains("1 file changed"));
}

/// ADR-0074 付記（2026-10-02）: `scope` が無ければ従来の出力と 1 バイトも変わらない。
#[test]
fn build_repair_objective_without_scope_matches_previous_output_exactly() {
    let without_none = build_repair_objective(
        RepairClass::Format,
        &["cmd=\"cargo fmt --check\" exit=Some(1)".to_string()],
        "some task",
        "do the thing",
        Some("1 file changed"),
        None,
    );
    let empty_scope = RepairScope {
        allowed_paths: vec![],
        scope_checks: vec![],
    };
    let with_empty_scope = build_repair_objective(
        RepairClass::Format,
        &["cmd=\"cargo fmt --check\" exit=Some(1)".to_string()],
        "some task",
        "do the thing",
        Some("1 file changed"),
        Some(&empty_scope),
    );
    assert_eq!(without_none, with_empty_scope);
    assert!(!without_none.contains("変更してよい範囲"));
    assert!(!without_none.contains("範囲外差分の検査"));
    assert!(!without_none.contains("plan_issue"));
}

/// ADR-0074 付記（2026-10-02）: 範囲があれば 2 節と `plan_issue` の指示が出る。
#[test]
fn build_repair_objective_with_scope_adds_allowed_range_and_out_of_scope_check_sections() {
    let scope = RepairScope {
        allowed_paths: vec![
            "web/".to_string(),
            "docs/adr/0099-root-delivery.md".to_string(),
        ],
        scope_checks: vec!["git diff --name-only <base> -- ':!web'".to_string()],
    };
    let obj = build_repair_objective(
        RepairClass::Format,
        &["cmd=\"cargo fmt --check\" exit=Some(1)".to_string()],
        "web task",
        "web だけを変える",
        None,
        Some(&scope),
    );
    assert!(obj.contains("## 変更してよい範囲"));
    assert!(obj.contains("- web/"));
    assert!(obj.contains("- docs/adr/0099-root-delivery.md"));
    assert!(obj.contains("## 範囲外差分の検査"));
    assert!(obj.contains("- git diff --name-only <base> -- ':!web'"));
    assert!(obj.contains("plan_issue"));
    assert!(obj.contains("範囲外のファイルを変えるな"));
}

#[test]
fn mixing_two_different_repairable_classes_is_substantive() {
    let d = classify_review_failure(&[
        command("cargo fmt --all -- --check", "diff"),
        command("cargo clippy -- -D warnings", "warning: ..."),
    ]);
    assert_eq!(d, RepairDecision::Substantive);
}

/// ADR-0072 D8 / ADR-0003 D6: 生成スキーマとコミット済みファイルの一致。`UPDATE_SCHEMA=1` で再生成。
#[test]
fn committed_schema_matches_generated() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/protocol/checkpoint.schema.json"
    );
    let generated = serde_json::to_string_pretty(&schema_value()).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e} (run with UPDATE_SCHEMA=1 to generate)"));
    assert_eq!(
        committed, generated,
        "schema drift: run `UPDATE_SCHEMA=1 cargo test -p task-core`"
    );
}

// ---- ADR-0120: IntegrationRepair ----

#[test]
fn integration_repair_class_bucket_title_and_key_round_trip() {
    assert_eq!(
        RepairClass::IntegrationConflict.bucket(),
        "integration_repair"
    );
    assert_eq!(RepairClass::IntegrationConflict.budget(), (30, 1800));
    assert_eq!(integration_repair_key(1), "integration-repair-1");
    assert!(is_integration_repair_unit(
        crate::WorkUnitKind::Repair,
        INTEGRATION_REPAIR_TITLE
    ));
    // 通常の ReviewRepair・repair 以外の kind は数えない。
    assert!(!is_integration_repair_unit(
        crate::WorkUnitKind::Repair,
        "repair (merge_conflict): merge"
    ));
    assert!(!is_integration_repair_unit(
        crate::WorkUnitKind::Implement,
        INTEGRATION_REPAIR_TITLE
    ));
    let units = [
        (crate::WorkUnitKind::Implement, "main"),
        (crate::WorkUnitKind::Repair, INTEGRATION_REPAIR_TITLE),
        (crate::WorkUnitKind::Repair, "repair (lint): clippy"),
        (crate::WorkUnitKind::Repair, INTEGRATION_REPAIR_TITLE),
    ];
    assert_eq!(count_integration_repairs(units), 2);
}

#[test]
fn integration_repair_objective_names_target_before_and_conflicts() {
    let files = vec![
        "src/b.rs".to_string(),
        "src/a.rs".to_string(),
        "src/b.rs".to_string(),
        " ".to_string(),
    ];
    let o = build_integration_repair_objective(
        "refs/heads/main",
        "tsha",
        "bsha",
        &files,
        "title",
        &"x".repeat(1000),
        None,
    );
    assert!(o.contains("`git rebase tsha`"));
    assert!(o.contains("- target_ref: refs/heads/main\n- target_sha: tsha"));
    assert!(o.contains("- before_sha: bsha"));
    assert!(o.contains("## 衝突したファイル\n- src/a.rs\n- src/b.rs\n\n"));
    assert!(o.contains("`git merge-base --is-ancestor tsha HEAD`"));
    assert!(o.contains("plan_issue"));
    assert!(!o.contains("## 変更してよい範囲"));
    assert!(!o.contains(&"x".repeat(601)));
    // 決定的（同じ入力は同じ出力）。
    let again = build_integration_repair_objective(
        "refs/heads/main",
        "tsha",
        "bsha",
        &files,
        "title",
        &"x".repeat(1000),
        Some(&RepairScope::default()),
    );
    assert_eq!(o, again);
    let scope = RepairScope {
        allowed_paths: vec!["crates/task-core/".into()],
        scope_checks: vec!["git diff --name-only".into()],
    };
    let scoped = build_integration_repair_objective(
        "refs/heads/main",
        "tsha",
        "bsha",
        &files,
        "title",
        "obj",
        Some(&scope),
    );
    assert!(scoped.contains("## 変更してよい範囲\n- crates/task-core/\n"));
    assert!(scoped.contains("## 範囲外差分の検査\n- git diff --name-only\n"));
}

#[test]
fn integration_repair_exhaust_reason_serde_names_are_fixed() {
    for (r, name) in [
        (
            IntegrationRepairExhaustReason::LimitReached,
            "limit_reached",
        ),
        (IntegrationRepairExhaustReason::PlanIssue, "plan_issue"),
        (
            IntegrationRepairExhaustReason::WorkUnitFailed,
            "work_unit_failed",
        ),
        (
            IntegrationRepairExhaustReason::BudgetExhausted,
            "budget_exhausted",
        ),
        (
            IntegrationRepairExhaustReason::ResultUntrusted,
            "result_untrusted",
        ),
        (IntegrationRepairExhaustReason::AbortFailed, "abort_failed"),
        (
            IntegrationRepairExhaustReason::WorktreeUnavailable,
            "worktree_unavailable",
        ),
    ] {
        assert_eq!(r.as_str(), name);
        assert_eq!(serde_json::to_value(r).unwrap(), serde_json::json!(name));
    }
}

fn scheduled(repo_id: crate::RepoId, wu: &str, attempt: u32, target: &str) -> crate::Event {
    crate::Event::IntegrationRepairScheduled {
        work_unit_id: wu.into(),
        key: integration_repair_key(attempt),
        repo_id,
        target_ref: "refs/heads/main".into(),
        target_sha: target.into(),
        before_sha: format!("before-{attempt}"),
        conflict_files: vec!["src/a.rs".into()],
        attempt,
    }
}

#[test]
fn integration_repair_events_serde_round_trip_and_type_names() {
    let repo_id = crate::RepoId::new();
    let events = [
        (
            scheduled(repo_id, "wu-1", 1, "t1"),
            "integration_repair_scheduled",
        ),
        (
            crate::Event::IntegrationRepairResolved {
                work_unit_id: "wu-1".into(),
                repo_id,
                target_sha: "t2".into(),
                reviewed_sha: "r".into(),
                attempt: 1,
            },
            "integration_repair_resolved",
        ),
        (
            crate::Event::IntegrationRepairExhausted {
                work_unit_id: None,
                repo_id,
                target_sha: "t".into(),
                before_sha: "b".into(),
                attempt: 3,
                reason: IntegrationRepairExhaustReason::LimitReached,
                rollback_to_sha: None,
                fallback: true,
            },
            "integration_repair_exhausted",
        ),
    ];
    for (e, name) in events {
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], serde_json::json!(name));
        let back: crate::Event = serde_json::from_value(v).unwrap();
        assert_eq!(back, e);
    }
    // 省略可能な欄は None なら出さない。
    let v = serde_json::to_value(crate::Event::IntegrationRepairExhausted {
        work_unit_id: None,
        repo_id,
        target_sha: "t".into(),
        before_sha: "b".into(),
        attempt: 1,
        reason: IntegrationRepairExhaustReason::AbortFailed,
        rollback_to_sha: None,
        fallback: false,
    })
    .unwrap();
    assert!(v.get("work_unit_id").is_none());
    assert!(v.get("rollback_to_sha").is_none());
    assert_eq!(v["reason"], serde_json::json!("abort_failed"));
}

#[test]
fn integration_repair_status_projects_last_event_with_its_scheduled_snapshot() {
    let repo_id = crate::RepoId::new();
    assert_eq!(integration_repair_status(&[]), None);

    let mut events = vec![scheduled(repo_id, "wu-1", 1, "t1")];
    let s = integration_repair_status(&events).unwrap();
    assert_eq!(s.state, IntegrationRepairState::Scheduled);
    assert_eq!(s.attempt, 1);
    assert_eq!(s.target_sha, "t1");
    assert_eq!(s.reason, None);
    assert_eq!(s.fallback, None);

    // resolved は再同期時の target_sha を返し、target_ref・before_sha は scheduled から引く。
    events.push(crate::Event::IntegrationRepairResolved {
        work_unit_id: "wu-1".into(),
        repo_id,
        target_sha: "t1b".into(),
        reviewed_sha: "r".into(),
        attempt: 1,
    });
    let s = integration_repair_status(&events).unwrap();
    assert_eq!(s.state, IntegrationRepairState::Resolved);
    assert_eq!(s.target_sha, "t1b");
    assert_eq!(s.target_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(s.before_sha.as_deref(), Some("before-1"));
    assert_eq!(s.conflict_files, vec!["src/a.rs".to_string()]);

    events.push(scheduled(repo_id, "wu-2", 2, "t2"));
    events.push(crate::Event::IntegrationRepairExhausted {
        work_unit_id: Some("wu-2".into()),
        repo_id,
        target_sha: "t2".into(),
        before_sha: "before-2".into(),
        attempt: 2,
        reason: IntegrationRepairExhaustReason::WorkUnitFailed,
        rollback_to_sha: Some("before-2".into()),
        fallback: true,
    });
    let s = integration_repair_status(&events).unwrap();
    assert_eq!(s.state, IntegrationRepairState::Exhausted);
    assert_eq!(s.work_unit_id.as_deref(), Some("wu-2"));
    assert_eq!(
        s.reason,
        Some(IntegrationRepairExhaustReason::WorkUnitFailed)
    );
    assert_eq!(s.rollback_to_sha.as_deref(), Some("before-2"));
    assert_eq!(s.fallback, Some(true));
    assert_eq!(s.target_ref.as_deref(), Some("refs/heads/main"));

    let snaps = integration_repair_snapshots(&events);
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps["wu-1"].target_sha, "t1");
    assert_eq!(snaps["wu-2"].before_sha, "before-2");
}
