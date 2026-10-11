use super::*;

#[test]
fn route_record_repeated_input_is_byte_identical() {
    let input = FeaturesInput {
        title: Some("題名".into()),
        objective: Some("目的".into()),
        genre: Some("coding".into()),
        prior_runs_n: Some(0),
        ..Default::default()
    };
    let a = serde_json::to_vec(&build_task_features(&input)).unwrap();
    let b = serde_json::to_vec(&build_task_features(&input)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn route_record_truncates_and_marks_original_size() {
    let input = FeaturesInput {
        objective: Some("x".repeat(OBJECTIVE_MAX_CHARS + 10)),
        ..Default::default()
    };
    let f = build_task_features(&input);
    assert!(f.brief.contains("…（"));
    assert_eq!(f.aux.objective_chars, Some(OBJECTIVE_MAX_CHARS + 10));
    assert_eq!(f.aux.truncated[0].original_chars, OBJECTIVE_MAX_CHARS + 10);
}

#[test]
fn route_record_missing_values_remain_null_and_are_listed() {
    let f = build_task_features(&FeaturesInput::default());
    assert!(f.aux.genre.is_none());
    assert!(f.aux.missing.contains(&"objective".into()));
    assert!(f.aux.missing.contains(&"genre".into()));
    assert!(f.brief.contains("（目的の記述なし）"));
}

#[test]
fn route_record_secret_replacement_covers_token_and_key_shapes() {
    assert_eq!(
        redact_secrets("Bearer abcdefghijkl sk-live_1234567890 api_key=super-secret"),
        "Bearer <redacted> <redacted> api_key=<redacted>"
    );
    let f = build_task_features(&FeaturesInput {
        objective: Some("token=objective-secret".into()),
        ..Default::default()
    });
    assert!(!f.brief.contains("objective-secret"));
}

#[test]
fn route_record_option_id_is_text_sha256_prefix_and_binds_candidate() {
    let c = Candidate {
        source_id: "s".into(),
        model_id: "m".into(),
        harness_id: "h".into(),
        label: "Choose model M".into(),
    };
    let options = candidates_one_shot("Choose", &[c]);
    assert_eq!(
        options.options[0].option_id,
        &digest(b"Choose model M")[..16]
    );
    assert_eq!(options.options[0].binds.as_ref().unwrap().model_id, "m");
}

#[test]
fn route_record_schema_pinned() {
    let f = build_task_features(&FeaturesInput {
        title: Some("T".into()),
        objective: Some("O".into()),
        prior_runs_n: Some(0),
        ..Default::default()
    });
    assert_eq!(TASK_FEATURES_SCHEMA, "celeris.task-features/1");
    assert_eq!(ROUTE_OPTIONS_SCHEMA, "celeris.route-options/1");
    assert_eq!(ROUTE_DECISION_SCHEMA, "celeris.route-decision/1");
    assert_eq!(ROUTE_OUTCOME_SCHEMA, "celeris.route-outcome/1");
    assert_eq!(TASK_BRIEF_TEMPLATE, "task-brief/2");
    assert_eq!(
        f.brief,
        "# 要求\nタイトル: T\n目的: O\n\n# 期待する成果物\n受け入れ条件:\n決定的な検査なし\nchecks:\n決定的な検査なし\ndone_when:\nなし\noutputs:\nなし\n\n# 触る範囲\n予定 path:\n範囲の記述なし\nrepo:\n範囲の記述なし\n書き込み集合:\n範囲の記述なし\n\n# 置かれた位置\n- genre: 不明\n- kind: 不明\n- harness: 不明\n- lane: 不明\n- depth: 不明\n- phase_index: 不明\n- deps_n: 不明\n\n# 過去の試行\n初回\nrepair: 不明"
    );
    assert_eq!(
        f.aux,
        FeaturesAux {
            objective_chars: Some(1),
            objective_truncated: Some(false),
            prior_runs_n: Some(0),
            missing: vec![
                "acceptance".into(),
                "checks".into(),
                "done_when".into(),
                "outputs".into(),
                "paths".into(),
                "repos".into(),
                "write_set".into(),
                "prior_runs".into(),
                "genre".into(),
                "kind".into(),
                "harness".into(),
                "lane".into(),
                "depth".into(),
                "phase_index".into(),
                "deps_n".into(),
                "acceptance_n".into(),
                "check_command_n".into(),
                "check_artifact_n".into(),
                "check_reviewer_n".into(),
                "check_human_n".into(),
                "context_paths_n".into(),
                "crates_n".into(),
                "repos_n".into(),
                "prior_failed_n".into(),
                "retries_n".into(),
                "repairs_n".into(),
                "budget_turns".into(),
                "budget_wall_s".into(),
                "task_features_v69".into()
            ],
            ..Default::default()
        }
    );
    assert_eq!(
        f.hash,
        "sha256:49393ff873e73bfb8cc9b99e56549723ca6cf89a6354af34bb023cb6609db15a"
    );
}

#[test]
fn route_record_prior_runs_render_one_line_per_run() {
    let f = build_task_features(&FeaturesInput {
        prior_runs_n: Some(2),
        prior_failed_n: Some(1),
        repairs_n: Some(1),
        prior_runs: Some(vec![
            PriorRunInput {
                status: "failed".into(),
                model: Some("model-a".into()),
                wall_s: Some(120),
                failed_criteria: vec![FailedCriterionInput {
                    text: "workspace 試験が通る".into(),
                    exit: Some(101),
                }],
            },
            PriorRunInput {
                status: "budget_exhausted".into(),
                model: None,
                wall_s: None,
                failed_criteria: vec![],
            },
        ]),
        ..Default::default()
    });
    assert!(f.brief.contains(
        "# 過去の試行\n前の run: 2 回（うち失敗 1）\n- run 1: failed、model: model-a、壁時計: 120 秒、落ちた検査: 「workspace 試験が通る」(exit 101)\n- run 2: budget_exhausted、model: 不明、壁時計: 不明、落ちた検査: なし\nrepair: 1 件"
    ));
    assert!(!f.aux.missing.contains(&"prior_runs".into()));

    let first = build_task_features(&FeaturesInput {
        prior_runs_n: Some(0),
        prior_runs: Some(vec![]),
        repairs_n: Some(0),
        ..Default::default()
    });
    assert!(first.brief.contains("# 過去の試行\n初回\nrepair: 0 件"));
}

#[test]
fn route_record_nature_renders_fixed_axis_sentences_or_is_omitted() {
    use crate::model_policy::{Level, TaskFeatures as LaneFeatures};
    let f = build_task_features(&FeaturesInput {
        task_features_v69: Some(LaneFeatures {
            judgment: Level::Medium,
            ambiguity: Level::Low,
            verifiability: Level::High,
            reversibility: Level::High,
            consequence: Level::Medium,
            context_size: Level::High,
            tool_intensity: Level::Medium,
            expected_length: Level::Low,
            cross_cutting: Level::Low,
        }),
        ..Default::default()
    });
    assert!(f.brief.ends_with(
        "# 性質（既存の判定）\n- 判断の重さ: 中\n- 要求の曖昧さ: 低\n- 決定的な検証のしやすさ: 高\n- やり直しやすさ: 高\n- 失敗したときの損失: 中\n- 読むべき文脈の大きさ: 高\n- 道具を使う度合い: 中\n- 想定される長さ: 低\n- 領域をまたぐ度合い: 低"
    ));
    assert!(!f.brief.contains('{'));
    let none = build_task_features(&FeaturesInput::default());
    assert!(!none.brief.contains("# 性質"));
}

#[test]
fn route_record_checks_render_kind_and_command_name() {
    use crate::execution_plan::WorkUnitCheck;
    use crate::model::{Check, Criterion};
    let criteria = [
        Criterion {
            text: "試験が通る".into(),
            check: Check::Command {
                cmd: "cargo test -p task-core --lib".into(),
                expect_exit: 0,
            },
        },
        Criterion {
            text: "報告がある".into(),
            check: Check::ArtifactExists {
                name: "report.md".into(),
            },
        },
        Criterion {
            text: "KB".into(),
            check: Check::KnowledgePage {
                path: "a.md".into(),
            },
        },
        Criterion {
            text: "目で見る".into(),
            check: Check::Reviewer,
        },
        Criterion {
            text: "人".into(),
            check: Check::Human,
        },
    ];
    let wu_check: WorkUnitCheck = serde_json::from_value(
        serde_json::json!({"cmd": "CI=1 bash scripts/dev/test-parallel.sh"}),
    )
    .unwrap();
    let f = build_task_features(&FeaturesInput {
        acceptance: Some(criteria.iter().map(AcceptanceInput::from).collect()),
        checks: Some(vec![CheckInput::from(&wu_check)]),
        ..Default::default()
    });
    assert!(f.brief.contains(
        "受け入れ条件:\n- 試験が通る（検査: command `cargo test`）\n- 報告がある（検査: artifact_exists）\n- KB（検査: knowledge_page）\n- 目で見る（検査: reviewer）\n- 人（検査: human）\nchecks:\n- command `bash`\n"
    ));
}

#[test]
fn route_record_redact_large_input_replaces_every_secret() {
    let unit = "あいう token=abc sk-abcdefghijkl Bearer abcdefghij 平文 ";
    let redacted = "あいう token=<redacted> <redacted> Bearer <redacted> 平文 ";
    let n = 200_000 / unit.chars().count() + 1;
    let input = unit.repeat(n);
    assert!(input.chars().count() >= 200_000);
    let out = redact_secrets(&input);
    assert_eq!(out, redacted.repeat(n));
    assert_eq!(redact_secrets(&input), out);
}
