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

/// ADR-0079 D2 / D5（Phase R1b）: 計画の採用時に停止点を解決する。/1・/2 は [`resolve_pause_points`]
/// と同じ（1 バイトも変えない）。/3 はそれに加えて `review: human` の段階を停止点にする（ADR-0074 D2 の
/// `pause_after` をその段階に指定したのと同じ意味）。並びは段階の順、重複なし。
pub fn resolve_plan_pause_points(
    policy: &PausePolicy,
    spec: &crate::execution_plan::ExecutionPlanSpec,
) -> Vec<String> {
    if spec.schema != crate::execution_plan::EXECUTION_PLAN_SCHEMA_V3 {
        return resolve_pause_points(policy, &spec.phases);
    }
    let view = crate::execution_plan::internal_view(spec);
    let from_policy = resolve_pause_points(policy, &view.phases);
    spec.stages
        .iter()
        .filter(|st| {
            st.review == crate::execution_plan::StageReview::Human
                || from_policy.iter().any(|k| k == &st.key)
        })
        .map(|st| st.key.clone())
        .collect()
}

/// D2.2: `Trigger::PhaseResume` の 2 つの選び方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseResumeMode {
    /// 次の工程へ進める。
    Continue,
    /// replan の planner run を起こす。
    Replan,
    /// ADR-0079 D8（Phase R3b）: root の計画を人が承認した（`awaiting_plan_approval` → ready）。
    PlanApprove,
    /// ADR-0079 D8（Phase R3b）: root の計画を人が note 付きで差し戻した（次の run は replan の planner）。
    PlanReplan,
}

impl PhaseResumeMode {
    pub fn name(self) -> &'static str {
        match self {
            PhaseResumeMode::Continue => "phase_continue",
            PhaseResumeMode::Replan => "phase_replan",
            PhaseResumeMode::PlanApprove => "plan_approved",
            PhaseResumeMode::PlanReplan => "plan_replan",
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
    /// ADR-0079 D5 / D11（Phase R4a）: この段階の子 task（計画の kind task の unit）ごとの要約（1 行ずつ:
    /// unit の key・子の題名・状態・subtree の run と定価・子の最新の報告の見出し）。木でない計画では空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_units: Vec<String>,
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

/// [`PhaseReport`] の借用ビュー。各 `Vec` の先頭 `keep` 要素だけを見せ、フィールド名・順序・serde 属性は
/// [`PhaseReport`] と同一（直列化結果がバイト単位で一致する）。[`PhaseReportView::new`] は
/// [`PhaseReport`] を網羅的に分解するので、フィールドを足すとここでコンパイルが止まる。
#[derive(Serialize)]
struct PhaseReportView<'a> {
    phase: &'a str,
    phase_title: &'a str,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    phases_done: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    work_units: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    child_units: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    integration: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    diff_stat: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    next_phase: &'a Option<String>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    next_phase_work_units: &'a [String],
    quota_summary: &'a str,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    artifact_paths: &'a [String],
}

/// 各 `Vec` の残す要素数（フィールドは落とす優先順に並べてある）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeepCounts {
    diff_stat: usize,
    integration: usize,
    phases_done: usize,
    work_units: usize,
    child_units: usize,
    next_phase_work_units: usize,
    artifact_paths: usize,
}

impl KeepCounts {
    fn full(r: &PhaseReport) -> Self {
        Self {
            diff_stat: r.diff_stat.len(),
            integration: r.integration.len(),
            phases_done: r.phases_done.len(),
            work_units: r.work_units.len(),
            child_units: r.child_units.len(),
            next_phase_work_units: r.next_phase_work_units.len(),
            artifact_paths: r.artifact_paths.len(),
        }
    }

    fn total(&self) -> usize {
        self.diff_stat
            + self.integration
            + self.phases_done
            + self.work_units
            + self.child_units
            + self.next_phase_work_units
            + self.artifact_paths
    }

    /// 「優先順で先頭の空でない `Vec` の末尾を 1 つ落とす」を `pops` 回繰り返した後の要素数。
    fn after_pops(&self, mut pops: usize) -> Self {
        let mut take = |n: usize| -> usize {
            let d = pops.min(n);
            pops -= d;
            n - d
        };
        // 構造体式のフィールドは書いた順に評価されるので、ここの並びが落とす優先順そのもの。
        Self {
            diff_stat: take(self.diff_stat),
            integration: take(self.integration),
            phases_done: take(self.phases_done),
            work_units: take(self.work_units),
            child_units: take(self.child_units),
            next_phase_work_units: take(self.next_phase_work_units),
            artifact_paths: take(self.artifact_paths),
        }
    }
}

