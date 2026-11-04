//! Host API `celeris:host` v1 の native op（境界 ADR 2026-11-02 (c)）。
//!
//! - 有効な capability は「bundle の manifest の宣言 ∩ 領域ごとの許可表（[`domain_capabilities`]）」だけ。
//! - 要求ごとに [`Grant`]（有効な capability と scope〈task id〉）を actor が登録し、op は JS から渡された
//!   要求の token（要求 ID）でそれを引いて照合する。範囲外・未登録（終わった要求）の token は `denied`。
//! - capability 付きの op（`cos_digest:*`）は store を持たない。引数を DTO で検証し、要求ごとに Rust 側が注入した
//!   host sink（[`crate::HostSink`]）へ送って応答を await する（付記 S4-12）。待っている間に要求が終わった
//!   （返事・deadline・cancel・generation の作り直し）ら結果を渡さない（付記 S4-18）。
//! - capability を持つ要求は 1 つの isolate で同時に 1 件まで（actor が排他する。付記 S4-13 の 1）。
//! - filesystem・network・環境変数・process・native addon の op は作らない。deno_core の組み込みの op のうち
//!   resource の読み書き・stdout への出力・任意の評価・同期 import は [`disable_builtin`] で無効にする。

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::time::Instant;

use deno_core::{OpDecl, OpState, op2};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{oneshot, watch};
use ulid::Ulid;

use crate::dto::{
    Capability, CosDigestInput, CosDigestListRunsReply, CosDigestListRunsRequest,
    CosDigestReadRunRequest, HOST_API_VERSION, HostError, HostErrorCode, HostResult,
    SelftestEchoReply, SelftestEchoRequest, SelftestObservation,
};
use crate::handle::{HostOpCall, HostSink, MAX_OPS_PER_REQUEST};

/// 領域ごとの許可表。ここに無い領域は capability を 1 つも持たない。
/// 各 capability は [`Capability::domain`] の領域にだけ置く（`ops::tests` が確かめる）。
const DOMAIN_CAPABILITIES: &[(&str, &[Capability])] = &[
    ("host_selftest", &[Capability::SelftestEchoTask]),
    (
        "cos_digest",
        &[Capability::CosDigestListRuns, Capability::CosDigestReadRun],
    ),
];

/// 領域が許す capability。
pub fn domain_capabilities(domain: &str) -> &'static [Capability] {
    DOMAIN_CAPABILITIES
        .iter()
        .find(|(d, _)| *d == domain)
        .map(|(_, caps)| *caps)
        .unwrap_or(&[])
}

/// 要求 1 つに有効な権限。
#[derive(Debug, Clone)]
pub(crate) struct Grant {
    /// 要求の領域。op は自分の領域の grant でだけ通る（付記 S4-13 の 2）。
    pub(crate) domain: String,
    pub(crate) capabilities: BTreeSet<Capability>,
    pub(crate) scope_task_ids: BTreeSet<String>,
    pub(crate) request_id: Ulid,
    /// 要求の deadline。op の待ちもここで `timeout` になる。
    pub(crate) deadline: Instant,
    pub(crate) sink: Option<HostSink>,
    /// この要求が呼んだ capability 付き op の回数（clone 間で共有）。
    pub(crate) calls: Rc<Cell<u32>>,
    /// 要求が終わる（[`GrantGuard`] が落ちる）と `changed()` が Err を返す。
    pub(crate) ended: watch::Receiver<()>,
}

/// 要求が終わったことを op へ知らせる送り口。[`GrantGuard`] が持ち、落ちると知らせる。
pub(crate) struct EndSignal(#[allow(dead_code)] watch::Sender<()>);

/// [`Grant::new`] の引数（要求から actor が写す）。
pub(crate) struct GrantSpec<'a> {
    pub(crate) declared: &'a BTreeSet<Capability>,
    pub(crate) domain: &'a str,
    pub(crate) scope_task_ids: &'a BTreeSet<String>,
    pub(crate) request_id: Ulid,
    pub(crate) deadline: Instant,
    pub(crate) sink: Option<HostSink>,
}

