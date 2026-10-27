//! 生記録（human-ops JSONL）: 冪等追記・欠測を 0 にしない・A/B 同一 id・不正入力の拒否。
use std::{collections::BTreeMap, fs};

use celeris_campaign::{human_ops::*, ledger::TrialKey, manifest::ConfigId};

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn base(set: ExperimentSet, arm: ConfigId, rt: RecordType) -> HumanOpRecord {
    let key = TrialKey {
        manifest_sha256: HASH.into(),
        problem_id: "case1".into(),
        arm,
        trial_no: 1,
        rep: 1,
    };
    HumanOpRecord {
        schema: HUMAN_OP_SCHEMA.into(),
        record_id: String::new(),
        record_type: rt,
        experiment_set: set,
        campaign_id: "camp".into(),
        manifest_sha256: HASH.into(),
        case_id: Some("case1".into()),
        arm: Some(arm),
        repetition: Some(1),
        trial_no: Some(1),
        trial_key: Some(key.idempotency_key()),
        task_id: None,
        run_id: None,
        job_ref: None,
        logical_op_id: None,
        attempt: None,
        recorded_at: "2026-10-19T01:00:00Z".into(),
        clock_source: ClockSource::SystemUtc,
        host: "h1".into(),
        start: None,
        end: None,
        occurred_at: None,
        job_submit: None,
        job_start: None,
        job_end: None,
        time_basis: None,
        time_precision: None,
        missing: BTreeMap::new(),
        op_kind: None,
        mode: None,
        intent: None,
        actor: None,
        decided_by: None,
        executed_by: None,
        approved_by: None,
        credential: None,
        request_ref: None,
        request_revision: None,
        policy_ref: None,
        reason: None,
        trigger: None,
        targets: vec![],
        celeris_event_ref: None,
        outcome: None,
        wait_kind: None,
        result: None,
        corrects: None,
        set: None,
        note: None,
    }
}

fn op_start(arm: ConfigId, start: Option<&str>) -> HumanOpRecord {
    let mut r = base(ExperimentSet::MainAb, arm, RecordType::OpStart);
    r.logical_op_id = Some("p3/submit".into());
    r.attempt = Some(1);
    r.op_kind = Some(OpKind::Resubmit);
    r.mode = Some(Mode::Manual);
    r.intent = Some(Intent::System);
    r.actor = Some(Actor {
        kind: ActorKind::Human,
        id: "eval-1".into(),
    });
    r.credential = Some("web".into());
    r.reason = Some("sbatch が失敗したため出し直す".into());
    r.trigger = Some(Trigger::Failure);
    match start {
        Some(s) => {
            r.start = Some(s.into());
            r.time_basis = Some(TimeBasis::EvaluatorLive);
            r.time_precision = Some(TimePrecision::Second);
        }
        None => {
            r.missing.insert("start".into(), MissingReason::Unknown);
        }
    }
    r.with_record_id()
}

fn op_end(arm: ConfigId, end: Option<&str>) -> HumanOpRecord {
    let mut r = op_start(arm, Some("2026-10-19T01:00:00Z"));
    r.record_type = RecordType::OpEnd;
    r.start = None;
    r.missing.clear();
    r.outcome = Some(Outcome::Ok);
    match end {
        Some(e) => r.end = Some(e.into()),
        None => {
            r.missing.insert("end".into(), MissingReason::OpenAtCutoff);
            r.time_basis = None;
            r.time_precision = None;
        }
    }
    r.with_record_id()
}

fn log() -> (tempfile::TempDir, HumanOpLog) {
    let d = tempfile::tempdir().expect("tmp");
    let l = HumanOpLog::in_out_dir(d.path());
    (d, l)
}