impl<'a> PhaseReportView<'a> {
    fn new(r: &'a PhaseReport, k: KeepCounts) -> Self {
        let PhaseReport {
            phase,
            phase_title,
            phases_done,
            work_units,
            child_units,
            integration,
            diff_stat,
            next_phase,
            next_phase_work_units,
            quota_summary,
            artifact_paths,
        } = r;
        Self {
            phase,
            phase_title,
            phases_done: &phases_done[..k.phases_done],
            work_units: &work_units[..k.work_units],
            child_units: &child_units[..k.child_units],
            integration: &integration[..k.integration],
            diff_stat: &diff_stat[..k.diff_stat],
            next_phase,
            next_phase_work_units: &next_phase_work_units[..k.next_phase_work_units],
            quota_summary,
            artifact_paths: &artifact_paths[..k.artifact_paths],
        }
    }
}

/// 書かずにバイト数だけ数える writer（候補ごとの `Vec<u8>` 確保を避ける）。
struct ByteCounter(usize);

impl std::io::Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `pops` 回落とした後の報告の JSON 直列化のバイト数（旧実装の `serde_json::to_vec(r).len()` と同じ値。
/// 直列化の失敗は旧実装と同じく 0 扱い。文字列だけなので実際には失敗しない）。
fn byte_len_after_pops(r: &PhaseReport, full: KeepCounts, pops: usize) -> usize {
    let mut c = ByteCounter(0);
    match serde_json::to_writer(&mut c, &PhaseReportView::new(r, full.after_pops(pops))) {
        Ok(()) => c.0,
        Err(_) => 0,
    }
}

/// D2.3: `PhaseReport` を [`PHASE_REPORT_MAX_BYTES`] に収まるまで決定的に縮める（`task_core::execution::
/// truncate_checkpoint` と同じ考え方）。落とす優先順は `diff_stat` → `integration` → `phases_done` →
/// `work_units` → `child_units`（ADR-0079 R4a）→ `next_phase_work_units` → `artifact_paths`（見出し・`quota_summary`・
/// `next_phase` は残す）。
///
/// 規則（ADR-0074 D2.3。Phase SD-3 で挙動を変えずに最適化、P-SD2-1）: 上の優先順で「先頭の空でない `Vec`
/// の末尾を 1 つ落とす」操作を並べた列を考え、実際の JSON 直列化のバイト数が上限以下になる**最小の**
/// 回数 `k` だけ落とす（0 回で収まれば何もしない。全部落としても超えるなら全部落として終わる。
/// 見出しだけで超えても無限ループにはしない）。
///
/// 不変条件: 1 回落とすたびに直列化は必ず短くなる（要素とその区切りの `,` が消え、`Vec` が空になれば
/// `skip_serializing_if` でキーごと消える）。よってバイト数は落とした回数について狭義単調減少で、
/// 「上限以下」は `k` について単調な述語になり、最小の `k` を二分探索で求めてよい。各候補の長さは
/// [`PhaseReportView`]（先頭の要素だけを借用で見せる同形の構造体）を数えるだけの writer に直列化して測る
/// （判定は旧実装と同じく実際の直列化の長さで行う）。
///
/// 計算量: 要素の総数を n、直列化の長さを S として O(S · log n)。旧実装は 1 回落とすたびに全体を直列化し直す
/// O(S · n)（実質 O(n²)。1 万要素のテストが約 46 s）だった。出力は旧実装とバイト単位で同じ
/// （`tests::truncate_matches_the_reference_implementation_on_generated_inputs`）。
pub fn truncate_phase_report(report: &mut PhaseReport) {
    truncate_phase_report_to(report, PHASE_REPORT_MAX_BYTES);
}

