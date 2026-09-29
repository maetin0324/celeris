//! ADR-0079 D11（Phase R4a）: 木の roll-up（節点ごとに「自分の分」と「subtree の合計」）。
//!
//! 純粋なデータ定義と純粋関数だけを置く（I/O・LLM 呼び出しはしない。ADR-0001 D2 / ADR-0079 D16）。
//! 材料（[`RollupNodeFacts`]）は store の読み取りだけで集める（`task_ops::tree_view`）: run は `runs` の索引、
//! unit は `work_units`、quota は events の `QuotaEstimated` を畳んだもの（`execution_metrics::summarize` と同じ）、
//! 未回答の決定は `decisions` の表。同じ関数を `GET /tasks/{id}/task-tree`・`GET /projects/{id}` の root の合計・
//! `GET /metrics/execution?group_by=depth`（U-R7 の指標）が使う。
//!
//! 合計の規則（[`RollupMetrics::absorb`]）: 件数・トークン・定価・quota は和、`cost_usd_complete` は論理積、
//! 壁時計は「最初の run の開始 → 最後の run の終わり」なので最小と最大（和ではない）、実働時間
//! （`busy_ms`、終わった run の長さ）は和。したがって節点の `subtree` は、自分と子孫の `own` を
//! [`RollupMetrics::absorb`] で畳んだものと常に一致する（root の合計 = subtree の和）。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::execution_plan::{
    RunIndexRole, RunIndexStatus, RunRow, WorkUnitKind, WorkUnitRow, WorkUnitStatus,
};
use crate::model::TaskId;
use crate::quota::QuotaUse;

/// D11: 1 節点分（または subtree・深さ・案件の合計）の数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RollupMetrics {
    /// 数えた task（節点）の数。
    pub tasks: u32,
    /// role ごとの run（`runs` の索引の role: `worker` / `planner` / `reviewer` / `wrap_up`。reviewer を含む）。
    #[serde(default)]
    pub runs_by_role: BTreeMap<String, u32>,
    /// reviewer を除く run（`max_tree_runs` と同じ数え方）。
    pub runs: u32,
    /// U-R7: reviewer の run と、その定価（USD）。
    pub reviewer_runs: u32,
    pub reviewer_cost_usd: f64,
    /// まだ終わっていない run（`runs.status = running`）。
    pub runs_in_flight: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// input + output（cache は含めない。`TreeCounters.tokens` と同じ）。
    pub tokens: u64,
    /// 定価（USD。reviewer を含む）。`cost_usd_complete = false` なら下限。
    pub cost_usd: f64,
    /// token を持つのに定価の無い run（単価表に無いモデル）が 1 件も無い。
    pub cost_usd_complete: bool,
    /// ADR-0074 D4: (source, account, window) ごとの quota（`merge_quota_use` で合計）。
    #[serde(default)]
    pub quota: Vec<QuotaUse>,
    /// 最初の run の開始（RFC 3339）。run が無ければ無し。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_run_started_at: Option<String>,
    /// 最後に終わった run の終わり（RFC 3339）。終わった run が無ければ無し。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_finished_at: Option<String>,
    /// 壁時計: `first_run_started_at` → `last_run_finished_at`（ミリ秒）。どちらかが無ければ無し。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    /// 実働時間: 終わった run の（開始 → 終わり）の和（ミリ秒）。
    pub busy_ms: u64,
    /// leaf（統合・repair・kind task を除く `work_units` の行。superseded / cancelled を除く）。
    pub leaves_total: u32,
    pub leaves_done: u32,
    /// 計画の kind task の unit（superseded / cancelled を除く）。
    pub child_tasks_total: u32,
    pub child_tasks_done: u32,
    /// 未回答の決定（その節点が出したもの）。
    pub open_decisions: u32,
}

impl Default for RollupMetrics {
    fn default() -> Self {
        RollupMetrics {
            tasks: 0,
            runs_by_role: BTreeMap::new(),
            runs: 0,
            reviewer_runs: 0,
            reviewer_cost_usd: 0.0,
            runs_in_flight: 0,
            input_tokens: 0,
            output_tokens: 0,
            tokens: 0,
            cost_usd: 0.0,
            cost_usd_complete: true,
            quota: Vec::new(),
            first_run_started_at: None,
            last_run_finished_at: None,
            wall_ms: None,
            busy_ms: 0,
            leaves_total: 0,
            leaves_done: 0,
            child_tasks_total: 0,
            child_tasks_done: 0,
            open_decisions: 0,
        }
    }
}