fn lines(l: &HumanOpLog) -> usize {
    fs::read_to_string(l.path())
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

#[test]
fn duplicate_and_resume_do_not_add_lines() {
    let (_d, l) = log();
    let r = op_start(ConfigId::A, Some("2026-10-19T01:00:00Z"));
    assert_eq!(l.append(&r).unwrap(), AppendOutcome::Appended);
    assert_eq!(l.append(&r).unwrap(), AppendOutcome::Duplicate);
    // 再開: 別 process（新しい handle）・別の記録時刻/host でも増えない。
    let mut again = r.clone();
    again.recorded_at = "2026-10-19T05:00:00Z".into();
    again.host = "h2".into();
    again.note = None;
    assert_eq!(
        HumanOpLog::new(l.path()).append(&again).unwrap(),
        AppendOutcome::Duplicate
    );
    assert_eq!(lines(&l), 1);
    assert_eq!(l.read().unwrap().len(), 1);
}

#[test]
fn conflicting_body_is_rejected_not_overwritten() {
    let (_d, l) = log();
    let r = op_start(ConfigId::A, Some("2026-10-19T01:00:00Z"));
    l.append(&r).unwrap();
    let mut other = r.clone();
    other.start = Some("2026-10-19T02:00:00Z".into());
    assert!(matches!(
        l.append(&other),
        Err(HumanOpError::Conflict { .. })
    ));
    assert_eq!(lines(&l), 1);
    assert_eq!(
        l.read().unwrap()[0].start.as_deref(),
        Some("2026-10-19T01:00:00Z")
    );
}

#[test]
fn correction_replaces_fields_on_read_without_rewriting() {
    let (_d, l) = log();
    let r = op_start(ConfigId::A, None);
    l.append(&r).unwrap();
    let mut c = base(ExperimentSet::MainAb, ConfigId::A, RecordType::Correction);
    c.corrects = Some(r.record_id.clone());
    let mut set = serde_json::Map::new();
    set.insert("start".into(), "2026-10-19T00:50:00Z".into());
    set.insert("missing".into(), serde_json::json!({}));
    set.insert("time_basis".into(), "evaluator_reported".into());
    set.insert("time_precision".into(), "minute".into());
    c.set = Some(set);
    let c = c.with_record_id();
    l.append(&c).unwrap();
    assert_eq!(lines(&l), 2);
    let all = resolve(&l.read().unwrap()).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].start.as_deref(), Some("2026-10-19T00:50:00Z"));
    // 対象の無い訂正は追記しない。
    let mut bad = c.clone();
    bad.corrects = Some(format!("hop-{}", "0".repeat(64)));
    assert!(l.append(&bad.with_record_id()).is_err());
}

#[test]
fn missing_time_is_never_zero() {
    // 開始だけ・終了だけの op は残り、所要時間は欠測。
    let (_d, l) = log();
    l.append(&op_start(ConfigId::A, None)).unwrap();
    l.append(&op_end(ConfigId::A, Some("2026-10-19T01:05:00Z")))
        .unwrap();
    let spans = op_spans(&l.read().unwrap());
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].1.start, None);
    assert_eq!(spans[0].1.duration_secs(), None);
    let st = DurationStat::from_spans(&[spans[0].1.clone()]);
    assert_eq!(
        (st.observed_sum, st.observed, st.missing, st.total()),
        (None, 0, 1, None)
    );

    // 観測した 0 秒は 0、欠測が混ざれば合計は出さない。
    let zero = TimingSpan {
        start: Some("2026-10-19T01:00:00Z".into()),
        end: Some("2026-10-19T01:00:00Z".into()),
    };
    assert_eq!(zero.duration_secs(), Some(0));
    let st = DurationStat::from_spans(&[zero, TimingSpan::default()]);
    assert_eq!(
        (st.observed_sum, st.missing, st.total()),
        (Some(0), 1, None)
    );

    // null の時刻は JSON でも null のまま往復する。
    let r = op_start(ConfigId::A, None);
    let v = serde_json::to_value(&r).unwrap();
    assert!(v["start"].is_null());
    assert_eq!(v["missing"]["start"], "unknown");
    let back: HumanOpRecord = serde_json::from_value(v).unwrap();
    assert_eq!(back.start, None);
}

#[test]
fn null_time_requires_reason_and_omitted_field_is_format_error() {
    let mut r = op_start(ConfigId::A, None);
    r.missing.clear();
    assert!(r.validate().is_err());
    let mut v = serde_json::to_value(op_start(ConfigId::A, Some("2026-10-19T01:00:00Z"))).unwrap();
    v.as_object_mut().unwrap().remove("start");
    assert!(serde_json::from_value::<HumanOpRecord>(v).is_err());
}

