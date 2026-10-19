//! 要求の分類段（ADR 2026-10-15-mlsys-min-routing-policy D4、wire-mode）。
//!
//! dispatcher・store の**外**で Jev（`jev-classifier`）を呼び、結果を `ClassificationBoard` に置く。dispatcher の
//! lane 層（`lane_assist`）は board を読むだけで、Jev を呼ばない。分類器は `RequestClassifier` 越しに注入できる
//! （試験は fake。実 API は呼ばない）。失敗は fixed へ戻る記録（`fell_back_to_fixed{reason}`）になり、
//! 鍵未登録・認証失敗は credential の宣言（secret id と用途）を添えて既存の credential 待ちへ渡す。
//! 「fixed に戻った」ことと「Jev 実 API 統合が検証済み」は別で、後者はこの段では true にしない。

use std::sync::Arc;

use async_trait::async_trait;
use jev_classifier::{
    ApiKeyProvider, ClassifyInput, ClassifyOutcome, FallbackReason, JevClassifier, JevError,
    Specificity,
};
use task_core::route_select::{
    ClassificationEntry, ClassificationHint, ClassifierStatus, RoutingMode, SpecificityHint,
};
use task_dispatch::{BoardItem, ClassificationBoard, CredentialGate};

/// 分類器の差し込み口。本番は `JevClassifier`、試験は fake。
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    async fn classify(
        &self,
        input: &ClassifyInput,
        keys: &dyn ApiKeyProvider,
    ) -> Result<ClassifyOutcome, JevError>;
}

#[async_trait]
impl RequestClassifier for JevClassifier {
    async fn classify(
        &self,
        input: &ClassifyInput,
        keys: &dyn ApiKeyProvider,
    ) -> Result<ClassifyOutcome, JevError> {
        JevClassifier::classify(self, input, keys).await
    }
}

fn reason_name(r: &FallbackReason) -> String {
    let name = match r {
        FallbackReason::BreakerOpen => "breaker_open",
        FallbackReason::Timeout => "timeout",
        FallbackReason::Transport => "transport",
        FallbackReason::RateLimited => "rate_limited",
        FallbackReason::ServerError(_) => "server_error",
        FallbackReason::ClientError(d) => {
            // status と応答の error.code・error.message だけ（鍵・入力本文は持たない）。
            let mut s = format!("client_error:{}", d.status);
            if let Some(c) = &d.code {
                s.push_str(&format!(" code={c}"));
            }
            if let Some(m) = &d.message {
                s.push_str(&format!(" message={m}"));
            }
            return s;
        }
        FallbackReason::InvalidResponse => "invalid_response",
        FallbackReason::UnknownClassification => "unknown_classification",
    };
    name.to_string()
}

fn hint_of(s: Specificity) -> SpecificityHint {
    match s {
        Specificity::Specified => SpecificityHint::Specified,
        Specificity::NeedsImplementation => SpecificityHint::NeedsImplementation,
        Specificity::NeedsInvestigationDesign => SpecificityHint::NeedsInvestigationDesign,
        Specificity::InsufficientInfo => SpecificityHint::InsufficientInfo,
    }
}

pub struct ClassificationStage {
    classifier: Arc<dyn RequestClassifier>,
    keys: Arc<dyn ApiKeyProvider>,
    /// 鍵の宣言（secret id・用途）。値ではない。
    credential: CredentialGate,
    board: ClassificationBoard,
}

impl ClassificationStage {
    pub fn new(
        classifier: Arc<dyn RequestClassifier>,
        keys: Arc<dyn ApiKeyProvider>,
        credential: CredentialGate,
        board: ClassificationBoard,
    ) -> Self {
        Self {
            classifier,
            keys,
            credential,
            board,
        }
    }

    /// `mode` が `Fixed` なら分類器を呼ばず board にも置かない（`not_attempted` を返すだけ）。
    pub async fn classify_for_mode(
        &self,
        mode: RoutingMode,
        subject: &str,
        input: &ClassifyInput,
    ) -> BoardItem {
        if mode == RoutingMode::Fixed {
            return BoardItem {
                entry: ClassificationEntry {
                    status: ClassifierStatus::NotAttempted,
                    hint: None,
                    model_snapshot: None,
                    cost_usd: None,
                },
                credential: None,
            };
        }
        self.classify_subject(subject, input).await
    }

    /// `subject`（work unit id、無ければ task id）を分類して board に置き、置いた内容を返す。
    pub async fn classify_subject(&self, subject: &str, input: &ClassifyInput) -> BoardItem {
        let result = self.classifier.classify(input, self.keys.as_ref()).await;
        let item = match result {
            Ok(ClassifyOutcome::Classified { classification, .. }) => BoardItem {
                entry: ClassificationEntry {
                    status: ClassifierStatus::JevOk,
                    hint: Some(ClassificationHint {
                        specificity: hint_of(classification.specificity.value),
                        confidence: classification.specificity.classification_confidence,
                    }),
                    model_snapshot: classification.response_model.clone(),
                    cost_usd: classification.cost_usd,
                },
                credential: None,
            },
            Ok(ClassifyOutcome::FallbackToFixed { reason }) => BoardItem {
                entry: ClassificationEntry::fell_back(reason_name(&reason)),
                credential: None,
            },
            Err(JevError::KeyMissing { .. }) => self.credential_item("key_missing"),
            Err(JevError::Auth { .. }) => self.credential_item("auth"),
            Err(JevError::Config(_)) => BoardItem {
                entry: ClassificationEntry::fell_back("config"),
                credential: None,
            },
        };
        // 鍵の値・入力本文は出さない（subject の id と結果だけ）。
        match &item.entry.status {
            ClassifierStatus::FellBackToFixed { reason } => {
                tracing::info!(subject, reason = %reason, "jev: classification fell_back_to_fixed")
            }
            status => tracing::info!(subject, ?status, "jev: classification result"),
        }
        self.board.put(subject, item.clone());
        item
    }

    fn credential_item(&self, reason: &str) -> BoardItem {
        BoardItem {
            entry: ClassificationEntry::fell_back(reason),
            credential: Some(self.credential.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
