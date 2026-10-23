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
use task_dispatch::{BoardItem, CallProvenance, ClassificationBoard, CredentialGate};

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
        // 応答由来の版は成功した call だけが持つ。fallback・credential 待ち・config は全欄 None（欠測）。
        let mut provenance = CallProvenance::default();
        let item = match result {
            Ok(ClassifyOutcome::Classified { classification, .. }) => {
                provenance = CallProvenance {
                    response_model: classification.response_model.clone(),
                    observed_snapshot: classification.model_snapshot.clone(),
                };
                BoardItem {
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
                }
            }
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
            status => {
                // 判定と確信だけ（鍵・入力本文・応答の生文は出さない）。確信が欠測なら "missing"。
                let hint = item.entry.hint.as_ref();
                let specificity = hint.map(|h| format!("{:?}", h.specificity));
                let confidence = hint
                    .and_then(|h| h.confidence)
                    .map_or_else(|| "missing".to_owned(), |c| format!("{c:.3}"));
                tracing::info!(
                    subject,
                    ?status,
                    specificity = specificity.as_deref().unwrap_or("none"),
                    confidence = %confidence,
                    "jev: classification result"
                )
            }
        }
        self.board
            .put_with_provenance(subject, item.clone(), provenance);
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
#[path = "classify_stage/tests.rs"]
mod tests;
