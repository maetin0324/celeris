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
use task_dispatch::{BoardItem, ClassificationBoard, CredentialGate};
use task_core::route_select::{
    ClassificationEntry, ClassificationHint, ClassifierStatus, SpecificityHint,
};

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

fn reason_name(r: FallbackReason) -> &'static str {
    match r {
        FallbackReason::BreakerOpen => "breaker_open",
        FallbackReason::Timeout => "timeout",
        FallbackReason::Transport => "transport",
        FallbackReason::RateLimited => "rate_limited",
        FallbackReason::ServerError(_) => "server_error",
        FallbackReason::ClientError(_) => "client_error",
        FallbackReason::InvalidResponse => "invalid_response",
        FallbackReason::UnknownClassification => "unknown_classification",
    }
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
                    model_snapshot: classification.model_snapshot.clone(),
                },
                credential: None,
            },
            Ok(ClassifyOutcome::FallbackToFixed { reason }) => BoardItem {
                entry: ClassificationEntry::fell_back(reason_name(reason)),
                credential: None,
            },
            Err(JevError::KeyMissing { .. }) => self.credential_item("key_missing"),
            Err(JevError::Auth { .. }) => self.credential_item("auth"),
            Err(JevError::Config(_)) => BoardItem {
                entry: ClassificationEntry::fell_back("config"),
                credential: None,
            },
        };
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
    use jev_classifier::{ApiKey, Answer, Classification, WorkNature};
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

    fn stage(results: Vec<Result<ClassifyOutcome, JevError>>) -> (ClassificationStage, ClassificationBoard) {
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
                model_snapshot: Some("typesafe/jev-1.14-2026".into()),
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
        let (s, board) = stage(vec![Ok(classified(Specificity::NeedsInvestigationDesign, 0.9))]);
        let item = s.classify_subject("wu-1", &input()).await;
        assert_eq!(item.entry.status, ClassifierStatus::JevOk);
        assert_eq!(
            item.entry.hint.expect("hint").specificity,
            SpecificityHint::NeedsInvestigationDesign
        );
        assert!(board.get("wu-1").is_some());
    }

    #[tokio::test]
    async fn failures_become_fell_back_to_fixed_with_a_reason_name() {
        for (res, reason, wants_credential) in [
            (Ok(ClassifyOutcome::FallbackToFixed { reason: FallbackReason::BreakerOpen }), "breaker_open", false),
            (Ok(ClassifyOutcome::FallbackToFixed { reason: FallbackReason::Timeout }), "timeout", false),
            (Err(JevError::KeyMissing { secret_id: "jev_api_key".into() }), "key_missing", true),
            (Err(JevError::Auth { status: 401, secret_id: "jev_api_key".into() }), "auth", true),
        ] {
            let (s, _) = stage(vec![res]);
            let item = s.classify_subject("t", &input()).await;
            assert_eq!(
                item.entry.status,
                ClassifierStatus::FellBackToFixed { reason: reason.into() }
            );
            assert!(item.entry.hint.is_none());
            assert_eq!(item.credential.is_some(), wants_credential);
        }
    }
}