/// [`truncate_phase_report`] の本体（上限を引数に取る。テストで境界ちょうどの上限を試すため）。
fn truncate_phase_report_to(report: &mut PhaseReport, cap: usize) {
    let full = KeepCounts::full(report);
    if byte_len_after_pops(report, full, 0) <= cap {
        return;
    }
    // 不変: `lo` 回では超える。`hi` 回では収まる、または `hi == total`（全部落とした＝旧実装が止まる点）。
    let (mut lo, mut hi) = (0usize, full.total());
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if byte_len_after_pops(report, full, mid) <= cap {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let keep = full.after_pops(hi);
    report.diff_stat.truncate(keep.diff_stat);
    report.integration.truncate(keep.integration);
    report.phases_done.truncate(keep.phases_done);
    report.work_units.truncate(keep.work_units);
    report.child_units.truncate(keep.child_units);
    report
        .next_phase_work_units
        .truncate(keep.next_phase_work_units);
    report.artifact_paths.truncate(keep.artifact_paths);
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

    /// ADR-0079 D2 / D5（Phase R1b）: /3 の `review: human` の段階は停止点になり、`pause_after` の解決と
    /// 和をとる（段階の順・重複なし）。/2 は従来の解決と同じ（`review` を持たない）。
    #[test]
    fn plan_pause_points_include_review_human_stages() {
        let v3: crate::execution_plan::ExecutionPlanSpec =
            serde_json::from_value(serde_json::json!({
                "schema": crate::execution_plan::EXECUTION_PLAN_SCHEMA_V3,
                "rationale": "r",
                "stages": [
                    {"key": "p1", "kind": "implement", "title": "Phase 1"},
                    {"key": "p2", "kind": "design", "title": "Phase 2", "review": "human"},
                    {"key": "p3", "kind": "implement", "title": "Phase 3"}
                ],
                "units": []
            }))
            .unwrap();
        assert_eq!(
            resolve_plan_pause_points(&PausePolicy::None, &v3),
            vec!["p2".to_string()]
        );
        assert_eq!(
            resolve_plan_pause_points(
                &PausePolicy::After {
                    phases: vec!["p1".into(), "p2".into()]
                },
                &v3
            ),
            vec!["p1".to_string(), "p2".to_string()]
        );
        assert_eq!(
            resolve_plan_pause_points(&PausePolicy::EachPhase, &v3),
            vec!["p1".to_string(), "p2".to_string()]
        );
        let v2 = crate::execution_plan::ExecutionPlanSpec {
            schema: crate::execution_plan::EXECUTION_PLAN_SCHEMA_V2.into(),
            rationale: "r".into(),
            phases: phases(),
            work_units: Vec::new(),
            children: Vec::new(),
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
        };
        for policy in [
            PausePolicy::None,
            PausePolicy::EachPhase,
            PausePolicy::After {
                phases: vec!["build".into()],
            },
        ] {
            assert_eq!(
                resolve_plan_pause_points(&policy, &v2),
                resolve_pause_points(&policy, &v2.phases)
            );
        }
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
            child_units: vec!["p1 Phase 1: done / 3 run / $0.40 — 完了しました".to_string()],
            integration: vec!["merged wu-a @ abc123".to_string()],
            diff_stat: vec!["src/main.rs | 3 +++".to_string()],
            next_phase: Some("verify".to_string()),
            next_phase_work_units: vec!["wu-verify".to_string()],
            quota_summary: "claude-oauth: 12% (measured)".to_string(),
            artifact_paths: vec!["artifacts/report.md".to_string()],
        }
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

    /// Phase SD-3 以前の `truncate_phase_report` の実装そのまま（上限を引数にしただけ）。
    /// 速い実装がこれとバイト単位で同じ出力を返すことを等価性テストで確かめる基準。
    fn truncate_phase_report_reference(report: &mut PhaseReport, cap: usize) {
        while report_byte_len(report) > cap {
            let shrank = pop_one(&mut report.diff_stat)
                || pop_one(&mut report.integration)
                || pop_one(&mut report.phases_done)
                || pop_one(&mut report.work_units)
                || pop_one(&mut report.child_units)
                || pop_one(&mut report.next_phase_work_units)
                || pop_one(&mut report.artifact_paths);
            if !shrank {
                // 見出しだけでも 16 KiB を超えることは実運用上ない（`quota_summary` は 1 行）が、
                // 無限ループにはしない。
                break;
            }
        }
    }

    /// 旧実装の 1 回分の pop（見出しだけになったら false）。
    fn reference_pop(r: &mut PhaseReport) -> bool {
        pop_one(&mut r.diff_stat)
            || pop_one(&mut r.integration)
            || pop_one(&mut r.phases_done)
            || pop_one(&mut r.work_units)
            || pop_one(&mut r.child_units)
            || pop_one(&mut r.next_phase_work_units)
            || pop_one(&mut r.artifact_paths)
    }

    /// 決定的な擬似乱数（splitmix64、固定 seed）。
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                (self.next() % n as u64) as usize
            }
        }
        fn string(&mut self, max_len: usize) -> String {
            // JSON でエスケープされる文字・多バイト文字・サロゲートペアになる文字を混ぜる。
            const ALPHABET: &[char] = &[
                'a', 'b', 'z', 'A', '0', '9', ' ', '|', '+', '-', '/', '.', '"', '\\', '\n', '\t',
                '\r', '\u{1}', '\u{1f}', '\u{7f}', 'é', 'ß', '日', '本', '語', '🦀', '😀',
                '\u{2028}', '\u{feff}',
            ];
            let len = self.below(max_len + 1);
            (0..len)
                .map(|_| ALPHABET[self.below(ALPHABET.len())])
                .collect()
        }
        fn strings(&mut self, max_count: usize, max_len: usize) -> Vec<String> {
            let n = self.below(max_count + 1);
            (0..n).map(|_| self.string(max_len)).collect()
        }
    }

    fn generated_report(rng: &mut Rng, max_count: usize, max_len: usize) -> PhaseReport {
        // 一部の Vec だけを大きくする・空にする形も混ぜる。
        let count = |rng: &mut Rng| match rng.below(4) {
            0 => 0,
            1 => rng.below(3),
            _ => max_count,
        };
        let c = [
            count(rng),
            count(rng),
            count(rng),
            count(rng),
            count(rng),
            count(rng),
            count(rng),
        ];
        PhaseReport {
            phase: rng.string(12),
            phase_title: rng.string(40),
            phases_done: rng.strings(c[0], max_len),
            work_units: rng.strings(c[1], max_len),
            integration: rng.strings(c[2], max_len),
            diff_stat: rng.strings(c[3], max_len),
            next_phase: if rng.below(2) == 0 {
                None
            } else {
                Some(rng.string(20))
            },
            next_phase_work_units: rng.strings(c[4], max_len),
            quota_summary: rng.string(60),
            artifact_paths: rng.strings(c[5], max_len),
            child_units: rng.strings(c[6], max_len),
        }
    }

    /// 旧実装の pop 列に沿った直列化の長さ（0 回, 1 回, …, 見出しだけ）。
    fn reference_len_sequence(r: &PhaseReport) -> Vec<usize> {
        let mut r = r.clone();
        let mut out = vec![report_byte_len(&r)];
        while reference_pop(&mut r) {
            out.push(report_byte_len(&r));
        }
        out
    }

    /// 速い実装と旧実装を同じ入力・同じ上限で回し、直列化のバイト列が一致することを確かめる。
    fn assert_equivalent(input: &PhaseReport, cap: usize, case: &str) {
        let mut fast = input.clone();
        let mut reference = input.clone();
        truncate_phase_report_to(&mut fast, cap);
        truncate_phase_report_reference(&mut reference, cap);
        let fast_bytes = serde_json::to_vec(&fast).expect("serialize fast");
        let ref_bytes = serde_json::to_vec(&reference).expect("serialize reference");
        assert!(
            fast_bytes == ref_bytes && fast == reference,
            "mismatch ({case}, cap {cap}): fast {} bytes, reference {} bytes",
            fast_bytes.len(),
            ref_bytes.len()
        );
    }

    #[test]
    fn truncate_matches_the_reference_implementation_on_generated_inputs() {
        let mut rng = Rng(0x5D3_7A11_C0FF_EE01);
        let mut cases = 0usize;
        // (レポート数, Vec あたりの最大要素数, 文字列の最大長): 任意の上限で境界を網羅する群
        // （旧実装は O(n²) なので入力は小さめ）。実際の 16 KiB 上限をまたぐ群は下で別に回す。
        for &(reports, max_count, max_len) in &[(600usize, 6usize, 8usize), (100, 12, 16)] {
            for i in 0..reports {
                let r = generated_report(&mut rng, max_count, max_len);
                let lens = reference_len_sequence(&r);
                // 不変条件（二分探索の前提）: pop するたびに直列化は狭義に短くなる。
                assert!(
                    lens.windows(2).all(|w| w[1] < w[0]),
                    "lengths not strictly decreasing: {lens:?}"
                );
                let full = lens[0];
                let headline = lens[lens.len() - 1];
                let mut caps = vec![
                    PHASE_REPORT_MAX_BYTES,
                    PHASE_REPORT_MAX_BYTES - 1,
                    PHASE_REPORT_MAX_BYTES + 1,
                    0,
                    1,
                    headline.saturating_sub(1),
                    headline,
                    headline + 1,
                    full.saturating_sub(1),
                    full,
                    full + 1,
                    usize::MAX,
                ];
                // pop 列の途中の長さちょうど・その前後（境界の上・下）。
                for _ in 0..6 {
                    let l = lens[rng.below(lens.len())];
                    caps.extend([l.saturating_sub(1), l, l + 1]);
                }
                // 範囲内の任意の上限。
                caps.push(headline + rng.below(full - headline + 1));
                for cap in caps {
                    assert_equivalent(&r, cap, &format!("report {i} max_count {max_count}"));
                    cases += 1;
                }
            }
        }
        // 実際の上限（公開関数）で、上限をまたぐ大きさの入力。
        let mut over_cap = 0usize;
        for i in 0..40 {
            let r = generated_report(&mut rng, 200, 60);
            if report_byte_len(&r) > PHASE_REPORT_MAX_BYTES {
                over_cap += 1;
            }
            let mut fast = r.clone();
            let mut reference = r.clone();
            truncate_phase_report(&mut fast);
            truncate_phase_report_reference(&mut reference, PHASE_REPORT_MAX_BYTES);
            assert_eq!(
                serde_json::to_vec(&fast).expect("serialize fast"),
                serde_json::to_vec(&reference).expect("serialize reference"),
                "public fn mismatch on large report {i}"
            );
            cases += 1;
        }
        // 見出しだけで上限を超える退化した入力（全部落として止まる）。
        let mut huge = sample_report();
        huge.quota_summary = "x".repeat(PHASE_REPORT_MAX_BYTES + 10);
        for cap in [0, 1, PHASE_REPORT_MAX_BYTES, PHASE_REPORT_MAX_BYTES + 200] {
            assert_equivalent(&huge, cap, "huge headline");
            cases += 1;
        }
        assert_equivalent(&PhaseReport::default(), 0, "empty report");
        cases += 1;
        eprintln!(
            "truncate equivalence: {cases} generated cases, all byte-identical \
             ({over_cap}/40 large reports were over the 16 KiB cap)"
        );
        assert!(cases > 10_000, "too few cases: {cases}");
        assert!(
            over_cap >= 10,
            "too few large reports over the cap: {over_cap}"
        );
    }

    /// ADR-0079 §7 R4a (c): 子の要約の行（`child_units`）も 16 KiB の切り詰めを通る。落とす順は `work_units` の後
    /// （子の要約は WU の段落より後まで残る）で、見出しと `quota_summary` は残る。
    #[test]
    fn child_unit_summaries_are_truncated_after_work_units() {
        let mut r = sample_report();
        for i in 0..400 {
            r.work_units.push(format!("wu-{i}: {}", "x".repeat(40)));
            r.child_units.push(format!(
                "c{i} child {i}: done / 2 run / $0.10 — {}",
                "y".repeat(40)
            ));
        }
        let child_before = r.child_units.clone();
        let mut expected = r.clone();
        truncate_phase_report(&mut r);
        assert!(report_byte_len(&r) <= PHASE_REPORT_MAX_BYTES);
        assert!(r.work_units.is_empty(), "work units go first");
        assert!(!r.child_units.is_empty(), "child summaries survive longer");
        assert_eq!(r.child_units[..], child_before[..r.child_units.len()]);
        assert_eq!(r.quota_summary, sample_report().quota_summary);
        truncate_phase_report_reference(&mut expected, PHASE_REPORT_MAX_BYTES);
        assert_eq!(r, expected, "same cut as the reference implementation");
    }

    /// 遅かったテストと同じ 1 万要素の入力で、旧実装と同じ点（上限以下になる最小の pop 回数）で止まることを
    /// 確かめる（旧実装そのものを回すと約 46 s かかるので、最小性で代える: 1 つ戻すと上限を超える）。
    #[test]
    fn truncate_stops_at_the_first_pop_that_fits_on_the_large_input() {
        let mut original = sample_report();
        for i in 0..5000 {
            original.diff_stat.push(format!("file-{i}.rs | 1 +"));
            original
                .integration
                .push(format!("merged wu-{i} @ deadbeef"));
        }
        let mut r = original.clone();
        truncate_phase_report(&mut r);
        assert!(report_byte_len(&r) <= PHASE_REPORT_MAX_BYTES);
        // diff_stat が全部落ち、integration の途中で止まるはず。
        assert!(r.diff_stat.is_empty());
        assert!(!r.integration.is_empty());
        assert_eq!(
            r.integration[..],
            original.integration[..r.integration.len()]
        );
        let mut one_back = r.clone();
        one_back
            .integration
            .push(original.integration[r.integration.len()].clone());
        assert!(report_byte_len(&one_back) > PHASE_REPORT_MAX_BYTES);
        assert_eq!(r.phases_done, original.phases_done);
        assert_eq!(r.work_units, original.work_units);
        assert_eq!(r.next_phase_work_units, original.next_phase_work_units);
        assert_eq!(r.artifact_paths, original.artifact_paths);
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
