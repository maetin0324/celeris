//! ADR-0048 D3（Phase 60b）: CoS の結果ファイルが宣言する `actions` の**形**だけを持つ。
//!
//! 読む（`task_worker::result_report`。ファイル I/O）のも実行する（`task_ops::actions`。ストアの
//! 読み書き）のも別の crate なので、両方が依存できる `task-core` にこの形だけを置く
//! （ADR-0034 D7 / ADR-0038 D1 の宣言的フィールドと同じ流儀）。判断も I/O もここには無い。
//!
//! ```json
//! {"summary": "…", "actions": [
//!   {"type": "create_task", "title": "…", "objective": "…", "acceptance": [...], "harness": "coding",
//!    "skills": ["rust"], "mode": "prototype", "repos": ["agent-platform"], "project": "<id or null>",
//!    "assignee": null, "stages_hint": [{"title": "Phase 1", "scope": "…"}],
//!    "workspace": {"kind": "remote", "cluster": "<id>", "path": "<remote dir or ~>"}},
//!   {"type": "propose_project", "title": "…", "request": "…", "repos": [...]},
//!   {"type": "ask_human", "text": "…"}
//! ]}
//! ```
//!
//! Phase 98（ADR-0018、実機障害 2026-09-22）: `create_task.workspace` は `WorkspaceSpec` そのもの
//! （`{"kind":"local","path":"…"}` または `{"kind":"remote","cluster":"…","path":"…"}`）。CoS がクラスタ作業
//! （pegasus / sirius / fern03 でのコマンド実行）を`cluster:<id>` を持つノードへ流すときに使う。実行側
//! （`task_ops::actions::create_task_action`）が `cluster` を `[[clusters]]` に照らして検証する。
//!
//! ADR-0074 D2.1（Phase F3 途中確認）: `create_task.pause_after` は工程ごとに人の確認を挟みたいときに
//! 付ける（`{"mode":"none"}`（既定）/ `{"mode":"each_phase"}` / `{"mode":"after","phases":["design"]}`）。
//! 出自は `PauseSource::Agent`（人の明示より安全側に倒すので、CoS の値もそのまま採る。ADR-0069 D1 が
//! `assignee`/`tier` を捨てるのとは扱いが違う）。
//!
//! ADR-0079 D12（Phase R5a）: `add_milestone` は廃止（この型から外した。`task_worker::result_report` が
//! 「途中目標は root task の段階で表す（ADR-0079）」の理由付きで落とし、人に見える）。人が段階を名指ししたときは
//! `create_task.stages_hint: [{"title": "Phase 1", "scope": "…"}]` に写す（`Task.routing.stages_hint`。planner への入力）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 1 件の action（ADR-0048 D3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConsoleAction {
    CreateTask {
        #[serde(default)]
        tier: Option<crate::Tier>,
        title: String,
        objective: String,
        #[serde(default, deserialize_with = "lenient_acceptance")]
        acceptance: Vec<String>,
        #[serde(default)]
        harness: Option<String>,
        #[serde(default)]
        skills: Vec<String>,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        repos: Vec<String>,
        #[serde(default)]
        project: Option<String>,
        // ADR-0079 D12 / D13（Phase R5a）: `milestone` は外した（途中目標は凍結。CoS は案件〈方向〉だけを選ぶ）。
        // 旧い結果ファイルの `"milestone": …` は未知の欄として読み飛ばす（action 全体は落とさない）。
        #[serde(default)]
        assignee: Option<String>,
        /// Phase 98（ADR-0018）: クラスタで動く仕事を指すときの作業場所。`{"kind":"remote",
        /// "cluster":"<id>","path":"<クラスタ側のパスか ~>"}` か `{"kind":"local","path":"…"}`。
        /// `cluster` は `[[clusters]]` に存在すること（実行側が検証する）。`Box` は
        /// `clippy::large_enum_variant`（`WorkspaceSpec` を直に持つと `CreateTask` だけ他の variant
        /// より大きく膨らむ）を避けるためだけで、意味は変わらない。
        #[serde(default)]
        workspace: Option<Box<crate::model::WorkspaceSpec>>,
        /// ADR-0069 D3（Phase 114）: 仕事の性質の記述（lane policy の `TaskFeatures` の上書き）。
        /// モデルの選択ではない。書いた軸だけが効く。
        #[serde(default)]
        features: Option<crate::model_policy::TaskFeatureHints>,
        /// ADR-0072 D13（Phase E3）: 大きな依頼は `"compound"` のヒントを付けてよい（判定そのものは
        /// Complexity Gate が決定的に行う。CoS の明示は signal `H` として +2 されるだけで、
        /// gate をバイパスしない）。
        #[serde(default)]
        execution: Option<crate::execution_gate::ExecutionMode>,
        /// ADR-0074 D2.1（Phase F3 途中確認）: 工程ごとに人の確認が要りそうなら付けてよい
        /// （`none`/`each_phase`/`after`）。出自は CoS（`PauseSource::Agent`）として記録される。
        /// `Box` は `clippy::large_enum_variant`（`workspace` と同じ理由）を避けるためだけで、
        /// 意味は変わらない。
        #[serde(default)]
        pause_after: Option<Box<crate::pause::PausePolicy>>,
        /// ADR-0079 D12（Phase R5a）: 人が段階（「Phase 1〜4」など）を名指ししたときだけ、その名前と範囲を
        /// そのまま写す（`Task.routing.stages_hint`。root の planner への入力で、構造の強制ではない）。
        #[serde(default)]
        stages_hint: Vec<crate::tree::StageHint>,
    },
    ProposeProject {
        title: String,
        request: String,
        #[serde(default)]
        repos: Vec<String>,
    },
    AskHuman {
        text: String,
    },
}

