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

/// D2.2: `Trigger::PhaseResume` の 2 つの選び方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseResumeMode {
    /// 次の工程へ進める。
    Continue,
    /// replan の planner run を起こす。
    Replan,
}

impl PhaseResumeMode {
    pub fn name(self) -> &'static str {
        match self {
            PhaseResumeMode::Continue => "phase_continue",
            PhaseResumeMode::Replan => "phase_replan",
        }
    }
}

/// D2.3: 16 KiB 上限（`task_core::execution::CHECKPOINT_MAX_BYTES` と同じ規律）。
pub const PHASE_REPORT_MAX_BYTES: usize = 16 * 1024;

/// D2.3: 途中報告そのもの（`Event::PhaseReported.report` と `artifacts/phase-reports/<n>-<phase>.md`
/// が同じ内容を持つ）。決定的に組み立てる（LLM は使わない）。各フィールドは既に人が読める 1 行・
/// 1 段落の文字列にしてある（型を増やしすぎず、レンダリングと切り詰めを単純にするため）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PhaseReport {
    /// 止まった工程の key。
    pub phase: String,
    /// 止まった工程の title。
    pub phase_title: String,
    /// 済んだ工程の一覧（1 行ずつ: title・WU の数・run の数・壁時計）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases_done: Vec<String>,
    /// この工程の WU ごとの要約（1 段落ずつ）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub work_units: Vec<String>,
    /// 統合の結果（merge・衝突・検査。1 行ずつ）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration: Vec<String>,
    /// Task ブランチの `git diff --stat` の要約（最大 30 行）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff_stat: Vec<String>,
    /// 次の工程の key・title（無ければ `None` = 実質最後の工程）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_phase: Option<String>,
    /// 次の工程の WU の title の一覧。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_phase_work_units: Vec<String>,
    /// 使った quota と参考の定価（ここまでの合計。1 行）。
    pub quota_summary: String,
    /// 成果物へのリンク（workspace 相対パス）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_paths: Vec<String>,
}

fn report_byte_len(r: &PhaseReport) -> usize {
    serde_json::to_vec(r).map(|v| v.len()).unwrap_or(0)
}

fn pop_one(v: &mut Vec<String>) -> bool {
    if v.is_empty() {
        false
    } else {
        v.pop().is_some()
    }
}

/// D2.3: `PhaseReport` を [`PHASE_REPORT_MAX_BYTES`] に収まるまで決定的に縮める（`task_core::execution::
/// truncate_checkpoint` と同じ考え方）。落とす優先順は `diff_stat` → `integration` → `phases_done` →
/// `work_units` → `next_phase_work_units` → `artifact_paths`（見出し・`quota_summary`・`next_phase` は残す）。
pub fn truncate_phase_report(report: &mut PhaseReport) {
    while report_byte_len(report) > PHASE_REPORT_MAX_BYTES {
        let shrank = pop_one(&mut report.diff_stat)
            || pop_one(&mut report.integration)
            || pop_one(&mut report.phases_done)
            || pop_one(&mut report.work_units)
            || pop_one(&mut report.next_phase_work_units)
            || pop_one(&mut report.artifact_paths);
        if !shrank {
            // 見出しだけでも 16 KiB を超えることは実運用上ない（`quota_summary` は 1 行）が、
            // 無限ループにはしない。
            break;
        }
    }
}

/// D2.3: ミリ秒を人が読む壁時計表記にする（`Xm` / `XhYm`。純粋関数、決定的）。
pub fn format_wall_ms(ms: i64) -> String {
    let total_secs = ms.max(0) / 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    if h > 0 {
        format!("{h}h{m}m")
    } else {
        format!("{m}m")
    }
}