impl Grant {
    /// manifest の宣言と領域の許可表の積をとる。
    pub(crate) fn new(spec: GrantSpec<'_>) -> (Self, EndSignal) {
        let capabilities = domain_capabilities(spec.domain)
            .iter()
            .copied()
            .filter(|c| spec.declared.contains(c))
            .collect();
        let (end_tx, end_rx) = watch::channel(());
        (
            Self {
                domain: spec.domain.to_owned(),
                capabilities,
                scope_task_ids: spec.scope_task_ids.clone(),
                request_id: spec.request_id,
                deadline: spec.deadline,
                sink: spec.sink,
                calls: Rc::new(Cell::new(0)),
                ended: end_rx,
            },
            EndSignal(end_tx),
        )
    }

    /// capability を 1 つ以上持つ要求は isolate の中で排他にする（付記 S4-13 の 1）。
    pub(crate) fn is_exclusive(&self) -> bool {
        !self.capabilities.is_empty()
    }
}

/// generation の OpState に置く、要求 token → 権限の表。
pub(crate) type Grants = Rc<RefCell<HashMap<String, Grant>>>;

/// 要求が終わったら（返事・破棄・generation の作り直しで落ちたら）表から消す。
/// 消すと同時に、待っている op へ要求の終わりを知らせる（`EndSignal` を落とす）。
pub(crate) struct GrantGuard {
    grants: Grants,
    token: String,
    _end: EndSignal,
}

impl GrantGuard {
    pub(crate) fn insert(grants: &Grants, token: String, grant: Grant, end: EndSignal) -> Self {
        grants.borrow_mut().insert(token.clone(), grant);
        Self {
            grants: Rc::clone(grants),
            token,
            _end: end,
        }
    }
}

impl Drop for GrantGuard {
    fn drop(&mut self) {
        self.grants.borrow_mut().remove(&self.token);
        // `_end` はこの後に落ち、待っている op の `ended.changed()` が Err を返す。
    }
}

struct GrantTable(Grants);

/// token の権限を引き、capability を確かめる。
fn check(state: &OpState, token: &str, needed: Capability) -> Result<Grant, HostError> {
    let table = state.borrow::<GrantTable>();
    let grants = table.0.borrow();
    let grant = grants.get(token).ok_or_else(|| {
        HostError::new(
            HostErrorCode::Denied,
            "unknown or finished request token".to_string(),
        )
    })?;
    if !grant.capabilities.contains(&needed) || grant.domain != needed.domain() {
        return Err(HostError::new(
            HostErrorCode::Denied,
            format!("capability {needed:?} is not granted for this request"),
        ));
    }
    Ok(grant.clone())
}

/// token の grant がまだあるか（要求が終わっていないか）。
fn alive(state: &OpState, token: &str) -> bool {
    state.borrow::<GrantTable>().0.borrow().contains_key(token)
}

