use super::*;
use jev_classifier::{Answer, ApiKey, Classification, WorkNature};
use std::sync::Mutex;

struct Fake(Mutex<Vec<Result<ClassifyOutcome, JevError>>>);

#[async_trait]
impl RequestClassifier for Fake {
    async fn classify(
        &self,
        _: &ClassifyInput,
        _: &dyn ApiKeyProvider,
    ) -> Result<ClassifyOutcome, JevError> {
        self.0.lock().expect("lock").remove(0)
    }
}

struct NoKey;
impl ApiKeyProvider for NoKey {
    fn api_key(&self) -> Option<ApiKey> {
        None
    }
}

fn stage(
    results: Vec<Result<ClassifyOutcome, JevError>>,
) -> (ClassificationStage, ClassificationBoard) {
    let board = ClassificationBoard::default();
    let s = ClassificationStage::new(
        Arc::new(Fake(Mutex::new(results))),
        Arc::new(NoKey),
        CredentialGate {
            secret_id: "jev_api_key".into(),
            provider: "jev".into(),
            purpose: "Jev で依頼を分類する".into(),
        },
        board.clone(),
    );
    (s, board)
}

fn input() -> ClassifyInput {
    ClassifyInput {
        instruction: "fix".into(),
        acceptance: vec![],
        required_artifacts: vec![],
        dependency_summaries: vec![],
        recent_failure: None,
    }
}

fn classified(s: Specificity, conf: f64) -> ClassifyOutcome {
    ClassifyOutcome::Classified {
        classification: Box::new(Classification {
            specificity: Answer {
                value: s,
                probability: Some(conf),
                classification_confidence: Some(conf),
            },
            work_nature: Answer {
                value: WorkNature::Implementation,
                probability: None,
                classification_confidence: None,
            },
            failure_kind: None,
            response_model: Some("typesafe/jev-1.14-2026".into()),
            model_snapshot: None,
            cost_usd: None,
            model_id: "typesafe/jev-1.14".into(),
            schema_version: "1".into(),
            prompt_version: "1".into(),
        }),
        cached: false,
    }
}

#[tokio::test]
async fn success_is_jev_ok_with_a_hint_and_never_marks_the_real_api_verified() {
    let (s, board) = stage(vec![Ok(classified(
        Specificity::NeedsInvestigationDesign,
        0.9,
    ))]);
    let item = s.classify_subject("wu-1", &input()).await;
    assert_eq!(item.entry.status, ClassifierStatus::JevOk);
    assert_eq!(
        item.entry.hint.expect("hint").specificity,
        SpecificityHint::NeedsInvestigationDesign
    );
    assert!(board.get("wu-1").is_some());
}

use std::sync::atomic::{AtomicUsize, Ordering};
use task_core::model::Tier;
use task_core::route_select::{AssistInput, LaneFacts, LaneLimits, assist_lane};
use task_core::route_select::{Fact, WorkClass};

struct Counting(AtomicUsize);

#[async_trait]
impl RequestClassifier for Counting {
    async fn classify(
        &self,
        _: &ClassifyInput,
        _: &dyn ApiKeyProvider,
    ) -> Result<ClassifyOutcome, JevError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(classified(Specificity::Specified, 0.99))
    }
}

fn routine_facts() -> LaneFacts {
    LaneFacts {
        class: WorkClass::Routine,
        procedure_clear: Fact::Yes,
        machine_checkable: Fact::Yes,
        reversible: Fact::Yes,
        failure_impact_small: Fact::Yes,
    }
}

fn assist(base: Tier, entry: &ClassificationEntry) -> task_core::route_select::AssistOutcome {
    assist_lane(
        &AssistInput {
            mode: RoutingMode::DecisionAssisted,
            explicit: false,
            base,
            facts: routine_facts(),
            entry: Some(entry),
            limits: &LaneLimits::default(),
        },
        |_| true,
    )
}

#[tokio::test]
async fn fixed_mode_never_calls_the_classifier_and_leaves_the_board_empty() {
    let board = ClassificationBoard::default();
    let counting = Arc::new(Counting(AtomicUsize::new(0)));
    let s = ClassificationStage::new(
        counting.clone(),
        Arc::new(NoKey),
        CredentialGate {
            secret_id: "jev_api_key".into(),
            provider: "jev".into(),
            purpose: "x".into(),
        },
        board.clone(),
    );
    let item = s
        .classify_for_mode(RoutingMode::Fixed, "wu-1", &input())
        .await;
    assert_eq!(item.entry.status, ClassifierStatus::NotAttempted);
    assert_eq!(counting.0.load(Ordering::SeqCst), 0);
    assert!(board.get("wu-1").is_none());
    s.classify_for_mode(RoutingMode::DecisionAssisted, "wu-1", &input())
        .await;
    assert_eq!(counting.0.load(Ordering::SeqCst), 1);
    assert!(board.get("wu-1").is_some());
}