impl ConsoleAction {
    /// 人が読む種類の名前（`Message.metadata` / ログに使う）。
    pub fn kind(&self) -> &'static str {
        match self {
            ConsoleAction::CreateTask { .. } => "create_task",
            ConsoleAction::ProposeProject { .. } => "propose_project",
            ConsoleAction::AskHuman { .. } => "ask_human",
        }
    }
}

/// 実機 2026-09-20: CoS は `acceptance` をタスクの実際の形（`{"text": "…", "check": {…}}`）で書くことがあり、
/// 文字列しか受けない型だと action 全体が「invalid type: map, expected a string」で捨てられた（タスクが作られない）。
/// 文字列でも、`text` を持つオブジェクトでも受け、どちらも受け入れ条件の**文**として扱う（判定の方法は実行側が決める）。
fn lenient_acceptance<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<serde_json::Value> = Deserialize::deserialize(deserializer)?;
    let mut out = Vec::with_capacity(raw.len());
    for item in raw {
        match item {
            serde_json::Value::String(text) => out.push(text),
            serde_json::Value::Object(map) => match map.get("text").and_then(|t| t.as_str()) {
                Some(text) => out.push(text.to_string()),
                None => {
                    return Err(serde::de::Error::custom(
                        "acceptance の各要素は文字列か、`text` を持つオブジェクト",
                    ));
                }
            },
            _ => {
                return Err(serde::de::Error::custom(
                    "acceptance の各要素は文字列か、`text` を持つオブジェクト",
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod lenient_tests {
    use super::*;

    #[test]
    fn acceptance_accepts_strings_and_criterion_objects() {
        let json = r#"{"type":"create_task","title":"t","objective":"o",
            "acceptance":["文だけ", {"text":"オブジェクト","check":{"type":"reviewer"}}]}"#;
        let action: ConsoleAction = serde_json::from_str(json).expect("parse");
        let ConsoleAction::CreateTask { acceptance, .. } = action else {
            panic!("create_task")
        };
        assert_eq!(
            acceptance,
            vec!["文だけ".to_string(), "オブジェクト".to_string()]
        );
        let bad =
            r#"{"type":"create_task","title":"t","objective":"o","acceptance":[{"check":{}}]}"#;
        assert!(serde_json::from_str::<ConsoleAction>(bad).is_err());
    }
}