/// capability 付きの op の本体（付記 S4-12・S4-18）: 権限と回数を確かめ、引数を DTO で検証して host sink へ送り、
/// 応答を DTO で検証して返す。待っている間に要求が終わったら `terminated`、deadline を過ぎたら `timeout`。
async fn forward<Req, Rep>(
    state: Rc<RefCell<OpState>>,
    token: String,
    op: Capability,
    request: serde_json::Value,
) -> Result<Rep, HostError>
where
    Req: DeserializeOwned + Serialize,
    Rep: DeserializeOwned,
{
    let grant = check(&state.borrow(), &token, op)?;
    let n = grant.calls.get().saturating_add(1);
    grant.calls.set(n);
    if n > MAX_OPS_PER_REQUEST {
        return Err(HostError::new(
            HostErrorCode::Busy,
            format!("more than {MAX_OPS_PER_REQUEST} op calls in one request"),
        ));
    }
    let name = op.as_str();
    let req: Req = serde_json::from_value(request)
        .map_err(|e| HostError::new(HostErrorCode::InvalidOutput, format!("{name}: {e}")))?;
    let request = serde_json::to_value(&req)
        .map_err(|e| HostError::new(HostErrorCode::InvalidOutput, format!("{name}: {e}")))?;
    let sink = grant.sink.clone().ok_or_else(|| {
        HostError::new(
            HostErrorCode::Exception,
            format!("{name}: no host sink for this request"),
        )
    })?;
    let deadline = tokio::time::Instant::from_std(grant.deadline);
    if tokio::time::Instant::now() >= deadline {
        return Err(HostError::new(
            HostErrorCode::Timeout,
            format!("{name}: deadline passed"),
        ));
    }
    let (reply_tx, reply_rx) = oneshot::channel();
    let call = HostOpCall {
        request_id: grant.request_id,
        op,
        request,
        reply: reply_tx,
    };
    match sink.try_send(call) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            return Err(HostError::new(
                HostErrorCode::Busy,
                format!("{name}: host sink is full"),
            ));
        }
        // op server が受け付けを止めた（pass の終わり・deadline）。
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            return Err(HostError::new(
                HostErrorCode::Terminated,
                format!("{name}: host sink is closed"),
            ));
        }
    }
    let terminated = || {
        HostError::new(
            HostErrorCode::Terminated,
            format!("{name}: the request ended while the op was waiting"),
        )
    };
    let mut ended = grant.ended.clone();
    let timeout = || {
        HostError::new(
            HostErrorCode::Timeout,
            format!("{name}: deadline passed while waiting"),
        )
    };
    let outcome = tokio::select! {
        biased;
        _ = tokio::time::sleep_until(deadline) => return Err(timeout()),
        // actor は deadline で要求を終わらせる。その場合は終わりの知らせより deadline を先に報告する。
        _ = ended.changed() => {
            if tokio::time::Instant::now() >= deadline {
                return Err(timeout());
            }
            return Err(terminated());
        }
        r = reply_rx => r,
    };
    // 待っている間に要求が終わっていたら結果を渡さない。
    if !alive(&state.borrow(), &token) {
        return Err(terminated());
    }
    match outcome {
        Ok(HostResult::Ok { value }) => serde_json::from_value(value).map_err(|e| {
            HostError::new(
                HostErrorCode::Exception,
                format!("{name}: invalid reply from the host: {e}"),
            )
        }),
        Ok(HostResult::Error { error }) => Err(error),
        // 受け手が答えずに応答口を落とした（op server の停止）。
        Err(_) => Err(terminated()),
    }
}

/// `cos_digest:list_runs`（付記 S4-12）。
#[op2]
#[serde]
async fn op_cos_digest_list_runs(
    state: Rc<RefCell<OpState>>,
    #[string] token: String,
    #[serde] request: serde_json::Value,
) -> HostResult<CosDigestListRunsReply> {
    forward::<CosDigestListRunsRequest, CosDigestListRunsReply>(
        state,
        token,
        Capability::CosDigestListRuns,
        request,
    )
    .await
    .into()
}

/// `cos_digest:read_run`（付記 S4-12）。scope（その pass の一覧にある組か）は op server が照合する。
#[op2]
#[serde]
async fn op_cos_digest_read_run(
    state: Rc<RefCell<OpState>>,
    #[string] token: String,
    #[serde] request: serde_json::Value,
) -> HostResult<CosDigestInput> {
    forward::<CosDigestReadRunRequest, CosDigestInput>(
        state,
        token,
        Capability::CosDigestReadRun,
        request,
    )
    .await
    .into()
}

/// Host API の版を返す（capability 不要）。
#[op2]
#[string]
fn op_host_version() -> String {
    HOST_API_VERSION.to_string()
}

