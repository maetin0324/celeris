//! `task_dispatch::PolicyScript` の本物の実装（境界 ADR 2026-11-02 付記 S2-6）。`ScriptRuntimeHandle` で TS を呼ぶ。
//!
//! 入出力は DTO だけで、Rust の判定を変えない。結果の分類と記録は dispatcher が行う。
//! `lane_rules` は op を呼ばない純粋関数なので、要求の scope は空で送る（S2-7）。

use std::collections::BTreeSet;
use std::time::Duration;

use celeris_script::dto::HOST_API_VERSION;
use celeris_script::{ScriptCall, ScriptError, ScriptRuntimeHandle};
use task_core::script_compare::{LaneRulesInput, ScriptDomain};
use task_dispatch::script_policy::{
    PolicyFuture, PolicyReply, PolicyScript, PolicyScriptBundle, PolicyScriptError,
};

/// 読み込んだ bundle の actor へ判定を送る口。clone した handle を持つだけで、actor の停止は持ち主が決める。
pub struct ScriptPolicy {
    handle: ScriptRuntimeHandle,
    bundle_sha256: String,
}

impl ScriptPolicy {
    /// `bundle_sha256` は `handle` が評価した bundle の hash（64 桁の小文字 hex）。
    pub fn new(handle: ScriptRuntimeHandle, bundle_sha256: String) -> Self {
        Self {
            handle,
            bundle_sha256,
        }
    }
}

impl PolicyScript for ScriptPolicy {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: self.bundle_sha256.clone(),
            host_api: HOST_API_VERSION.to_owned(),
        }
    }

    fn lane_rules(&self, input: LaneRulesInput, deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async move {
            let input = serde_json::to_value(&input).map_err(|e| {
                PolicyScriptError::new("invalid_output", format!("lane_rules input: {e}"))
            })?;
            let call = ScriptCall {
                domain: ScriptDomain::LaneRules.as_str().to_owned(),
                input,
                scope_task_ids: BTreeSet::new(),
                timeout: deadline,
            };
            match self.handle.call(call).await {
                Ok(reply) => Ok(PolicyReply {
                    output: reply.output,
                    isolate_epoch: isolate_epoch(reply.generation),
                }),
                Err(error) => {
                    let mut mapped = policy_error(&error);
                    mapped.isolate_epoch = Some(isolate_epoch(self.handle.stats().generation));
                    Err(mapped)
                }
            }
        })
    }
}

/// S2-4 の `isolate_epoch` は actor の generation 番号 +1。
fn isolate_epoch(generation: u64) -> u64 {
    generation.saturating_add(1)
}

/// `ScriptError` を DTO にする。種類の名前は `ScriptError` の直列化（`kind`）と同じ snake_case。
fn policy_error(error: &ScriptError) -> PolicyScriptError {
    let kind = match error {
        ScriptError::Exception { .. } => "exception",
        ScriptError::Timeout => "timeout",
        ScriptError::Busy => "busy",
        ScriptError::InvalidOutput { .. } => "invalid_output",
        ScriptError::Denied { .. } => "denied",
        ScriptError::Terminated => "terminated",
        ScriptError::GenerationGone => "generation_gone",
        ScriptError::InputTooLarge { .. } => "input_too_large",
        ScriptError::HeapLimit => "heap_limit",
    };
    let mut mapped = PolicyScriptError::new(kind, error.to_string());
    if let ScriptError::Denied { reason } = error {
        mapped.safety_violation = Some(denied_code(reason).to_owned());
    }
    mapped
}

/// S2-8 の code。`ScriptError::Denied` は理由の文しか持たないので、host の op が書く文で分ける。
/// scope の拒否以外（capability が無い・token が無効）は `denied_capability` に数える。
fn denied_code(reason: &str) -> &'static str {
    if reason.contains("outside the request scope") {
        "denied_scope"
    } else {
        "denied_capability"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_policy_error_kind_matches_script_error_serialization() {
        let errors = [
            ScriptError::Exception {
                message: "boom".into(),
            },
            ScriptError::Timeout,
            ScriptError::Busy,
            ScriptError::InvalidOutput {
                reason: "undefined".into(),
            },
            ScriptError::Denied {
                reason: "nope".into(),
            },
            ScriptError::Terminated,
            ScriptError::GenerationGone,
            ScriptError::InputTooLarge { size: 2, limit: 1 },
            ScriptError::HeapLimit,
        ];
        for error in errors {
            let kind = serde_json::to_value(&error).unwrap()["kind"].clone();
            assert_eq!(kind, policy_error(&error).kind, "{error:?}");
        }
    }

    #[test]
    fn script_policy_denied_maps_to_safety_violation_code() {
        let denied = |reason: &str| {
            policy_error(&ScriptError::Denied {
                reason: reason.into(),
            })
            .safety_violation
        };
        assert_eq!(
            denied("task 01M is outside the request scope").as_deref(),
            Some("denied_scope")
        );
        assert_eq!(
            denied("capability SelftestEchoTask is not granted for this request").as_deref(),
            Some("denied_capability")
        );
        assert_eq!(policy_error(&ScriptError::Timeout).safety_violation, None);
    }
}