#[test]
fn a_and_b_same_input_same_record_id() {
    // 材料に recorded_at・host・note は入らない。同じ事実は同じ id。
    let a1 = op_start(ConfigId::A, Some("2026-10-19T01:00:00Z"));
    let mut a2 = a1.clone();
    a2.recorded_at = "2026-10-20T00:00:00Z".into();
    a2.host = "other".into();
    a2.note = Some("memo".into());
    assert_eq!(a1.record_id, a2.with_record_id().record_id);
    // 手順が同一なら（arm の違いを除き）A/B で同じ検証を通り、arm が違えば id は別。
    let b = op_start(ConfigId::B, Some("2026-10-19T01:00:00Z"));
    assert_ne!(a1.record_id, b.record_id);
    assert!(a1.validate().is_ok() && b.validate().is_ok());
    assert_eq!(a1.arm_label(), Some("A0"));
    assert_eq!(b.arm_label(), Some("B0"));
    // 同じ入力を 2 回組んでも同じ。
    assert_eq!(
        b.record_id,
        op_start(ConfigId::B, Some("2026-10-19T01:00:00Z")).record_id
    );
    assert!(a1.record_id.starts_with("hop-") && a1.record_id.len() == 68);
}

#[test]
fn invalid_inputs_are_rejected() {
    let ok = op_start(ConfigId::A, Some("2026-10-19T01:00:00Z"));
    assert!(ok.validate().is_ok());
    let check = |f: &dyn Fn(&mut HumanOpRecord)| {
        let mut r = ok.clone();
        f(&mut r);
        r.record_id = r.compute_record_id();
        let (_d, l) = log();
        assert!(r.validate().is_err(), "{r:?}");
        assert!(l.append(&r).is_err());
        assert_eq!(lines(&l), 0);
    };
    check(&|r| r.op_kind = None);
    check(&|r| r.reason = None); // resubmit は reason 必須
    check(&|r| r.attempt = Some(0));
    check(&|r| r.start = Some("2026-10-19T01:00:00+09:00".into()));
    check(&|r| r.start = Some("not a time".into()));
    check(&|r| r.arm = Some(ConfigId::C)); // main-ab に C は無い
    check(&|r| r.trial_key = Some(format!("campaign-{}", "0".repeat(64))));
    check(&|r| r.case_id = None); // 試行の欄は揃えて書く
    check(&|r| {
        r.actor = Some(Actor {
            kind: ActorKind::Human,
            id: "a@b.example".into(),
        })
    });
    check(&|r| r.time_basis = None);
    // /2 の欄: credential 必須・語彙外は拒否、request_ref は両方か無しか・形式、役割の id にメール不可。
    check(&|r| r.credential = None);
    check(&|r| r.credential = Some("password".into()));
    check(&|r| r.credential = Some("operator:".into()));
    check(&|r| r.request_ref = Some("notice:k1".into()));
    check(&|r| {
        r.request_ref = Some("no-colon".into());
        r.request_revision = Some("1".into());
    });
    check(&|r| {
        r.decided_by = Some(Actor {
            kind: ActorKind::Human,
            id: "x@y.example".into(),
        })
    });
    check(&|r| r.end = Some("2026-10-19T00:00:00Z".into())); // op_start では使えない
    check(&|r| r.schema = "x".into());
    check(&|r| r.manifest_sha256 = "short".into());
    // 材料と record_id の食い違い。
    let mut bad = ok.clone();
    bad.record_id = format!("hop-{}", "1".repeat(64));
    assert!(bad.validate().is_err());
    // end < start（attempt）。
    let mut a = base(ExperimentSet::MainAb, ConfigId::A, RecordType::Attempt);
    a.logical_op_id = Some("x".into());
    a.attempt = Some(1);
    a.start = Some("2026-10-19T02:00:00Z".into());
    a.end = Some("2026-10-19T01:00:00Z".into());
    a.result = Some(AttemptResult::Failed);
    a.time_basis = Some(TimeBasis::EvaluatorReported);
    a.time_precision = Some(TimePrecision::Minute);
    assert!(a.with_record_id().validate().unwrap_err().contains("end"));
}

#[test]
fn broken_line_reports_line_number() {
    let (_d, l) = log();
    l.append(&op_start(ConfigId::A, Some("2026-10-19T01:00:00Z")))
        .unwrap();
    let mut t = fs::read_to_string(l.path()).unwrap();
    t.push_str("{\"schema\": broken\n");
    fs::write(l.path(), &t).unwrap();
    match l.read() {
        Err(HumanOpError::Line { line, .. }) => assert_eq!(line, 2),
        o => panic!("{o:?}"),
    }
    // 壊れた file には追記しない。
    assert!(
        l.append(&op_end(ConfigId::A, Some("2026-10-19T01:05:00Z")))
            .is_err()
    );
}