/// 試験用の async op: 要求の scope の task について、引数を検証して写し返す。
#[op2]
#[serde]
async fn op_host_selftest_echo(
    state: Rc<RefCell<OpState>>,
    #[string] token: String,
    #[serde] request: serde_json::Value,
) -> HostResult<SelftestEchoReply> {
    // 値は serde_v8 で serde_json::Value にしてから from_value で検証する（文字列の parse を通さない）。
    let checked = (|| {
        let grant = check(&state.borrow(), &token, Capability::SelftestEchoTask)?;
        let req: SelftestEchoRequest = serde_json::from_value(request).map_err(|e| {
            HostError::new(
                HostErrorCode::InvalidOutput,
                format!("op_host_selftest_echo: {e}"),
            )
        })?;
        if !grant.scope_task_ids.contains(&req.task_id) {
            return Err(HostError::new(
                HostErrorCode::Denied,
                format!("task {} is outside the request scope", req.task_id),
            ));
        }
        Ok(req)
    })();
    let req = match checked {
        Ok(req) => req,
        Err(e) => return HostResult::Error { error: e },
    };
    // 非同期の経路を通す（実際の store 読み出しの代わり）。
    tokio::task::yield_now().await;
    // 待っている間に要求が終わっていたら結果を渡さない。
    if let Err(e) = check(&state.borrow(), &token, Capability::SelftestEchoTask) {
        return HostResult::Error { error: e };
    }
    let observation = match &req.note {
        Some(n) => SelftestObservation::Noted {
            length: u32::try_from(n.chars().count()).unwrap_or(u32::MAX),
        },
        None => SelftestObservation::Empty,
    };
    HostResult::Ok {
        value: SelftestEchoReply {
            task_id: req.task_id,
            note: req.note,
            tokens: req.tokens,
            priority: req.priority,
            at: req.at,
            observation,
        },
    }
}

/// 無効にする deno_core の組み込みの op。resource（stdio 等）の読み書き・stdout への出力・
/// 任意の文脈での評価・同期 import・wasm streaming。
pub const DISABLED_BUILTIN_OPS: &[&str] = &[
    "op_print",
    "op_read",
    "op_read_all",
    "op_read_sync",
    "op_write",
    "op_write_sync",
    "op_write_all",
    "op_write_type_error",
    "op_pipe",
    "op_shutdown",
    "op_close",
    "op_try_close",
    "op_cancel_read",
    "op_resources",
    "op_is_terminal",
    "op_import_sync",
    "op_import_sync_with_source",
    "op_eval_context",
    "op_compile_function",
    "op_wasm_streaming_feed",
    "op_wasm_streaming_set_url",
    "op_panic",
];

fn disable_builtin(op: OpDecl) -> OpDecl {
    if DISABLED_BUILTIN_OPS.contains(&op.name) {
        op.disable()
    } else {
        op
    }
}

deno_core::extension!(
    celeris_host,
    ops = [
        op_host_version,
        op_host_selftest_echo,
        op_cos_digest_list_runs,
        op_cos_digest_read_run
    ],
    options = { grants: Grants },
    middleware = disable_builtin,
    state = |state, options| {
        state.put(GrantTable(options.grants));
    },
);

/// generation 1 つ分の extension。`grants` は actor と共有する。
pub(crate) fn extension(grants: Grants) -> deno_core::Extension {
    celeris_host::init(grants)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 許可表の各 capability は自分の領域にだけ置く（付記 S4-13 の 2）。
    #[test]
    fn domain_table_matches_capability_domain() {
        for (domain, caps) in DOMAIN_CAPABILITIES {
            for cap in *caps {
                assert_eq!(cap.domain(), *domain, "{cap:?}");
            }
        }
        assert!(domain_capabilities("lane_rules").is_empty());
        assert!(domain_capabilities("route_priority").is_empty());
        assert!(domain_capabilities("execution_gate").is_empty());
        assert!(domain_capabilities("project_controller").is_empty());
    }
}
