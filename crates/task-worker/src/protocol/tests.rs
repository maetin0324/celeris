use super::*;

#[test]
fn worker_message_roundtrip_and_unknown_fields_ignored() {
    let line = r#"{"type":"done","summary":"s","evidence":[{"criterion":0,"command":"true","exit":0,"stdout_tail":""}],"extra":1}"#;
    let m: WorkerMessage = serde_json::from_str(line).unwrap();
    assert!(m.is_terminal());
    match &m {
        WorkerMessage::Done {
            summary,
            evidence,
            usage,
        } => {
            assert_eq!(summary, "s");
            assert_eq!(evidence.len(), 1);
            assert!(usage.is_none());
        }
        _ => panic!("expected done"),
    }
    let back = serde_json::to_string(&m).unwrap();
    assert!(back.starts_with(r#"{"type":"done""#));
    assert!(!back.contains("usage"));
}

#[test]
fn unknown_type_is_a_parse_error_and_missing_required_is_error() {
    assert!(serde_json::from_str::<WorkerMessage>(r#"{"type":"bogus"}"#).is_err());
    assert!(serde_json::from_str::<WorkerMessage>(r#"{"type":"error","message":"m"}"#).is_err());
    assert!(
        !serde_json::from_str::<WorkerMessage>(r#"{"type":"progress","msg":"m"}"#)
            .unwrap()
            .is_terminal()
    );
}

/// ADR-0048 D2（Phase 60a）: `progress` の構造化フィールドは**任意**。従来の `{"type":"progress","msg":…}`
/// はそのまま読め（全て `None` / `false`）、書き戻しても余分な鍵は出ない。
#[test]
fn progress_accepts_both_the_plain_and_the_structured_form() {
    let plain: WorkerMessage =
        serde_json::from_str(r#"{"type":"progress","msg":"working"}"#).expect("plain");
    let fields = plain.progress_fields().expect("progress");
    assert!(fields.is_plain());
    assert_eq!(
        serde_json::to_string(&plain).expect("ser"),
        r#"{"type":"progress","msg":"working"}"#
    );

    let line = r#"{"type":"progress","msg":"tool_use: Bash …","kind":"tool_use","tool":"Bash",
            "summary":"cargo test --workspace","detail":"{\"command\":\"cargo test --workspace\"}","truncated":true,"error":false}"#;
    let structured: WorkerMessage = serde_json::from_str(line).expect("structured");
    let fields = structured.progress_fields().expect("progress");
    assert!(!fields.is_plain());
    assert_eq!(fields.kind, Some(ProgressKind::ToolUse));
    assert_eq!(fields.tool.as_deref(), Some("Bash"));
    assert_eq!(fields.summary.as_deref(), Some("cargo test --workspace"));
    assert!(fields.truncated);
    assert!(!fields.error);
    assert!(!structured.is_terminal());

    // 知らない `kind` の行は**行ごと**読めない（unknown type と同じ扱い）。混在は `msg` で救う。
    assert!(
        serde_json::from_str::<WorkerMessage>(r#"{"type":"progress","msg":"m","kind":"bogus"}"#)
            .is_err()
    );

    // 組み立て側（`WorkerMessage::progress`）と対称。
    let built = WorkerMessage::progress(
        "tool_result: ok",
        ProgressFields::of(ProgressKind::ToolResult)
            .with_summary("ok")
            .with_error(true),
    );
    let json = serde_json::to_string(&built).expect("ser");
    assert!(json.contains(r#""kind":"tool_result""#), "{json}");
    assert!(json.contains(r#""error":true"#), "{json}");
    assert!(!json.contains("truncated"), "{json}");
    assert_eq!(
        serde_json::from_str::<WorkerMessage>(&json).expect("round trip"),
        built
    );
}

/// ADR-0012 D3（P-12）: コマンドを伴わない条件の evidence は `criterion` だけでよく、旧形式（全フィールドあり）も読める。
#[test]
fn evidence_fields_other_than_criterion_are_optional() {
    let line = r#"{"type":"done","summary":"s","evidence":[{"criterion":1},{"criterion":0,"command":"cargo test","exit":0,"stdout_tail":"ok"}]}"#;
    let WorkerMessage::Done { evidence, .. } = serde_json::from_str::<WorkerMessage>(line).unwrap()
    else {
        panic!("expected done");
    };
    assert_eq!(
        evidence[0],
        Evidence {
            criterion: 1,
            command: None,
            exit: None,
            stdout_tail: None
        }
    );
    assert_eq!(evidence[1].command.as_deref(), Some("cargo test"));
    assert_eq!(evidence[1].exit, Some(0));
    assert_eq!(
        serde_json::to_string(&evidence[0]).unwrap(),
        r#"{"criterion":1}"#
    );
}

/// ADR-0016 D2: `delegate` は非終端で、`tasks` は `DelegateTask`。`depends_on` は整数と ID 文字列を混ぜられる。
#[test]
fn delegate_message_parses_and_is_not_terminal() {
    let line = r#"{"type":"delegate","tasks":[{"title":"a","objective":"o","acceptance":[{"text":"c","check":{"type":"human"}}],"role":"implementer","genre":"coding"},
            {"title":"b","objective":"o","acceptance":[{"text":"c","check":{"type":"command","cmd":"true","expect_exit":0}}],"depends_on":[0]}]}"#;
    let m: WorkerMessage = serde_json::from_str(line).unwrap();
    assert!(!m.is_terminal());
    let WorkerMessage::Delegate { tasks } = m else {
        panic!("expected delegate")
    };
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].role.as_deref(), Some("implementer"));
    // ADR-0027 D1: `genre` は任意。
    assert_eq!(tasks[0].genre.as_deref(), Some("coding"));
    assert_eq!(tasks[1].genre, None);
    assert_eq!(tasks[1].depends_on, vec![task_core::DelegateDep::Index(0)]);
}

#[test]
fn run_request_serializes_with_type_tag() {
    let req = RunRequest {
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: sample_task(),
        workspace: PathBuf::from("/tmp/ws"),
        work_dir: None,
        artifacts_dir: PathBuf::from("/tmp/ws/artifacts"),
        context: RunContext::default(),
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["type"], "run");
    assert_eq!(v["protocol"], 4);
    assert_eq!(v["task"]["kind"], "execute");
    let back: RunRequest = serde_json::from_value(v).unwrap();
    assert_eq!(back, req);
}

/// ADR-0027 D1: `available_genres` は空なら省略され、非空なら分野と役割の一覧が乗る。
#[test]
fn available_genres_round_trips_and_is_omitted_when_empty() {
    let empty = RunContext::default();
    let v = serde_json::to_value(&empty).unwrap();
    assert!(v.get("available_genres").is_none());

    let genres = [task_core::GenreSpec {
        id: "literature".into(),
        description: "related work survey".into(),
        capabilities: vec![
            "academic literature search".into(),
            "citation graph traversal".into(),
        ],
        input_artifacts: vec!["question".into(), "pdf".into()],
        output_artifacts: vec!["answer.md".into(), "citations.json".into()],
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-scout".into(), "literature-reader".into()],
    }];
    let context = RunContext {
        available_genres: genres.iter().map(GenreContext::from).collect(),
        ..RunContext::default()
    };
    let v = serde_json::to_value(&context).unwrap();
    assert_eq!(v["available_genres"][0]["id"], "literature");
    assert_eq!(
        v["available_genres"][0]["roles"][1]["id"],
        "literature-reader"
    );
    assert_eq!(
        v["available_genres"][0]["capabilities"][1],
        "citation graph traversal"
    );
    assert_eq!(
        v["available_genres"][0]["input_artifacts"],
        serde_json::json!(["question", "pdf"])
    );
    assert_eq!(
        v["available_genres"][0]["output_artifacts"],
        serde_json::json!(["answer.md", "citations.json"])
    );
    let back: RunContext = serde_json::from_value(v).unwrap();
    assert_eq!(back, context);

    // 空なら 3 フィールドとも省略される（既存設定との互換）。
    let bare_genre = task_core::GenreSpec {
        id: "coding".into(),
        description: "write and fix code".into(),
        ..task_core::GenreSpec::default()
    };
    let bare_json = serde_json::to_value(GenreContext::from(&bare_genre)).unwrap();
    assert!(bare_json.get("capabilities").is_none());
    assert!(bare_json.get("input_artifacts").is_none());
    assert!(bare_json.get("output_artifacts").is_none());
}

/// Phase 38（ADR-0028 追記）: `harness`（`default_role` の役割のアダプタ）と `subject_genre` は
/// 追加のみのフィールドで、無ければ JSON に出ない（旧ワーカー互換）。`名前: 説明` は名前と説明に分かれる。
#[test]
fn harness_and_subject_genre_are_optional_additions() {
    let roles = vec![task_core::RoleSpec {
        id: "literature-reader".into(),
        adapter: Some("paperqa".into()),
        ..task_core::RoleSpec::default()
    }];
    let spec = task_core::GenreSpec {
        id: "literature".into(),
        description: "related work".into(),
        output_artifacts: vec!["answer.md: 引用付きの答え".into(), "papers.json".into()],
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-reader".into()],
        ..task_core::GenreSpec::default()
    };
    // 役割を渡さずに組むと `harness` は付かない（従来の `From<&GenreSpec>`）。
    let bare = serde_json::to_value(GenreContext::from(&spec)).unwrap();
    assert!(bare.get("harness").is_none(), "{bare}");

    let genre = GenreContext::from_spec(&spec, &roles);
    assert_eq!(genre.harness.as_deref(), Some("paperqa"));
    assert!(genre.is_harness());
    assert_eq!(
        genre.output_artifacts_named(),
        vec![("answer.md", Some("引用付きの答え")), ("papers.json", None)]
    );

    let empty = serde_json::to_value(RunContext::default()).unwrap();
    assert!(empty.get("subject_genre").is_none(), "{empty}");
    let context = RunContext {
        subject_genre: Some(genre),
        ..RunContext::default()
    };
    let json = serde_json::to_value(&context).unwrap();
    assert_eq!(json["subject_genre"]["harness"], "paperqa");
    assert_eq!(
        json["subject_genre"]["output_artifacts"][0],
        "answer.md: 引用付きの答え"
    );
    let back: RunContext = serde_json::from_value(json).unwrap();
    assert_eq!(back, context);
}

/// ADR-0003 D6: 生成スキーマとコミット済みファイルの一致。`UPDATE_SCHEMA=1` で再生成する。
#[test]
fn committed_schema_matches_generated() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/protocol/worker-protocol.schema.json"
    );
    let generated = serde_json::to_string_pretty(&schema_value()).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e} (run with UPDATE_SCHEMA=1 to generate)"));
    assert_eq!(
        committed, generated,
        "schema drift: run `UPDATE_SCHEMA=1 cargo test -p task-worker`"
    );
}

pub(crate) fn sample_task() -> Task {
    use task_core::*;
    let now = time::OffsetDateTime::now_utc();
    Task {
        expected_write_paths: None,
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Running,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from("/tmp/ws"),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 60,
            max_retries: 1,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}
