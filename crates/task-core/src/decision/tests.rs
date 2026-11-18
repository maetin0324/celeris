use super::*;

fn opt(key: &str) -> DecisionOption {
    DecisionOption {
        key: key.into(),
        label: format!("label {key}"),
        consequence: None,
    }
}

pub(crate) fn spec(key: &str, needed_before: &[&str]) -> DecisionSpec {
    DecisionSpec {
        key: key.into(),
        question: format!("question {key}?"),
        options: vec![opt("a"), opt("b")],
        recommended: "a".into(),
        cost_of_reversal: CostOfReversal::Medium,
        cost_note: None,
        needed_before: needed_before.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn shape_accepts_a_well_formed_decision() {
    assert!(validate_shape(&spec("h1", &["p2"])).is_empty());
}

#[test]
fn shape_rejects_bad_options_recommended_and_missing_needed_before() {
    let mut s = spec("H1", &[]);
    s.options = vec![opt("a")];
    s.recommended = "zzz".into();
    let errs = validate_shape(&s);
    assert!(
        errs.iter()
            .any(|e| matches!(e, DecisionShapeError::InvalidKey { .. }))
    );
    assert!(
        errs.iter()
            .any(|e| matches!(e, DecisionShapeError::OptionCount { count: 1, .. }))
    );
    assert!(
        errs.iter()
            .any(|e| matches!(e, DecisionShapeError::UnknownRecommended { .. }))
    );
    assert!(
        errs.iter()
            .any(|e| matches!(e, DecisionShapeError::NoNeededBefore { .. }))
    );

    let mut six = spec("h2", &["x"]);
    six.options = (0..6).map(|i| opt(&format!("o{i}"))).collect();
    six.recommended = "o0".into();
    assert!(matches!(
        validate_shape(&six).as_slice(),
        [DecisionShapeError::OptionCount { count: 6, .. }]
    ));

    let mut dup = spec("h3", &["x"]);
    dup.options = vec![opt("a"), opt("a")];
    assert!(matches!(
        validate_shape(&dup).as_slice(),
        [DecisionShapeError::DuplicateOptionKey { .. }]
    ));
}

/// 回答・取り下げの畳み込み（store と replay が同じ関数を通る）。
#[test]
fn row_folds_answer_and_withdrawal() {
    let root = TaskId::new();
    let req = DecisionRequest {
        id: "01D".into(),
        key: "h1".into(),
        kind: DecisionKind::Choice,
        question: "q".into(),
        options: vec![opt("a"), opt("b")],
        recommended: "a".into(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: None,
        needed_before: vec!["p2".into()],
        path: vec![DecisionPathEntry {
            task_id: root,
            title: "root".into(),
            stage: Some("phase-2".into()),
            unit: None,
        }],
        raised_by: DecisionRaisedBy {
            task_id: root,
            run_id: None,
            origin: DecisionOrigin::Planner,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    let mut row = DecisionRow::from_request(root, &req, "t0");
    assert_eq!(row.root_id, root);
    assert_eq!(row.status, DecisionStatus::Open);
    row.apply_answer("b", Some("n"), "human", "t1");
    assert_eq!(row.status, DecisionStatus::Answered);
    assert_eq!(row.answered_at.as_deref(), Some("t1"));
    assert_eq!(
        row.request.answer.as_ref().map(|a| a.option.as_str()),
        Some("b")
    );
    row.apply_withdrawal("superseded");
    assert_eq!(row.status, DecisionStatus::Withdrawn);
    assert_eq!(row.request.withdrawn_reason.as_deref(), Some("superseded"));
}

fn request(kind: DecisionKind, options: &[&str], recommended: &str) -> DecisionRequest {
    DecisionRequest {
        id: "01D".into(),
        key: "h1".into(),
        kind,
        question: "which backend".into(),
        options: options.iter().map(|k| opt(k)).collect(),
        recommended: recommended.into(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: None,
        needed_before: vec!["p2".into()],
        path: Vec::new(),
        raised_by: DecisionRaisedBy {
            task_id: TaskId::new(),
            run_id: None,
            origin: DecisionOrigin::Planner,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

/// R3a: 選択肢 → 効き目の表（ADR-0079 付記 R3a の表と同じ）。
#[test]
fn answer_effect_table() {
    use DecisionEffect::*;
    use DecisionKind::*;
    let table = [
        (Choice, "anything", Resume),
        (Choice, FREE_TEXT_OPTION, Resume),
        (Choice, "replan", Resume),
        (LeafTooLarge, "run-as-leaf", Resume),
        (LeafTooLarge, "replan", Replan),
        (LeafTooLarge, "withdraw", Withdraw),
        (Limit, "raise-once", RaiseOnce),
        (Limit, "replan", Replan),
        (Limit, "withdraw", Withdraw),
        (PlanInvalid, "replan", Replan),
        (PlanInvalid, "human-plan", Replan),
        (PlanInvalid, "atomic", Atomic),
        (PlanInvalid, "cancel", Withdraw),
        (PlanInvalid, "withdraw", Withdraw),
    ];
    for (kind, option, effect) in table {
        assert_eq!(answer_effect(kind, option), effect, "{kind:?} {option}");
    }
}

/// R3a: 回答の検証（選択肢の中・自由記述は choice で note があるときだけ・note の長さ）。
#[test]
fn validate_answer_checks_options_free_text_and_note() {
    let choice = request(DecisionKind::Choice, &["a", "b"], "a");
    assert_eq!(validate_answer(&choice, Some("b"), None).unwrap(), "b");
    assert!(matches!(
        validate_answer(&choice, Some("zzz"), None),
        Err(AnswerError::UnknownOption { .. })
    ));
    assert_eq!(
        validate_answer(&choice, None, Some("use the org vault")).unwrap(),
        FREE_TEXT_OPTION
    );
    assert!(matches!(
        validate_answer(&choice, None, Some("  ")),
        Err(AnswerError::OptionRequired { .. })
    ));
    let limit = request(DecisionKind::Limit, &["raise-once", "replan"], "replan");
    assert!(matches!(
        validate_answer(&limit, None, Some("free text")),
        Err(AnswerError::OptionRequired { .. })
    ));
    let long = "x".repeat(ANSWER_NOTE_MAX_CHARS + 1);
    assert!(matches!(
        validate_answer(&choice, Some("a"), Some(&long)),
        Err(AnswerError::NoteTooLong { .. })
    ));
}

/// R3a: 注入の固定の書式（子の objective と leaf の前置きが共有する 1 行）。
#[test]
fn answer_line_is_the_fixed_format() {
    let mut r = request(DecisionKind::Choice, &["a", "b"], "a");
    assert_eq!(answer_line(&r), None);
    r.answer = Some(DecisionAnswer {
        option: "b".into(),
        note: Some(" trial first ".into()),
        by: "human".into(),
    });
    assert_eq!(
        answer_line(&r).unwrap(),
        "- h1 which backend: label b（推奨と異なる） — trial first"
    );
    r.answer = Some(DecisionAnswer {
        option: "a".into(),
        note: None,
        by: "human".into(),
    });
    assert_eq!(
        answer_line(&r).unwrap(),
        "- h1 which backend: label a（推奨どおり）"
    );
    r.answer = Some(DecisionAnswer {
        option: FREE_TEXT_OPTION.into(),
        note: Some("neither".into()),
        by: "human".into(),
    });
    assert_eq!(
        answer_line(&r).unwrap(),
        "- h1 which backend: 自由記述（推奨と異なる） — neither"
    );
}

fn raw(key: &str, needed_before: &[&str]) -> serde_json::Value {
    serde_json::to_value(spec(key, needed_before)).unwrap()
}

/// R3a: worker の決定の検証（形・指す先・key の重複・`self` の書き換え）。
#[test]
fn worker_decisions_are_validated_and_self_is_resolved() {
    let units: BTreeSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
    let stages: BTreeSet<String> = ["s1"].iter().map(|s| s.to_string()).collect();
    let taken: BTreeMap<String, String> = [("h0".to_string(), "question h0?".to_string())].into();
    let items = vec![
        raw("w1", &["self"]),
        raw("w2", &["b", "stage:s1"]),
        raw("w3", &["nope"]),
        raw("h0", &["b"]),
        serde_json::json!({"key": "w5", "question": "q"}),
        raw("w1", &["b"]),
    ];
    let batch = prepare_worker_decisions(&items, Some("a"), &units, &stages, &taken, 8);
    assert_eq!(
        batch
            .accepted
            .iter()
            .map(|d| (d.key.as_str(), d.needed_before.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("w1", vec!["a".to_string()]),
            ("w2", vec!["b".to_string(), "stage:s1".to_string()]),
        ]
    );
    assert_eq!(batch.rejected.len(), 4, "{:?}", batch.rejected);
    assert!(batch.bundled.is_empty());
    // atomic の run: `self` のまま。unit は指せない。
    let atomic = prepare_worker_decisions(
        &[raw("w1", &["self"]), raw("w2", &["a"])],
        None,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeMap::new(),
        8,
    );
    assert_eq!(atomic.accepted.len(), 1);
    assert_eq!(atomic.accepted[0].needed_before, vec!["self".to_string()]);
    assert_eq!(worker_decisions_from_json(r#"{"summary":"s"}"#).len(), 0);
    assert_eq!(
        worker_decisions_from_json(r#"{"decisions":[{"key":"x"}]}"#).len(),
        1
    );
}

/// R3a: 上限を超えた worker の決定は 1 件に束ねる（先頭 cap − 1 件は残す）。
#[test]
fn worker_decisions_beyond_the_cap_are_bundled() {
    let units: BTreeSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    let taken: BTreeMap<String, String> = [(BUNDLE_KEY.to_string(), "q".to_string())].into();
    let mut high = spec("w3", &["c"]);
    high.cost_of_reversal = CostOfReversal::High;
    let items = vec![
        raw("w1", &["a"]),
        raw("w2", &["b"]),
        serde_json::to_value(high).unwrap(),
        raw("w4", &["a"]),
    ];
    let batch = prepare_worker_decisions(&items, None, &units, &BTreeSet::new(), &taken, 2);
    assert_eq!(batch.accepted.len(), 2);
    assert_eq!(batch.accepted[0].key, "w1");
    let bundle = &batch.accepted[1];
    assert_eq!(bundle.key, "bundle-2", "the plain key is taken");
    assert_eq!(batch.bundled, vec!["w2", "w3", "w4"]);
    assert_eq!(bundle.needed_before, vec!["b", "c", "a"]);
    assert_eq!(bundle.cost_of_reversal, CostOfReversal::High);
    assert!(
        bundle.question.contains("question w3?"),
        "{}",
        bundle.question
    );
    assert!(validate_shape(bundle).is_empty());
    // cap 0 でも 1 件には束ねる。cap 1 なら全部が 1 件の束になる。
    let zero = prepare_worker_decisions(
        &items[..2],
        None,
        &units,
        &BTreeSet::new(),
        &BTreeMap::new(),
        0,
    );
    assert_eq!(zero.accepted.len(), 1);
    assert_eq!(zero.accepted[0].key, BUNDLE_KEY);
    let one_over = prepare_worker_decisions(
        &items[..2],
        None,
        &units,
        &BTreeSet::new(),
        &BTreeMap::new(),
        1,
    );
    assert_eq!(
        one_over
            .accepted
            .iter()
            .map(|d| d.key.as_str())
            .collect::<Vec<_>>(),
        vec![BUNDLE_KEY]
    );
}

/// ADR 2026-11-18-rejected-worker-decisions-reach-human D1・D3: 受け付けなかった要求は 1 件の決定 `rejected` に
/// まとまり、`needed_before` は要求を出した仕事（leaf の key / atomic なら `self`）。本番の事象と同じく integrate の
/// unit 名（`needed_before: ["integrate-s1"]`）を指した要求、形の誤り・読めない要素・問いの違う key の重複が入り、
/// 問いが同じ key の重複（読み直し）は入らない。同じ問いのまとめが記録済みなら出し直さない。守る不変条件は
/// 「受け付けなかった要求が人に届く決定になる」こと。既存の 2 試験は `rejected` の数しか見ないので、これを検出できない。
#[test]
fn rejected_worker_decisions_become_one_self_decision() {
    // 計画の unit（Integrate の unit は呼び出し側が除く）と段階。
    let units: BTreeSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
    let stages: BTreeSet<String> = ["s1"].iter().map(|s| s.to_string()).collect();
    let taken: BTreeMap<String, String> = [
        ("h0".to_string(), "question h0?".to_string()),
        ("h1".to_string(), "an older question".to_string()),
        (REJECTED_KEY.to_string(), "an older summary".to_string()),
    ]
    .into();
    let mut high = spec("w2", &["b"]);
    high.options = vec![opt("a")];
    high.cost_of_reversal = CostOfReversal::High;
    let items = vec![
        raw("ok", &["b"]),
        raw("trim", &["integrate-s1"]),
        serde_json::to_value(high).unwrap(),
        serde_json::json!({"key": "w5", "question": "q5"}),
        raw("h0", &["b"]),
        raw("h1", &["b"]),
    ];
    let batch = prepare_worker_decisions(&items, Some("a"), &units, &stages, &taken, 8);
    assert_eq!(
        batch
            .accepted
            .iter()
            .map(|d| d.key.as_str())
            .collect::<Vec<_>>(),
        vec!["ok"]
    );
    assert_eq!(batch.rejected.len(), 5, "{:?}", batch.rejected);
    let e = batch
        .escalated
        .as_ref()
        .expect("the rejected ones reach a human");
    assert_eq!(e.key, "rejected-2", "the plain key is taken");
    assert_eq!(
        e.needed_before,
        vec!["a".to_string()],
        "holds the leaf itself"
    );
    assert_eq!(e.cost_of_reversal, CostOfReversal::High);
    assert!(validate_shape(e).is_empty(), "{:?}", validate_shape(e));
    for needle in [
        "決定の要求 4 件",
        "1) trim question trim?（選択肢: a=label a / b=label b。推奨: label a）",
        r#"needed_before names an unknown unit "integrate-s1""#,
        "2) w2 question w2?",
        "3) #4 w5 q5 — 受け付けなかった理由: decision #4:",
        "4) h1 question h1?",
    ] {
        assert!(e.question.contains(needle), "{needle:?} in {}", e.question);
    }
    assert!(
        !e.question.contains("h0 question h0?"),
        "the same key with the same question is a re-read: {}",
        e.question
    );
    assert_eq!(
        e.cost_note.as_deref(),
        Some("受け付けなかった決定: trim, w2, #4, h1")
    );
    // atomic の run: `self` のまま。
    let atomic = prepare_worker_decisions(
        &items[1..2],
        None,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeMap::new(),
        8,
    );
    let ae = atomic.escalated.expect("atomic run");
    assert_eq!(ae.key, REJECTED_KEY);
    assert_eq!(ae.needed_before, vec![NEEDED_BEFORE_SELF.to_string()]);
    assert_eq!(ae.cost_of_reversal, CostOfReversal::Medium);
    // 同じ問いのまとめが記録済み（古い `result.json` の読み直し）なら出し直さない。
    let recorded: BTreeMap<String, String> = [(ae.key.clone(), ae.question.clone())].into();
    let again = prepare_worker_decisions(
        &items[1..2],
        None,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &recorded,
        8,
    );
    assert_eq!(again.rejected.len(), 1);
    assert!(again.escalated.is_none());
    // 拒否が無ければ（受け付けた・束ねただけなら）まとめは無い。
    let clean = prepare_worker_decisions(&items[..1], Some("a"), &units, &stages, &taken, 0);
    assert!(clean.escalated.is_none());
    assert!(clean.rejected.is_empty());
}

/// D7 / R1a (e): `decision.schema.json` の生成スキーマとコミット済みファイルの一致。
#[test]
fn decision_committed_schema_matches_generated() {
    crate::assert_committed_schema("protocol/decision.schema.json", &schema_value());
}