fn parse(ts: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(ts, &Rfc3339).ok()
}

/// 2 つの時刻の文字列のうち早い方 / 遅い方（読めない値は読める値に負ける。どちらも読めなければ `a`）。
fn pick(a: Option<String>, b: Option<&String>, later: bool) -> Option<String> {
    match (a, b) {
        (None, None) => None,
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b.clone()),
        (Some(a), Some(b)) => match (parse(&a), parse(b)) {
            (Some(x), Some(y)) => {
                if (later && y > x) || (!later && y < x) {
                    Some(b.clone())
                } else {
                    Some(a)
                }
            }
            (None, Some(_)) => Some(b.clone()),
            _ => Some(a),
        },
    }
}

impl RollupMetrics {
    fn refresh_wall(&mut self) {
        self.wall_ms = match (
            self.first_run_started_at.as_deref().and_then(parse),
            self.last_run_finished_at.as_deref().and_then(parse),
        ) {
            (Some(s), Some(e)) if e >= s => Some((e - s).whole_milliseconds() as u64),
            (Some(_), Some(_)) => Some(0),
            _ => None,
        };
    }

    /// `other` をこの合計に足す（件数・トークン・定価・quota は和、完全性は論理積、壁時計は最小と最大）。
    pub fn absorb(&mut self, other: &RollupMetrics) {
        self.tasks += other.tasks;
        for (role, n) in &other.runs_by_role {
            *self.runs_by_role.entry(role.clone()).or_default() += n;
        }
        self.runs += other.runs;
        self.reviewer_runs += other.reviewer_runs;
        self.reviewer_cost_usd += other.reviewer_cost_usd;
        self.runs_in_flight += other.runs_in_flight;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.tokens += other.tokens;
        self.cost_usd += other.cost_usd;
        self.cost_usd_complete = self.cost_usd_complete && other.cost_usd_complete;
        if !other.quota.is_empty() {
            let mut rows = std::mem::take(&mut self.quota);
            rows.extend(other.quota.iter().cloned());
            self.quota = crate::quota::merge_quota_use(rows);
        }
        self.first_run_started_at = pick(
            self.first_run_started_at.take(),
            other.first_run_started_at.as_ref(),
            false,
        );
        self.last_run_finished_at = pick(
            self.last_run_finished_at.take(),
            other.last_run_finished_at.as_ref(),
            true,
        );
        self.refresh_wall();
        self.busy_ms += other.busy_ms;
        self.leaves_total += other.leaves_total;
        self.leaves_done += other.leaves_done;
        self.child_tasks_total += other.child_tasks_total;
        self.child_tasks_done += other.child_tasks_done;
        self.open_decisions += other.open_decisions;
    }

    /// 複数の合計を 1 つに畳む（空なら 0 件の合計）。
    pub fn sum<'a>(items: impl IntoIterator<Item = &'a RollupMetrics>) -> RollupMetrics {
        let mut out = RollupMetrics::default();
        for m in items {
            out.absorb(m);
        }
        out
    }
}

/// [`node_metrics`] / [`rollup`] の入力: 木の 1 節点の事実（store の読み取りだけで集める）。
#[derive(Debug, Clone)]
pub struct RollupNodeFacts {
    pub task_id: TaskId,
    /// 木の親（`Task.tree.parent_unit.task_id`）。root・木でない task は無し。
    pub parent_id: Option<TaskId>,
    /// `tree::depth_of`（木でない task は 1）。
    pub depth: u32,
    pub runs: Vec<RunRow>,
    pub work_units: Vec<WorkUnitRow>,
    /// この節点の quota（`execution_metrics::summarize(..).quota`）。読まない呼び出し側は空。
    pub quota: Vec<QuotaUse>,
    pub open_decisions: u32,
}

