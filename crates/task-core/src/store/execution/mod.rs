//! ADR-0072 D5（Phase E2）以降の実行分解（`execution_plans` / `work_units` / `runs`）と、それに付く
//! 判断（`decisions`）・木（ADR-0079）・修復/再計画（D16/D17）の永続化。子 module の item は
//! `pub(in crate::store)` で、store の外には出さない。

mod decisions;
mod plans;
mod repair;
mod runs;
mod tree;
mod work_units;
