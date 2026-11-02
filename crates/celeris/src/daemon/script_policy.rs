//! `task_dispatch::PolicyScript` の本物の実装（境界 ADR 2026-11-02 付記 S2-6）。`ScriptRuntimeHandle` で TS を呼ぶ。
//!
//! 入出力は DTO だけで、Rust の判定を変えない。結果の分類と記録は dispatcher が行う。
//! `lane_rules`・`route_priority` は op を呼ばない純粋関数なので、要求の scope は空で送る（S2-7）。
//! `cos_digest` は pass 1 回を要求 1 件として host sink 付きで送り、op を dispatcher の op server へ写す（付記 S4-12）。

use std::collections::BTreeSet;
use std::time::Duration;

use celeris_script::dto::{Capability, HOST_API_VERSION, HostError, HostErrorCode, HostResult};
use celeris_script::{HostOpCall, ScriptCall, ScriptError, ScriptRuntimeHandle};
use task_core::script_compare::{LaneRulesInput, RoutePriorityInput, ScriptDomain};
use task_dispatch::script_policy::{
    CosDigestOp, CosDigestOpCall, CosDigestOpReply, CosDigestPassCall, PolicyFuture, PolicyReply,
    PolicyScript, PolicyScriptBundle, PolicyScriptError,
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
        self.call_domain(
            ScriptDomain::LaneRules,
            serde_json::to_value(&input),
            deadline,
        )
    }

    fn route_priority(&self, input: RoutePriorityInput, deadline: Duration) -> PolicyFuture<'_> {
        self.call_domain(
            ScriptDomain::RoutePriority,
            serde_json::to_value(&input),
            deadline,
        )
    }

    /// 付記 S4-12: pass 1 回を `cos_digest` の要求 1 件として送る。TS の op（host sink に届く `HostOpCall`）は
    /// dispatcher の op server（`call.sink`）へ写し、応答を `HostResult` の形で返す。写しは呼び出しと同じ
    /// future の中で走る（別の task を作らない）。呼び出しが終われば写しも止まる。
    fn cos_digest_pass(&self, call: CosDigestPassCall, deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async move {
            let (host_sink, mut host_rx) = celeris_script::host_sink();
            let request = ScriptCall {
                domain: ScriptDomain::CosDigest.as_str().to_owned(),
                input: serde_json::json!({}),
                scope_task_ids: BTreeSet::new(),
                timeout: deadline,
            };
            let reply = self.handle.call_with_host_sink(request, Some(host_sink));
            tokio::pin!(reply);
            let result = loop {
                tokio::select! {
                    result = &mut reply => break result,
                    Some(op) = host_rx.recv() => forward_op(&call, op).await,
                }
            };
            self.reply(result)
        })
    }
}

/// TS の op 1 回を dispatcher の op server へ写す。送れない・答えが無いときは `terminated`。
async fn forward_op(call: &CosDigestPassCall, op: HostOpCall) {
    let kind = match op.op {
        Capability::CosDigestListRuns => CosDigestOp::ListRuns,
        Capability::CosDigestReadRun => CosDigestOp::ReadRun,
        other => {
            let _ = op.reply.send(HostResult::Error {
                error: HostError::new(
                    HostErrorCode::Denied,
                    format!("{} is not a cos_digest op", other.as_str()),
                ),
            });
            return;
        }
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sent = call
        .sink
        .send(CosDigestOpCall {
            op: kind,
            request: op.request,
            reply: tx,
        })
        .await;
    let answer = match (sent, rx.await) {
        (Ok(()), Ok(CosDigestOpReply::Ok(value))) => HostResult::Ok { value },
        (Ok(()), Ok(CosDigestOpReply::Error { code, message })) => HostResult::Error {
            error: HostError::new(host_code(&code), message),
        },
        _ => HostResult::Error {
            error: HostError::new(HostErrorCode::Terminated, "cos_digest op server is gone"),
        },
    };
    let _ = op.reply.send(answer);
}

/// op server の `code` を `HostErrorCode` にする（知らない code は `exception`）。
fn host_code(code: &str) -> HostErrorCode {
    serde_json::from_value(serde_json::Value::String(code.to_owned()))
        .unwrap_or(HostErrorCode::Exception)
}

impl ScriptPolicy {
    /// 領域の入力 DTO を scope 空（S2-7）で actor に送る。
    fn call_domain(
        &self,
        domain: ScriptDomain,
        input: Result<serde_json::Value, serde_json::Error>,
        deadline: Duration,
    ) -> PolicyFuture<'_> {
        Box::pin(async move {
            let input = input.map_err(|e| {
                PolicyScriptError::new("invalid_output", format!("{} input: {e}", domain.as_str()))
            })?;
            let call = ScriptCall {
                domain: domain.as_str().to_owned(),
                input,
                scope_task_ids: BTreeSet::new(),
                timeout: deadline,
            };
            let result = self.handle.call(call).await;
            self.reply(result)
        })
    }

    /// actor の結果を DTO にする。
    fn reply(
        &self,
        result: Result<celeris_script::ScriptReply, ScriptError>,
    ) -> Result<PolicyReply, PolicyScriptError> {
        match result {
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