/// D11: 1 節点の自分の分（純粋関数）。
pub fn node_metrics(facts: &RollupNodeFacts) -> RollupMetrics {
    let mut m = RollupMetrics {
        tasks: 1,
        open_decisions: facts.open_decisions,
        quota: facts.quota.clone(),
        ..RollupMetrics::default()
    };
    for run in &facts.runs {
        *m.runs_by_role
            .entry(run.role.as_str().to_string())
            .or_default() += 1;
        let reviewer = run.role == RunIndexRole::Reviewer;
        if reviewer {
            m.reviewer_runs += 1;
        } else {
            m.runs += 1;
        }
        if run.status == RunIndexStatus::Running {
            m.runs_in_flight += 1;
        }
        m.first_run_started_at = pick(m.first_run_started_at.take(), Some(&run.started_at), false);
        if let Some(fin) = &run.finished_at {
            m.last_run_finished_at = pick(m.last_run_finished_at.take(), Some(fin), true);
            if let (Some(s), Some(e)) = (parse(&run.started_at), parse(fin))
                && e >= s
            {
                m.busy_ms += (e - s).whole_milliseconds() as u64;
            }
        }
        let Some(usage) = run.usage.as_ref() else {
            continue;
        };
        let input = usage.input_tokens.unwrap_or(0);
        let output = usage.output_tokens.unwrap_or(0);
        m.input_tokens += input;
        m.output_tokens += output;
        m.tokens += input + output;
        match usage.cost_usd {
            Some(c) => {
                m.cost_usd += c;
                if reviewer {
                    m.reviewer_cost_usd += c;
                }
            }
            None if input + output > 0 => m.cost_usd_complete = false,
            None => {}
        }
    }
    m.refresh_wall();
    for u in facts.work_units.iter().filter(|u| u.status.is_active()) {
        match u.kind {
            WorkUnitKind::Integrate | WorkUnitKind::Repair => {}
            WorkUnitKind::Task => {
                m.child_tasks_total += 1;
                if u.status == WorkUnitStatus::Done {
                    m.child_tasks_done += 1;
                }
            }
            _ => {
                m.leaves_total += 1;
                if u.status == WorkUnitStatus::Done {
                    m.leaves_done += 1;
                }
            }
        }
    }
    m
}

/// D11: 節点ごとの自分の分と subtree の合計。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SubtreeMetrics {
    pub task_id: TaskId,
    pub own: RollupMetrics,
    pub subtree: RollupMetrics,
}

/// D11: `task_core::tree_metrics::rollup`（純粋関数）。入力の順に、各節点の自分の分と subtree（自分と、
/// `parent_id` を辿って自分に至る子孫すべて）の合計を返す。`parent_id` が入力に無い節点はその集合の根として
/// 扱う。祖先の辿りは 64 段で打ち切る（壊れた親の輪で止まらないため）。
pub fn rollup(nodes: &[RollupNodeFacts]) -> Vec<SubtreeMetrics> {
    let own: Vec<RollupMetrics> = nodes.iter().map(node_metrics).collect();
    let index: BTreeMap<TaskId, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.task_id, i))
        .collect();
    let mut subtree: Vec<RollupMetrics> = vec![RollupMetrics::default(); nodes.len()];
    for (i, own_i) in own.iter().enumerate() {
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        let mut at = Some(i);
        for _ in 0..64 {
            let Some(j) = at else {
                break;
            };
            if !seen.insert(j) {
                break;
            }
            subtree[j].absorb(own_i);
            at = nodes[j].parent_id.and_then(|p| index.get(&p).copied());
        }
    }
    nodes
        .iter()
        .zip(own)
        .zip(subtree)
        .map(|((n, own), subtree)| SubtreeMetrics {
            task_id: n.task_id,
            own,
            subtree,
        })
        .collect()
}

/// U-R7: 深さ（task の層。root = 1）ごとの合計（各節点の自分の分を深さで畳む）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DepthRollup {
    pub depth: u32,
    pub metrics: RollupMetrics,
}

