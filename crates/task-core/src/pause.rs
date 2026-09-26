//! ADR-0074 D2（Phase F3 途中確認）: `PausePolicy` とその解決のデータ定義と純粋関数。
//! I/O・LLM 呼び出しはしない（ADR-0001 D2）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// D2.1: `pause_after` を誰が書いたか（ADR-0069 D1 の `SpecOrigin` と同じ考え方。`task_ops::add::SpecOrigin`
/// は task-ops 側の内部型なので、`Event` から見える task-core 側にこの小さな型を別に置く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PauseSource {
    /// 人（API・GUI・`celerisctl`。既定）。
    #[default]
    Human,
    /// CoS（`create_task.pause_after` / 案件計画のマイルストーン）。
    Agent,
}

/// D2.1: `NewTaskSpec.pause_after` / `PATCH` / `PUT /tasks/{id}/execution/pause-after` /
/// CoS の `create_task.pause_after` に書ける値。既定は `None`（全工程自動）。
/// **planner は書けない**（`ExecutionPlanSpec` に欄が無い。`deny_unknown_fields` で拒否される）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PausePolicy {
    /// 既定。全工程自動（統合の後で止まらない）。
    #[default]
    None,
    /// 最後の工程を除くすべての工程の後で止まる。
    EachPhase,
    /// 挙げた工程（`key` か `kind` に一致するもの）の後で止まる。
    After {
        #[serde(default)]
        phases: Vec<String>,
    },
}

impl PausePolicy {
    /// serde の `skip_serializing_if` 用（既定 `none` は JSON に出さない）。
    pub fn is_default(&self) -> bool {
        matches!(self, PausePolicy::None)
    }
}

/// D2.1: 採用時に工程の key の集合へ解決する（純粋関数）。`phases` は計画の `PhaseSpec` の並び
/// （実行順）。v1/atomic（`phases` が空）は常に空集合を返す（「工程を持たないタスクには効かない」）。
///
/// `EachPhase` は最後の工程を除くすべて。`After` は、挙げた文字列が工程の `key` か `kind`
/// （`WorkUnitKind::as_str()`）のどちらかに一致する工程を選ぶ（計画が採用される前でも
/// `"design"`/`"implement"` のような種類名で書けるようにするため）。
pub fn resolve_pause_points(
    policy: &PausePolicy,
    phases: &[crate::execution_plan::PhaseSpec],
) -> Vec<String> {
    if phases.is_empty() {
        return Vec::new();
    }
    match policy {
        PausePolicy::None => Vec::new(),
        PausePolicy::EachPhase => phases[..phases.len().saturating_sub(1)]
            .iter()
            .map(|p| p.key.clone())
            .collect(),
        PausePolicy::After { phases: wanted } => phases
            .iter()
            .filter(|p| {
                wanted
                    .iter()
                    .any(|w| w == &p.key || w.as_str() == p.kind.as_str())
            })
            .map(|p| p.key.clone())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_plan::{PhaseSpec, WorkUnitKind};

    fn phases() -> Vec<PhaseSpec> {
        vec![
            PhaseSpec {
                key: "design".into(),
                kind: WorkUnitKind::Design,
                title: "design".into(),
            },
            PhaseSpec {
                key: "build".into(),
                kind: WorkUnitKind::Implement,
                title: "build".into(),
            },
            PhaseSpec {
                key: "verify".into(),
                kind: WorkUnitKind::Test,
                title: "verify".into(),
            },
        ]
    }

    #[test]
    fn none_never_pauses() {
        assert_eq!(
            resolve_pause_points(&PausePolicy::None, &phases()),
            Vec::<String>::new()
        );
        assert_eq!(
            resolve_pause_points(&PausePolicy::None, &[]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn each_phase_excludes_the_last_phase() {
        assert_eq!(
            resolve_pause_points(&PausePolicy::EachPhase, &phases()),
            vec!["design".to_string(), "build".to_string()]
        );
        // 1 工程しかない計画では、除いた残りが空。
        let one = vec![PhaseSpec {
            key: "only".into(),
            kind: WorkUnitKind::Other,
            title: "only".into(),
        }];
        assert_eq!(
            resolve_pause_points(&PausePolicy::EachPhase, &one),
            Vec::<String>::new()
        );
    }

    #[test]
    fn after_matches_by_key_or_kind() {
        let by_key = PausePolicy::After {
            phases: vec!["build".to_string()],
        };
        assert_eq!(
            resolve_pause_points(&by_key, &phases()),
            vec!["build".to_string()]
        );
        // `kind` でも当たる（計画の前に書ける。D2.1）。
        let by_kind = PausePolicy::After {
            phases: vec!["design".to_string()],
        };
        assert_eq!(
            resolve_pause_points(&by_kind, &phases()),
            vec!["design".to_string()]
        );
        let unknown = PausePolicy::After {
            phases: vec!["release".to_string()],
        };
        assert_eq!(
            resolve_pause_points(&unknown, &phases()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn v1_and_atomic_plans_have_no_phases_so_nothing_resolves() {
        for policy in [
            PausePolicy::None,
            PausePolicy::EachPhase,
            PausePolicy::After {
                phases: vec!["design".to_string()],
            },
        ] {
            assert_eq!(resolve_pause_points(&policy, &[]), Vec::<String>::new());
        }
    }
}
