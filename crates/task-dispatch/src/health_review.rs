//! ADR 2026-11-07-health-review-integration: 健全性を既存の planner・reviewer の経路に組み込む dispatcher 側の処理。
//!
//! `dispatcher/` は module 全体が repo health の `size.responsibility-growth` の閾値を超えているので、新しい処理は
//! この top-level module に置く。`dispatcher/` に足すのは呼び出しの配線だけで、その行は同じ file の既存の行の
//! 整理で相殺する。どの下位 module も LLM を呼ばない（I4）。
//!
//! - [`policy`]: review 方針を base（target）側の検査器で読む（D2.3・I3）。
//! - `health_review/worker_profile.rs`: 読んだ方針を worker と planner の run の `RunContext.workspace` に渡す（付記 B1〜B5）。
//!   `dispatcher::run_context` の子 module として `#[path]` で読む。
//! - [`design`]: 計画の採用前の risk 判定と高 risk だけの設計 review（D3.3・D3.5）の純粋な部分。Dispatcher の状態（実行中の
//!   run・完了の channel）を使う配線は `health_review/design_gate.rs` で、`dispatcher` の子 module として `#[path]` で読む。
//! - [`input`]: final review の health 文脈を candidate SHA ごとに 1 回作る（D4）。
//! - [`verdict`]: 構造化 health verdict の blocking を不合格にし repair/replan に写す（D5）。
//! - `health_review/inventory.rs`: cron の全体棚卸しの runner（ADR 2026-11-08-health-enforcement-loop D6）。Dispatcher の欄
//!   （Busy の印）が型を使うので、`dispatcher::cron_tick` の子 module として `#[path]` で読む。

pub(crate) mod design;
pub mod gate;
pub(crate) mod input;
pub mod policy;
pub(crate) mod verdict;
