//! Host API `celeris:host` v1 の native op（境界 ADR 2026-11-02 (c)）。
//!
//! - 有効な capability は「bundle の manifest の宣言 ∩ 領域ごとの許可表（[`domain_capabilities`]）」だけ。
//! - 要求ごとに [`Grant`]（有効な capability と scope〈task id〉）を actor が登録し、op は JS から渡された
//!   要求の token（要求 ID）でそれを引いて照合する。範囲外・未登録（終わった要求）の token は `denied`。
//! - filesystem・network・環境変数・process・native addon の op は作らない。deno_core の組み込みの op のうち
//!   resource の読み書き・stdout への出力・任意の評価・同期 import は [`disable_builtin`] で無効にする。

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use deno_core::{OpDecl, OpState, op2};

use crate::dto::{
    Capability, HOST_API_VERSION, HostError, HostErrorCode, HostResult, SelftestEchoReply,
    SelftestEchoRequest, SelftestObservation,
};

/// 領域ごとの許可表。ここに無い領域は capability を 1 つも持たない。
const DOMAIN_CAPABILITIES: &[(&str, &[Capability])] =
    &[("host_selftest", &[Capability::SelftestEchoTask])];

/// 領域が許す capability。
pub fn domain_capabilities(domain: &str) -> &'static [Capability] {
    DOMAIN_CAPABILITIES
        .iter()
        .find(|(d, _)| *d == domain)
        .map(|(_, caps)| *caps)
        .unwrap_or(&[])
}

/// 要求 1 つに有効な権限。
#[derive(Debug, Clone, Default)]
pub(crate) struct Grant {
    pub(crate) capabilities: BTreeSet<Capability>,
    pub(crate) scope_task_ids: BTreeSet<String>,
}

impl Grant {
    /// manifest の宣言と領域の許可表の積をとる。
    pub(crate) fn new(
        declared: &BTreeSet<Capability>,
        domain: &str,
        scope_task_ids: &BTreeSet<String>,
    ) -> Self {
        let capabilities = domain_capabilities(domain)
            .iter()
            .copied()
            .filter(|c| declared.contains(c))
            .collect();
        Self {
            capabilities,
            scope_task_ids: scope_task_ids.clone(),
        }
    }
}

/// generation の OpState に置く、要求 token → 権限の表。
pub(crate) type Grants = Rc<RefCell<HashMap<String, Grant>>>;

/// 要求が終わったら（返事・破棄・generation の作り直しで落ちたら）表から消す。
pub(crate) struct GrantGuard {
    grants: Grants,
    token: String,
}

impl GrantGuard {
    pub(crate) fn insert(grants: &Grants, token: String, grant: Grant) -> Self {
        grants.borrow_mut().insert(token.clone(), grant);
        Self {
            grants: Rc::clone(grants),
            token,
        }
    }
}

impl Drop for GrantGuard {
    fn drop(&mut self) {
        self.grants.borrow_mut().remove(&self.token);
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
    if !grant.capabilities.contains(&needed) {
        return Err(HostError::new(
            HostErrorCode::Denied,
            format!("capability {needed:?} is not granted for this request"),
        ));
    }
    Ok(grant.clone())
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
    ops = [op_host_version, op_host_selftest_echo],
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
