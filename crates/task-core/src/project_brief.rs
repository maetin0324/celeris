//! ADR 2026-10-27-project-brief: 案件の状態の要約（目的・現状・未解決・次の手・待っている相手・根拠）。
//! 1 案件 1 行（`project_briefs`）。書き込みは全置換で、入力の検査は [`ProjectBriefInput::validate`]。

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::model::{Status, TaskId};
use crate::org::ProjectId;

/// `objective` の上限（文字数）。
pub const PROJECT_BRIEF_OBJECTIVE_MAX_CHARS: usize = 2_000;
/// `state` の上限（文字数）。
pub const PROJECT_BRIEF_STATE_MAX_CHARS: usize = 4_000;
/// 一覧の欄（`open_issues` など）の件数の上限。
pub const PROJECT_BRIEF_LIST_MAX_ITEMS: usize = 20;
/// 一覧の 1 件（と `waiting_on` の `target`・`note`）の上限（文字数）。
pub const PROJECT_BRIEF_ITEM_MAX_CHARS: usize = 500;

/// 待っている相手の種類（D2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitingOnKind {
    /// 人の判断・回答・操作。
    Human,
    /// この案件の task（`target` は task id）。
    Task,
    /// 外部（クラスタの保守・他者の返事など）。
    External,
}

impl WaitingOnKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WaitingOnKind::Human => "human",
            WaitingOnKind::Task => "task",
            WaitingOnKind::External => "external",
        }
    }
}

/// 待っている相手 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitingOn {
    pub kind: WaitingOnKind,
    /// 相手の参照（`kind = task` では必須でこの案件の task id。他は任意）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// 何を待っているか（1 行）。
    pub note: String,
}

/// `PUT /projects/{id}/status`（CoS op `project.status_put`）の本文。全置換。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectBriefInput {
    /// 案件の目的。
    pub objective: String,
    /// 今の状態（Markdown 可）。
    pub state: String,
    #[serde(default)]
    pub open_issues: Vec<String>,
    #[serde(default)]
    pub next_actions: Vec<String>,
    #[serde(default)]
    pub waiting_on: Vec<WaitingOn>,
    /// 根拠への参照（`task:<id>`・`run:<id>`・文書の path・URL など）。
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

/// 検査で見つかった誤り（欄の名前と説明）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefFieldError {
    pub field: String,
    pub message: String,
}

impl ProjectBriefInput {
    /// 前後の空白を除き、空と上限超えを誤りにする。`kind = task` の `target` は task id の形で、
    /// `is_project_task` が真であること（この案件の task）。誤りが無ければ整えた入力を返す。
    pub fn validate(
        self,
        is_project_task: &dyn Fn(TaskId) -> bool,
    ) -> Result<ProjectBriefInput, Vec<BriefFieldError>> {
        let mut errors = Vec::new();
        let mut err =
            |field: String, message: String| errors.push(BriefFieldError { field, message });
        let objective = self.objective.trim().to_string();
        let state = self.state.trim().to_string();
        for (field, value, max) in [
            ("objective", &objective, PROJECT_BRIEF_OBJECTIVE_MAX_CHARS),
            ("state", &state, PROJECT_BRIEF_STATE_MAX_CHARS),
        ] {
            if value.is_empty() {
                err(field.into(), format!("{field} must not be blank"));
            } else if value.chars().count() > max {
                err(
                    field.into(),
                    format!("{field} must be at most {max} characters"),
                );
            }
        }
        let mut list = |field: &str, items: Vec<String>| -> Vec<String> {
            if items.len() > PROJECT_BRIEF_LIST_MAX_ITEMS {
                err(
                    field.into(),
                    format!("{field} must have at most {PROJECT_BRIEF_LIST_MAX_ITEMS} items"),
                );
            }
            items
                .into_iter()
                .enumerate()
                .map(|(i, item)| {
                    let item = item.trim().to_string();
                    if item.is_empty() || item.chars().count() > PROJECT_BRIEF_ITEM_MAX_CHARS {
                        err(
                            format!("{field}[{i}]"),
                            format!("must be 1..={PROJECT_BRIEF_ITEM_MAX_CHARS} characters"),
                        );
                    }
                    item
                })
                .collect()
        };
        let open_issues = list("open_issues", self.open_issues);
        let next_actions = list("next_actions", self.next_actions);
        let evidence_refs = list("evidence_refs", self.evidence_refs);
        if self.waiting_on.len() > PROJECT_BRIEF_LIST_MAX_ITEMS {
            err(
                "waiting_on".into(),
                format!("waiting_on must have at most {PROJECT_BRIEF_LIST_MAX_ITEMS} items"),
            );
        }
        let waiting_on = self
            .waiting_on
            .into_iter()
            .enumerate()
            .map(|(i, w)| {
                let note = w.note.trim().to_string();
                if note.is_empty() || note.chars().count() > PROJECT_BRIEF_ITEM_MAX_CHARS {
                    err(
                        format!("waiting_on[{i}].note"),
                        format!("must be 1..={PROJECT_BRIEF_ITEM_MAX_CHARS} characters"),
                    );
                }
                let target = w
                    .target
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty());
                if let Some(t) = &target
                    && t.chars().count() > PROJECT_BRIEF_ITEM_MAX_CHARS
                {
                    err(
                        format!("waiting_on[{i}].target"),
                        format!("must be at most {PROJECT_BRIEF_ITEM_MAX_CHARS} characters"),
                    );
                }
                if w.kind == WaitingOnKind::Task {
                    match target.as_deref().map(str::parse::<TaskId>) {
                        Some(Ok(id)) if is_project_task(id) => {}
                        Some(Ok(_)) => err(
                            format!("waiting_on[{i}].target"),
                            "must be a task of this project".into(),
                        ),
                        _ => err(
                            format!("waiting_on[{i}].target"),
                            "kind task needs a task id as target".into(),
                        ),
                    }
                }
                WaitingOn {
                    kind: w.kind,
                    target,
                    note,
                }
            })
            .collect();
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(ProjectBriefInput {
            objective,
            state,
            open_issues,
            next_actions,
            waiting_on,
            evidence_refs,
        })
    }
}