/// U-R7: 深さごとの合計（深さの昇順。純粋関数）。
pub fn by_depth(nodes: &[RollupNodeFacts]) -> Vec<DepthRollup> {
    let mut acc: BTreeMap<u32, RollupMetrics> = BTreeMap::new();
    for n in nodes {
        acc.entry(n.depth).or_default().absorb(&node_metrics(n));
    }
    acc.into_iter()
        .map(|(depth, metrics)| DepthRollup { depth, metrics })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_plan::{WorkUnitContext, WorkUnitSpec};
    use crate::model::{RunRole, Usage};
    use crate::quota::QuotaWindow;

    fn run(
        id: &str,
        role: RunIndexRole,
        start: &str,
        end: Option<&str>,
        tokens: (u64, u64),
        cost: Option<f64>,
    ) -> RunRow {
        RunRow {
            run_id: id.to_string(),
            task_id: "t".to_string(),
            work_unit_id: None,
            role,
            seq: 1,
            status: if end.is_some() {
                RunIndexStatus::Completed
            } else {
                RunIndexStatus::Running
            },
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: Some(Usage {
                input_tokens: Some(tokens.0),
                output_tokens: Some(tokens.1),
                cache_read_tokens: None,
                cache_creation_tokens: None,
                cost_usd: cost,
            }),
            metrics: None,
            started_at: start.to_string(),
            finished_at: end.map(str::to_string),
        }
    }

    fn unit(key: &str, kind: WorkUnitKind, status: WorkUnitStatus) -> WorkUnitRow {
        WorkUnitRow::new(
            format!("id-{key}"),
            "t".to_string(),
            "p".to_string(),
            0,
            WorkUnitSpec {
                key: key.to_string(),
                kind,
                title: key.to_string(),
                objective: "o".to_string(),
                depends_on: vec![],
                done_when: vec![],
                checks: vec![],
                context: WorkUnitContext::default(),
                harness: None,
                features: None,
                budget: None,
                outputs: vec![],
                phase: None,
            },
            status,
            "2026-09-29T00:00:00Z".to_string(),
        )
    }

    fn quota(runs: u32, pct: f64, role: RunRole) -> QuotaUse {
        QuotaUse {
            source: "claude-oauth".to_string(),
            account: Some("acct".to_string()),
            window: QuotaWindow::FiveHour,
            used_pct: Some(pct),
            runs,
            method_counts: [("measured".to_string(), runs)].into_iter().collect(),
            runs_by_role: [(role.as_str().to_string(), runs)].into_iter().collect(),
        }
    }

    /// ADR-0079 §7 R4a (a) `rollup_matches_hand_computed_fixture`: root → child → grandchild の 3 段で、各節点の
    /// subtree の run・トークン・定価（`cost_usd_complete` の伝播）・quota・壁時計・leaf の done / total・
    /// 未回答の決定が手計算と一致する。
    #[test]
    fn rollup_matches_hand_computed_fixture() {
        let root = TaskId::new();
        let child = TaskId::new();
        let grandchild = TaskId::new();
        let nodes = vec![
            RollupNodeFacts {
                task_id: root,
                parent_id: None,
                depth: 1,
                // planner 1 本（10:00〜10:01）、reviewer 1 本（12:00〜12:05、$0.50）。
                runs: vec![
                    run(
                        "r-plan",
                        RunIndexRole::Planner,
                        "2026-09-29T10:00:00Z",
                        Some("2026-09-29T10:01:00Z"),
                        (100, 10),
                        Some(0.10),
                    ),
                    run(
                        "r-rev",
                        RunIndexRole::Reviewer,
                        "2026-09-29T12:00:00Z",
                        Some("2026-09-29T12:05:00Z"),
                        (50, 5),
                        Some(0.50),
                    ),
                ],
                work_units: vec![
                    unit("a", WorkUnitKind::Implement, WorkUnitStatus::Done),
                    unit("c", WorkUnitKind::Task, WorkUnitStatus::Running),
                    unit(
                        "integrate-s1",
                        WorkUnitKind::Integrate,
                        WorkUnitStatus::Pending,
                    ),
                    unit("old", WorkUnitKind::Implement, WorkUnitStatus::Superseded),
                ],
                quota: vec![quota(1, 2.0, RunRole::Planner)],
                open_decisions: 1,
            },
            RollupNodeFacts {
                task_id: child,
                parent_id: Some(root),
                depth: 2,
                // worker 1 本（10:10〜10:20、$1.00）、単価不明の worker 1 本（10:30〜10:40）。
                runs: vec![
                    run(
                        "c-w1",
                        RunIndexRole::Worker,
                        "2026-09-29T10:10:00Z",
                        Some("2026-09-29T10:20:00Z"),
                        (1000, 100),
                        Some(1.00),
                    ),
                    run(
                        "c-w2",
                        RunIndexRole::Worker,
                        "2026-09-29T10:30:00Z",
                        Some("2026-09-29T10:40:00Z"),
                        (200, 20),
                        None,
                    ),
                ],
                work_units: vec![
                    unit("x", WorkUnitKind::Implement, WorkUnitStatus::Done),
                    unit("y", WorkUnitKind::Implement, WorkUnitStatus::Ready),
                    unit("g", WorkUnitKind::Task, WorkUnitStatus::Done),
                ],
                quota: vec![quota(2, 3.5, RunRole::Worker)],
                open_decisions: 2,
            },
            RollupNodeFacts {
                task_id: grandchild,
                parent_id: Some(child),
                depth: 3,
                // worker 1 本（09:50〜11:00、$0.25）と reviewer 1 本（11:00〜、まだ走っている、$0.05）。
                runs: vec![
                    run(
                        "g-w",
                        RunIndexRole::Worker,
                        "2026-09-29T09:50:00Z",
                        Some("2026-09-29T11:00:00Z"),
                        (300, 30),
                        Some(0.25),
                    ),
                    run(
                        "g-rev",
                        RunIndexRole::Reviewer,
                        "2026-09-29T11:00:00Z",
                        None,
                        (0, 0),
                        Some(0.05),
                    ),
                ],
                work_units: vec![],
                quota: vec![quota(1, 1.0, RunRole::Worker)],
                open_decisions: 0,
            },
        ];
        let out = rollup(&nodes);
        assert_eq!(out.len(), 3);
        let (r, c, g) = (&out[0], &out[1], &out[2]);

        // 孫: 自分の分 = subtree。
        assert_eq!(g.own, g.subtree);
        assert_eq!(g.subtree.tasks, 1);
        assert_eq!(g.subtree.runs, 1);
        assert_eq!(g.subtree.reviewer_runs, 1);
        assert_eq!(g.subtree.runs_in_flight, 1);
        assert_eq!(g.subtree.tokens, 330);
        assert!((g.subtree.cost_usd - 0.30).abs() < 1e-9);
        assert!((g.subtree.reviewer_cost_usd - 0.05).abs() < 1e-9);
        assert!(g.subtree.cost_usd_complete);
        assert_eq!(g.subtree.wall_ms, Some(70 * 60 * 1000));
        assert_eq!(g.subtree.busy_ms, 70 * 60 * 1000);

        // 子: 自分（worker 2、1,320 トークン、$1.00 だが不完全、leaf 1/2、子 task 1/1、決定 2）+ 孫。
        assert_eq!(c.own.runs, 2);
        assert!(
            !c.own.cost_usd_complete,
            "an unpriced run makes the cost a lower bound"
        );
        assert_eq!((c.own.leaves_done, c.own.leaves_total), (1, 2));
        assert_eq!((c.own.child_tasks_done, c.own.child_tasks_total), (1, 1));
        assert_eq!(c.subtree.tasks, 2);
        assert_eq!(c.subtree.runs, 3);
        assert_eq!(c.subtree.reviewer_runs, 1);
        assert_eq!(c.subtree.runs_by_role.get("worker"), Some(&3));
        assert_eq!(c.subtree.runs_by_role.get("reviewer"), Some(&1));
        assert_eq!(c.subtree.tokens, 1320 + 330);
        assert!((c.subtree.cost_usd - 1.30).abs() < 1e-9);
        assert!(!c.subtree.cost_usd_complete, "incompleteness propagates up");
        assert_eq!(c.subtree.open_decisions, 2);
        assert_eq!((c.subtree.leaves_done, c.subtree.leaves_total), (1, 2));
        // 壁時計: 孫の 09:50 → 孫の 11:00（子の run は 10:10〜10:40 でその内側）。
        assert_eq!(
            c.subtree.first_run_started_at.as_deref(),
            Some("2026-09-29T09:50:00Z")
        );
        assert_eq!(
            c.subtree.last_run_finished_at.as_deref(),
            Some("2026-09-29T11:00:00Z")
        );
        assert_eq!(c.subtree.wall_ms, Some(70 * 60 * 1000));
        assert_eq!(c.subtree.busy_ms, (10 + 10 + 70) * 60 * 1000);
        assert_eq!(c.subtree.quota.len(), 1);
        assert_eq!(c.subtree.quota[0].runs, 3);
        assert_eq!(c.subtree.quota[0].used_pct, Some(4.5));

        // root: 3 節点の合計。leaf は root の a（done、superseded の old は数えない）と子の x / y。
        assert_eq!(r.subtree.tasks, 3);
        assert_eq!(
            r.subtree.runs, 4,
            "planner 1 + child workers 2 + grandchild worker 1"
        );
        assert_eq!(r.subtree.reviewer_runs, 2);
        assert_eq!(r.subtree.runs_by_role.get("planner"), Some(&1));
        assert_eq!(r.subtree.runs_by_role.get("worker"), Some(&3));
        assert_eq!(r.subtree.runs_by_role.get("reviewer"), Some(&2));
        assert_eq!(r.subtree.input_tokens, 100 + 50 + 1000 + 200 + 300);
        assert_eq!(r.subtree.output_tokens, 10 + 5 + 100 + 20 + 30);
        assert_eq!(r.subtree.tokens, 1815);
        assert!((r.subtree.cost_usd - 1.90).abs() < 1e-9);
        assert!((r.subtree.reviewer_cost_usd - 0.55).abs() < 1e-9);
        assert!(!r.subtree.cost_usd_complete);
        assert_eq!((r.subtree.leaves_done, r.subtree.leaves_total), (2, 3));
        assert_eq!(
            (r.subtree.child_tasks_done, r.subtree.child_tasks_total),
            (1, 2)
        );
        assert_eq!(r.subtree.open_decisions, 3);
        assert_eq!(r.subtree.runs_in_flight, 1);
        // 壁時計: 孫の 09:50 → root の reviewer の 12:05 = 2h15m。
        assert_eq!(r.subtree.wall_ms, Some(135 * 60 * 1000));
        assert_eq!(r.subtree.busy_ms, (1 + 5 + 10 + 10 + 70) * 60 * 1000);
        assert_eq!(r.subtree.quota[0].runs, 4);
        assert_eq!(r.subtree.quota[0].used_pct, Some(6.5));
        assert_eq!(
            r.subtree.quota[0].runs_by_role.get("worker"),
            Some(&3),
            "{:?}",
            r.subtree.quota
        );
        // root の合計 = subtree の和。
        assert_eq!(
            r.subtree,
            RollupMetrics::sum([&r.own, &c.own, &g.own]),
            "the root's totals equal the sum over the subtree"
        );

        // 深さ別（U-R7）: 深さ 1 の reviewer 1 本 $0.50、深さ 3 の reviewer 1 本 $0.05。
        let depths = by_depth(&nodes);
        assert_eq!(
            depths.iter().map(|d| d.depth).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(depths[0].metrics.reviewer_runs, 1);
        assert!((depths[0].metrics.reviewer_cost_usd - 0.50).abs() < 1e-9);
        assert_eq!(depths[1].metrics.reviewer_runs, 0);
        assert_eq!(depths[2].metrics.reviewer_runs, 1);
        assert_eq!(depths[1].metrics, c.own);
    }

    #[test]
    fn a_node_without_runs_has_no_wall_clock_and_a_complete_cost() {
        let m = node_metrics(&RollupNodeFacts {
            task_id: TaskId::new(),
            parent_id: None,
            depth: 1,
            runs: vec![],
            work_units: vec![],
            quota: vec![],
            open_decisions: 0,
        });
        assert_eq!(m.tasks, 1);
        assert_eq!(m.wall_ms, None);
        assert!(m.cost_usd_complete);
        assert_eq!(RollupMetrics::sum([]), RollupMetrics::default());
    }
}
