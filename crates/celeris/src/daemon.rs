//! デーモンの配線。`lib.rs` は入口（`mod` 宣言・`DaemonError` / `RunOptions` / `Exit`・再公開）だけを持ち、
//! 起動から停止までの配線はこの下に責務ごとに置く。
//!
//! | module | 中身 |
//! |---|---|
//! | `secrets` | `[secrets]` の解決と env の合成（ADR-0030）、実効モデル |
//! | `adapters` | `build_adapters`・`secret_usage`・`effective_models`・`provider_lives` |
//! | `bootstrap` | DB の置き場所の検査、`build_dispatcher`（store を開き migrate する）、`seed_org_if_empty`・`seed_cron_if_empty` |
//! | `clusters` | ssh master・liveness hook・tunnel の配線（ADR-0032 / ADR-0062） |
//! | `api` | `config_view`・`api_settings`・`bind_reuseport`・`start_api` |
//! | `services` | llm-proxy と MCP の組み立て・起動・停止 |
//! | `run` | `run`: 起動順と停止順（interrupt → api → llm_proxy → mcp → checkpoint → backup → deregister） |
//! | `tick_loop` | tick ループ本体（シグナル・役割の変化・裏方の定期処理） |
//! | `admin` | API から委譲された reload / provider check / notify test |
//! | `browser_ledger_path` | 適合台帳の path 解決と `configure_conformance`（ADR 2026-10-08-browser-prod-enablement D1.4） |
//!
//! 依存は `run` → {`bootstrap`, `api`, `services`, `clusters`, `tick_loop`} → `tick_loop` → `admin` の一方向。
//! `wire_cluster_liveness_hooks` は `run` からだけ呼ぶ（`build_dispatcher` の中へ入れない）。

pub(crate) mod adapters;
pub(crate) mod admin;
pub(crate) mod api;
pub(crate) mod bootstrap;
pub(crate) mod browser_ledger_path;
pub(crate) mod clusters;
pub(crate) mod routing_shadow;
pub(crate) mod routing_sidecar;
pub(crate) mod run;
pub(crate) mod secrets;
pub(crate) mod services;
pub(crate) mod tick_loop;