#[test]
fn attempts_and_redo_counts() {
    let (_d, l) = log();
    for n in 1..=3u32 {
        let mut a = base(ExperimentSet::MainAb, ConfigId::A, RecordType::Attempt);
        a.logical_op_id = Some("p3/train".into());
        a.attempt = Some(n);
        a.result = Some(if n < 3 {
            AttemptResult::Failed
        } else {
            AttemptResult::Ok
        });
        a.missing.insert("start".into(), MissingReason::NotRecorded);
        a.missing.insert("end".into(), MissingReason::NotRecorded);
        l.append(&a.with_record_id()).unwrap();
    }
    let recs = l.read().unwrap();
    assert_eq!(recs[0].as_attempt().unwrap().span.duration_secs(), None);
    let counts = redo_counts(&recs);
    assert_eq!(counts.values().copied().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn file_without_trailing_newline_does_not_glue_lines() {
    let (_d, l) = log();
    l.append(&op_start(ConfigId::A, Some("2026-10-19T01:00:00Z")))
        .unwrap();
    let t = fs::read_to_string(l.path()).unwrap();
    fs::write(l.path(), t.trim_end()).unwrap();
    l.append(&op_end(ConfigId::A, Some("2026-10-19T01:05:00Z")))
        .unwrap();
    assert_eq!(l.read().unwrap().len(), 2);
}

#[test]
fn v2_fields_validate_and_old_rows_are_rejected() {
    let mut r = op_start(ConfigId::A, Some("2026-10-19T01:00:00Z"));
    r.actor = Some(Actor {
        kind: ActorKind::CosProject,
        id: "cos-p1".into(),
    });
    r.decided_by = Some(Actor {
        kind: ActorKind::Human,
        id: "eval-1".into(),
    });
    r.executed_by = Some(Actor {
        kind: ActorKind::ExternalOperator,
        id: "op1".into(),
    });
    r.credential = Some("operator:op1".into());
    r.request_ref = Some("notice:release-ready".into());
    r.request_revision = Some("3".into());
    r.policy_ref = Some("adr:2026-10-27-external-operator".into());
    let r = r.with_record_id();
    r.validate().expect("v2 op");
    for c in [
        "admin_token",
        "web",
        "cos_run",
        "none",
        "unknown",
        "mcp:chatgpt",
    ] {
        let mut x = r.clone();
        x.credential = Some(c.into());
        x.validate().unwrap_or_else(|e| panic!("{c}: {e}"));
    }
    // revision は record_id の材料（同じ論理作業・attempt の別 revision は別の行）。
    let mut r4 = r.clone();
    r4.request_revision = Some("4".into());
    assert_ne!(r4.compute_record_id(), r.record_id);

    let (_d, l) = log();
    let mut v = serde_json::to_value(&r).expect("json");
    // 新しい欄の無い行（/1 の形）は形式エラー。
    for k in ["credential", "request_ref", "decided_by", "policy_ref"] {
        let mut x = v.clone();
        x.as_object_mut().expect("obj").remove(k);
        fs::write(l.path(), format!("{x}\n")).expect("write");
        assert!(l.read().is_err(), "{k} が無い行を受けた");
    }
    // 未知の主体の種類は拒否。
    v["actor"]["kind"] = serde_json::json!("robot");
    fs::write(l.path(), format!("{v}\n")).expect("write");
    assert!(l.read().is_err());
    // /1 の schema は拒否（互換分岐を置かない）。
    let mut old = r.clone();
    old.schema = "celeris.eval-human-op/1".into();
    let old = old.with_record_id();
    assert!(old.validate().unwrap_err().contains("schema"));
    // 7 種の語彙。
    let kinds = [
        ActorKind::Human,
        ActorKind::Script,
        ActorKind::CosProject,
        ActorKind::CosGeneral,
        ActorKind::ExternalOperator,
        ActorKind::McpClient,
        ActorKind::Unknown,
    ];
    let names: Vec<&str> = kinds.iter().map(ActorKind::as_str).collect();
    assert_eq!(
        names,
        [
            "human",
            "script",
            "cos_project",
            "cos_general",
            "external_operator",
            "mcp_client",
            "unknown"
        ]
    );
}