/// 案件の状態の要約（`project_briefs` の 1 行）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectBrief {
    pub project_id: ProjectId,
    pub objective: String,
    pub state: String,
    pub open_issues: Vec<String>,
    pub next_actions: Vec<String>,
    pub waiting_on: Vec<WaitingOn>,
    pub evidence_refs: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub updated_at: OffsetDateTime,
    /// 書いた者（`cos:<thread id>`・`human`・`migration:0095`）。
    pub updated_by: String,
}

impl ProjectBrief {
    pub fn from_input(
        project_id: ProjectId,
        input: ProjectBriefInput,
        updated_by: impl Into<String>,
        updated_at: OffsetDateTime,
    ) -> Self {
        ProjectBrief {
            project_id,
            objective: input.objective,
            state: input.state,
            open_issues: input.open_issues,
            next_actions: input.next_actions,
            waiting_on: input.waiting_on,
            evidence_refs: input.evidence_refs,
            updated_at,
            updated_by: updated_by.into(),
        }
    }
}

/// 案件の task の活動（D4）。store が決定的に数える。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectActivity {
    /// task の状態（`ready` 等）ごとの件数。
    pub task_counts: BTreeMap<String, u32>,
    /// 案件の task の `updated_at` の最大（task が無ければ `None`）。
    pub last_task_update: Option<OffsetDateTime>,
}

impl ProjectActivity {
    /// 終端でない task の数。
    pub fn open_tasks(&self) -> u32 {
        self.task_counts
            .iter()
            .filter(|(status, _)| {
                serde_json::from_value::<Status>(serde_json::Value::String((*status).clone()))
                    .map(|s| !s.is_terminal())
                    .unwrap_or(false)
            })
            .map(|(_, n)| n)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ProjectBriefInput {
        ProjectBriefInput {
            objective: " 目的 ".into(),
            state: "現状".into(),
            open_issues: vec!["a".into()],
            next_actions: vec![],
            waiting_on: vec![WaitingOn {
                kind: WaitingOnKind::Human,
                target: Some("  ".into()),
                note: "承認".into(),
            }],
            evidence_refs: vec!["task:x".into()],
        }
    }

    #[test]
    fn validate_trims_and_drops_blank_target() {
        let ok = input().validate(&|_| false).unwrap();
        assert_eq!(ok.objective, "目的");
        assert_eq!(ok.waiting_on[0].target, None);
    }

    #[test]
    fn validate_rejects_blank_and_foreign_task() {
        let mut bad = input();
        bad.state = " ".into();
        bad.open_issues.push(String::new());
        bad.waiting_on.push(WaitingOn {
            kind: WaitingOnKind::Task,
            target: Some(TaskId::new().to_string()),
            note: "子".into(),
        });
        bad.waiting_on.push(WaitingOn {
            kind: WaitingOnKind::Task,
            target: None,
            note: "子".into(),
        });
        let errors = bad.validate(&|_| false).unwrap_err();
        let fields: Vec<_> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "state",
                "open_issues[1]",
                "waiting_on[1].target",
                "waiting_on[2].target"
            ]
        );
    }

    #[test]
    fn validate_accepts_own_task_and_limits_lists() {
        let id = TaskId::new();
        let mut ok = input();
        ok.waiting_on.push(WaitingOn {
            kind: WaitingOnKind::Task,
            target: Some(id.to_string()),
            note: "子".into(),
        });
        assert!(ok.clone().validate(&|t| t == id).is_ok());
        ok.next_actions = vec!["x".into(); PROJECT_BRIEF_LIST_MAX_ITEMS + 1];
        let errors = ok.validate(&|t| t == id).unwrap_err();
        assert_eq!(errors[0].field, "next_actions");
    }

    #[test]
    fn open_tasks_counts_non_terminal() {
        let mut a = ProjectActivity::default();
        a.task_counts.insert("ready".into(), 2);
        a.task_counts.insert("blocked".into(), 1);
        a.task_counts.insert("done".into(), 5);
        assert_eq!(a.open_tasks(), 3);
    }
}
