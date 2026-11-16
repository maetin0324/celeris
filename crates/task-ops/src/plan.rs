//! 撤去済みの `POST /plans`（ADR-0079 R5a で 410）の本文の型。
//!
//! `kind = plan` の根の task を作る入口（`celerisctl plan` と `create_plan`）は R6 で消した。新しい仕事は root task と
//! その計画（`celeris.execution-plan/3`、`celerisctl execution plan set`）で表す。この型は `task-api` の schema
//! （`schema.rs` の `new_plan`）と生成 API schema だけが参照する。

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::Tier;

// 下の `///` は生成 API schema（`docs/api/v1/api-v1.schema.json`・`web/api/generated/schema.json`）の description に
// 写る。schema はこの unit の範囲の外なので文言を変えない（型ごと消す案は R6 の進捗の「提案」）。
/// `celerisctl plan` から組み立てる新規 Plan タスクの指定。API の `POST /plans` の本文でもある（`docs/api/v1/api.md` §3.14）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewPlanSpec {
    /// 大目標。1行目の先頭80文字が `title` になる。
    pub goal: String,
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default = "default_plan_tier")]
    pub tier: Tier,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_plan_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_plan_max_wall_secs")]
    pub max_wall_secs: u64,
    #[serde(default = "default_plan_max_retries")]
    pub max_retries: u32,
}

fn default_plan_tier() -> Tier {
    Tier::Frontier
}
fn default_plan_max_turns() -> u32 {
    30
}
fn default_plan_max_wall_secs() -> u64 {
    900
}
fn default_plan_max_retries() -> u32 {
    1
}