#[tokio::test]
async fn fallback_records_fell_back_while_real_api_verified_stays_false() {
    for res in [
        Ok(ClassifyOutcome::FallbackToFixed {
            reason: FallbackReason::Timeout,
        }),
        Ok(ClassifyOutcome::FallbackToFixed {
            reason: FallbackReason::BreakerOpen,
        }),
        Err(JevError::KeyMissing {
            secret_id: "jev_api_key".into(),
        }),
        Err(JevError::Auth {
            status: 401,
            secret_id: "jev_api_key".into(),
        }),
    ] {
        let (s, _) = stage(vec![res]);
        let item = s
            .classify_for_mode(RoutingMode::DecisionAssisted, "wu", &input())
            .await;
        let out = assist(Tier::Standard, &item.entry);
        let rec = out.record.expect("record");
        // fixed へ戻る: lane は既存方策のまま。
        assert_eq!(out.lane, Tier::Standard);
        assert!(matches!(
            rec.classifier_status,
            ClassifierStatus::FellBackToFixed { .. }
        ));
        assert!(!rec.real_api_verified);
    }
    // 成功でも検証済みにはならない。
    let (s, _) = stage(vec![Ok(classified(Specificity::Specified, 0.99))]);
    let item = s.classify_subject("wu", &input()).await;
    assert!(
        !assist(Tier::Standard, &item.entry)
            .record
            .expect("record")
            .real_api_verified
    );
}

#[tokio::test]
async fn classification_never_lowers_to_cheap_and_carries_no_success_rate() {
    // 手順確定・高確信でも、既存 lane が standard なら cheap にはならない。
    let (s, _) = stage(vec![Ok(classified(Specificity::Specified, 0.99))]);
    let item = s.classify_subject("wu", &input()).await;
    assert_eq!(assist(Tier::Standard, &item.entry).lane, Tier::Standard);
    // 難しい分類は引き上げのみ。
    let (s, _) = stage(vec![Ok(classified(
        Specificity::NeedsInvestigationDesign,
        0.99,
    ))]);
    let item = s.classify_subject("wu", &input()).await;
    assert_ne!(assist(Tier::Standard, &item.entry).lane, Tier::Cheap);
    // board の項目は分類の確信（confidence）だけで、成功率の欄を持たない。
    let hint = item.entry.hint.expect("hint");
    assert_eq!(hint.confidence, Some(0.99));
}

#[tokio::test]
async fn missing_key_hands_the_gate_to_the_existing_credential_wait() {
    let (s, board) = stage(vec![Err(JevError::KeyMissing {
        secret_id: "jev_api_key".into(),
    })]);
    s.classify_for_mode(RoutingMode::DecisionAssisted, "wu-9", &input())
        .await;
    // dispatcher の apply_lane_assist（既存の credential_gate 経路）が読む形で board に載る。
    let item = board.get("wu-9").expect("board item");
    assert_eq!(
        item.entry.status,
        ClassifierStatus::FellBackToFixed {
            reason: "key_missing".into()
        }
    );
    let gate = item.credential.expect("credential gate");
    assert_eq!(gate.secret_id, "jev_api_key");
}

#[tokio::test]
async fn failures_become_fell_back_to_fixed_with_a_reason_name() {
    for (res, reason, wants_credential) in [
        (
            Ok(ClassifyOutcome::FallbackToFixed {
                reason: FallbackReason::BreakerOpen,
            }),
            "breaker_open",
            false,
        ),
        (
            Ok(ClassifyOutcome::FallbackToFixed {
                reason: FallbackReason::Timeout,
            }),
            "timeout",
            false,
        ),
        (
            Ok(ClassifyOutcome::FallbackToFixed {
                reason: FallbackReason::ClientError(jev_classifier::ClientErrorDetail {
                    status: 400,
                    code: Some("invalid_request".into()),
                    message: Some("criteria must be an object".into()),
                }),
            }),
            "client_error:400 code=invalid_request message=criteria must be an object",
            false,
        ),
        (
            Ok(ClassifyOutcome::FallbackToFixed {
                reason: FallbackReason::ClientError(jev_classifier::ClientErrorDetail {
                    status: 402,
                    code: Some("402".into()),
                    message: Some("insufficient credits".into()),
                }),
            }),
            "client_error:402 code=402 message=insufficient credits",
            false,
        ),
        (
            Err(JevError::KeyMissing {
                secret_id: "jev_api_key".into(),
            }),
            "key_missing",
            true,
        ),
        (
            Err(JevError::Auth {
                status: 401,
                secret_id: "jev_api_key".into(),
            }),
            "auth",
            true,
        ),
    ] {
        let (s, _) = stage(vec![res]);
        let item = s.classify_subject("t", &input()).await;
        assert_eq!(
            item.entry.status,
            ClassifierStatus::FellBackToFixed {
                reason: reason.into()
            }
        );
        assert!(item.entry.hint.is_none());
        assert_eq!(item.credential.is_some(), wants_credential);
    }
}

