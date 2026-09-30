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
#[path = "pause/tests.rs"]
mod tests;