/// D2.3: 「使った quota と参考の定価」の 1 行（GUI の `quotaSummaryLines`/`costReferenceLabel`
/// と同じ考え方を決定的な 1 行に畳んだもの。ADR-0074 D4 の quota 側とは独立: ここでは記録済みの
/// `ExecutionMetrics` を読むだけで、新しい判断はしない）。
pub fn quota_summary_line(metrics: &crate::execution_metrics::ExecutionMetrics) -> String {
    let cost = match (metrics.cost_usd, metrics.cost_usd_complete) {
        (Some(v), true) => format!("参考 ${v:.2}"),
        (Some(v), false) => format!("参考 ${v:.2}（不完全）"),
        (None, _) => "参考 不明".to_string(),
    };
    if metrics.quota.is_empty() {
        return format!("quota: 消費なし・{cost}");
    }
    let mut parts: Vec<String> = metrics
        .quota
        .iter()
        .map(|q| {
            let account = q.account.as_deref().unwrap_or(q.source.as_str());
            let pct = q
                .used_pct
                .map(|p| format!("{p:.1}pt"))
                .unwrap_or_else(|| "不明".to_string());
            format!("{account} {} {pct}", q.window.as_str())
        })
        .collect();
    parts.sort();
    format!("{}・{cost}", parts.join(" / "))
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

    fn sample_report() -> PhaseReport {
        PhaseReport {
            phase: "build".into(),
            phase_title: "build".into(),
            phases_done: vec!["design: 2 WU / 3 run / 12m".to_string()],
            work_units: vec!["wu-a: did the thing".to_string()],
            integration: vec!["merged wu-a @ abc123".to_string()],
            diff_stat: vec!["src/main.rs | 3 +++".to_string()],
            next_phase: Some("verify".to_string()),
            next_phase_work_units: vec!["wu-verify".to_string()],
            quota_summary: "claude-oauth: 12% (measured)".to_string(),
            artifact_paths: vec!["artifacts/report.md".to_string()],
        }
    }

    #[test]
    fn truncate_is_a_no_op_under_the_cap() {
        let mut r = sample_report();
        let before = r.clone();
        truncate_phase_report(&mut r);
        assert_eq!(r, before);
    }

    #[test]
    fn truncate_shrinks_to_the_overall_byte_cap_and_keeps_the_headline() {
        let mut r = sample_report();
        for i in 0..5000 {
            r.diff_stat.push(format!("file-{i}.rs | 1 +"));
            r.integration.push(format!("merged wu-{i} @ deadbeef"));
        }
        truncate_phase_report(&mut r);
        assert!(
            report_byte_len(&r) <= PHASE_REPORT_MAX_BYTES,
            "got {}",
            report_byte_len(&r)
        );
        assert_eq!(r.phase, "build");
        assert_eq!(r.quota_summary, "claude-oauth: 12% (measured)");
    }

    #[test]
    fn format_wall_ms_examples() {
        assert_eq!(format_wall_ms(0), "0m");
        assert_eq!(format_wall_ms(59_000), "0m");
        assert_eq!(format_wall_ms(60_000), "1m");
        assert_eq!(format_wall_ms(12 * 60_000), "12m");
        assert_eq!(format_wall_ms(3_600_000 + 5 * 60_000), "1h5m");
        assert_eq!(format_wall_ms(-1), "0m", "負値は 0 として扱う");
    }

    #[test]
    fn quota_summary_line_reports_no_consumption_when_metrics_has_no_quota() {
        let task = crate::model_policy::tests::task("o", vec![]);
        let metrics = crate::execution_metrics::summarize(&task, &[]);
        let line = quota_summary_line(&metrics);
        assert!(line.starts_with("quota: 消費なし"), "{line}");
        assert!(line.contains("参考"), "{line}");
    }

    #[test]
    fn quota_summary_line_formats_measured_quota_and_cost() {
        use crate::model::Event;
        use crate::quota::{QuotaMethod, QuotaWindow, QuotaWindowUse};
        let task = crate::model_policy::tests::task("o", vec![]);
        let events = vec![Event::QuotaEstimated {
            run_id: "r1".to_string(),
            work_unit_id: None,
            source: "claude-oauth".to_string(),
            account: Some("acct-a".to_string()),
            windows: vec![QuotaWindowUse {
                window: QuotaWindow::FiveHour,
                before: None,
                after: None,
                resets_at: None,
                used_pct: Some(3.25),
                method: QuotaMethod::Measured,
            }],
            weighted_tokens: 100.0,
            method: QuotaMethod::Measured,
            calibration: None,
            weights_version: "quota-weights/1".to_string(),
            list_price_usd: Some(1.5),
        }];
        let metrics = crate::execution_metrics::summarize(&task, &events);
        let line = quota_summary_line(&metrics);
        assert!(line.contains("acct-a"), "{line}");
        assert!(line.contains("five_hour"), "{line}");
        assert!(line.contains("3.3pt") || line.contains("3.2pt"), "{line}");
        assert!(
            line.contains("参考 $1.50") || line.contains("参考 不明"),
            "{line}"
        );
    }
}