#[tokio::test]
async fn board_call_carries_response_versions_and_fallback_has_none() {
    // 設定 model（model_id）と応答 model が異なり、独立 snapshot がある成功。
    let mut ok = classified(Specificity::Specified, 0.9);
    if let ClassifyOutcome::Classified { classification, .. } = &mut ok {
        classification.model_snapshot = Some("resp-snap-1".into());
    }
    let (s, board) = stage(vec![
        Ok(ok),
        Ok(classified(Specificity::Specified, 0.9)),
        Ok(ClassifyOutcome::FallbackToFixed {
            reason: FallbackReason::Timeout,
        }),
        Err(JevError::KeyMissing {
            secret_id: "jev_api_key".into(),
        }),
        Ok(ClassifyOutcome::FallbackToFixed {
            reason: FallbackReason::Timeout,
        }),
    ]);
    s.classify_subject("wu-a", &input()).await;
    let c = board.get_call("wu-a").expect("call");
    assert_eq!(
        c.provenance.response_model.as_deref(),
        Some("typesafe/jev-1.14-2026")
    );
    assert_eq!(
        c.provenance.observed_snapshot.as_deref(),
        Some("resp-snap-1")
    );
    // 独立 snapshot が無い応答は None のまま（response_model で補完しない）。
    s.classify_subject("wu-b", &input()).await;
    let c = board.get_call("wu-b").expect("call");
    assert!(c.provenance.response_model.is_some());
    assert_eq!(c.provenance.observed_snapshot, None);
    // fallback / credential 待ちは成功情報を持たない。
    for subj in ["wu-c", "wu-d"] {
        s.classify_subject(subj, &input()).await;
        let c = board.get_call(subj).expect("call");
        assert_eq!(c.provenance, CallProvenance::default());
    }
    // 再分類は新しい call_id・call_no になり、前の版を引き継がない。
    let first = board.get_call("wu-a").expect("call");
    s.classify_subject("wu-a", &input()).await;
    let second = board.get_call("wu-a").expect("call");
    assert_ne!(first.call_id, second.call_id);
    assert_eq!(second.call_no, first.call_no + 1);
    assert_eq!(second.provenance, CallProvenance::default());
}

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
    type Writer = Buf;
    fn make_writer(&'a self) -> Buf {
        self.clone()
    }
}

/// 分類ログは subject・判定（specificity）・確信（欠測は "missing"）を出し、入力本文は出さない。
#[tokio::test]
async fn classification_log_shows_specificity_and_confidence_but_no_input_text() {
    let buf = Buf::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish(),
    );
    // dispatcher が 1 つだけだと、tracing-core は callsite の interest を「登録した thread の既定」で決める。
    // subscriber の無い別の試験 thread が先に callsite を登録すると「無効」で固まるので、2 つ目の dispatcher を
    // 試験の間だけ生かし、全 dispatcher に問い合わせる経路にする。そのうえで既に覚えた interest を作り直す。
    let _second = tracing::Dispatch::new(tracing_subscriber::registry());
    tracing::callsite::rebuild_interest_cache();
    let mut missing = classified(Specificity::NeedsImplementation, 0.9);
    if let ClassifyOutcome::Classified { classification, .. } = &mut missing {
        classification.specificity.classification_confidence = None;
    }
    let (s, _board) = stage(vec![
        Ok(classified(Specificity::Specified, 0.42)),
        Ok(missing),
    ]);
    let secret_input = ClassifyInput {
        instruction: "SECRET-INSTRUCTION-TEXT".into(),
        ..input()
    };
    s.classify_subject("wu-1", &secret_input).await;
    s.classify_subject("wu-2", &secret_input).await;
    let log = String::from_utf8(buf.0.lock().expect("lock").clone()).expect("utf8");
    let line = |subject: &str| {
        log.lines()
            .find(|l| l.contains(subject))
            .unwrap_or_else(|| panic!("no log line for {subject}: {log}"))
            .to_owned()
    };
    assert!(line("wu-1").contains("Specified"), "{log}");
    assert!(line("wu-1").contains("0.420"), "{log}");
    assert!(line("wu-2").contains("NeedsImplementation"), "{log}");
    assert!(line("wu-2").contains("missing"), "{log}");
    assert!(!log.contains("SECRET-INSTRUCTION-TEXT"), "{log}");
}
