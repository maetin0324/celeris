//! Celeris の TS 制御層を動かす埋込み ScriptRuntime（境界 ADR 2026-11-02 (b)）。
//!
//! `deno_core` の `JsRuntime` は `!Send` なので、専用の OS thread（actor）1 本が生成・所有・破棄する。
//! 外から見えるのは [`ScriptRuntimeHandle`]（`Clone + Send + Sync`）・DTO・[`ScriptError`] だけで、
//! `deno_core` と `v8` の型はこの crate の外に出さない。

mod actor;
mod error;
mod handle;

pub use error::ScriptError;
pub use handle::{
    DISPATCH_FUNCTION, MAX_IN_FLIGHT, QUEUE_CAPACITY, RuntimeStats, ScriptCall, ScriptReply,
    ScriptRuntimeHandle, ScriptSource,
};
