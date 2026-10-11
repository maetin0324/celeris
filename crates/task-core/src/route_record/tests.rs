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
    assert_eq!(TASK_BRIEF_TEMPLATE, "task-brief/1");
    assert_eq!(
        f.brief,
        "# 要求\nタイトル: T\n目的: O\n\n# 期待する成果物\n受け入れ条件:\n決定的な検査なし\nchecks:\n決定的な検査なし\ndone_when:\nなし\noutputs:\nなし\n\n# 触る範囲\n予定 path:\n範囲の記述なし\nrepo:\n範囲の記述なし\n書き込み集合:\n範囲の記述なし\n\n# 置かれた位置\n- genre: 不明\n- kind: 不明\n- harness: 不明\n- lane: 不明\n- depth: 不明\n- phase_index: 不明\n- deps_n: 不明\n\n# 過去の試行\n初回\n\n# 性質（既存の判定）\n不明"
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
        "sha256:93bb770adef2c18cff2021636e3de31ca67d890626c023392251c8d4374f569c"
    );
}
