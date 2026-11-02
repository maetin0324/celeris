//! Celeris の TS 制御層を動かす埋込み ScriptRuntime（境界 ADR 2026-11-02 (b)）。
//!
//! `deno_core` の `JsRuntime` は `!Send` なので、専用の OS thread（actor）1 本が生成・所有・破棄する。
//! 外から見えるのは [`ScriptRuntimeHandle`]（`Clone + Send + Sync`）・DTO・[`ScriptError`] だけで、
//! `deno_core` と `v8` の型はこの crate の外に出さない。

mod actor;
pub mod dto;
mod error;
mod handle;
mod loader;
pub mod manifest;
mod ops;
pub mod selftest;
mod watchdog;

pub use ops::{DISABLED_BUILTIN_OPS, domain_capabilities};

pub use error::ScriptError;
pub use handle::{
    DEFAULT_HEAP_LIMIT_BYTES, DEFAULT_INPUT_LIMIT_BYTES, DISPATCH_FUNCTION, MAX_IN_FLIGHT,
    QUEUE_CAPACITY, RuntimeLimits, RuntimeStats, ScriptCall, ScriptReply, ScriptRuntimeHandle,
    ScriptSource,
};
