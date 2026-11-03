//! デーモンの配線。`lib.rs` は入口（`mod` 宣言・`DaemonError` / `RunOptions` / `Exit`・再公開）だけを持ち、
//! 起動から停止までの配線はこの下に責務ごとに置く。
//!
//! | module | 中身 |
//! |---|---|
//! | `secrets` | `[secrets]` の解決と env の合成（ADR-0030）、実効モデル |
//! | `adapters` | `build_harness_adapters`・`secret_usage` |
//! | `bootstrap` | DB の置き場所の検査、`build_dispatcher`（store を開き migrate する）、`seed_org_if_empty`・`seed_cron_if_empty` |
//! | `clusters` | ssh master・liveness hook・tunnel の配線（ADR-0032 / ADR-0062） |
//! | `api` | `config_view`・`api_settings`・`bind_reuseport`・`start_api` |
//! | `services` | llm-proxy と MCP の組み立て・起動・停止 |
//! | `run` | `run`: 起動順と停止順（interrupt → api → llm_proxy → mcp → script → checkpoint → backup → deregister） |
//! | `script_guard` | 付記 S5-7: script の自動の戻し・無効化・停止の記録を cursor で読み、受信箱 `script_control` の項目を作る（LLM なし） |
//! | `script_layer` | 付記 S5-1・S5-2: generation の登録（新しい isolate で検証）・切替後の drain・起動時の復元（使えない領域は `rust` へ）。失敗しても起動を続ける |
//! | `script_policy` | `task_dispatch::PolicyScript` を generation の登録簿（または `ScriptRuntimeHandle`）で実装する（付記 S2-6・S5-2） |
//! | `tick_loop` | tick ループ本体（シグナル・役割の変化・裏方の定期処理） |
//! | `admin` | API から委譲された reload / provider check / notify test |
//! | `classify_stage` / `classify_tick` | Jev 分類段と、tick の直前に ready の WU を分類して `ClassificationBoard` に置く連携（ADR 2026-10-15-jev-wire D3） |
//! | `browser_ledger_path` | 適合台帳の path 解決と `configure_conformance`（ADR 2026-10-08-browser-prod-enablement D1.4） |
//!
//! 依存は `run` → {`bootstrap`, `api`, `services`, `clusters`, `tick_loop`} → `tick_loop` → `admin` の一方向。
//! `wire_cluster_liveness_hooks` は `run` からだけ呼ぶ（`build_dispatcher` の中へ入れない）。

pub(crate) mod adapters;
pub(crate) mod admin;
pub(crate) mod api;
pub(crate) mod bootstrap;
pub(crate) mod browser_ledger_path;
pub(crate) mod classify_stage;
pub(crate) mod classify_tick;
pub(crate) mod clusters;
pub(crate) mod completion_evidence_tick;
pub(crate) mod config_admin;
mod cos_launch;
pub(crate) mod jev_key;
pub(crate) mod routing_guardrail;
pub(crate) mod run;
pub(crate) mod script_guard;
pub(crate) mod script_layer;
pub(crate) mod script_policy;
pub(crate) mod secrets;
pub(crate) mod services;
pub(crate) mod stranded_sweep;
pub(crate) mod tick_loop;
pub(crate) mod unblocks_sweep;
