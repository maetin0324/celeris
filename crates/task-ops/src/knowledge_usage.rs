//! ADR 2026-11-12-knowledge-usage-per-run D4: KB の使われ方の report（`celerisctl knowledge usage`）。
//!
//! 入力は events（正本）だけ。worker run は `WorkerFinished.metrics.knowledge`（入口で数えた回数。D1〜D3）、
//! curation は `KnowledgeCurationApplied` の件数。読み取り率の分母は計測した run（`knowledge` が `Some`）だけ。

use std::collections::BTreeMap;

use serde::Serialize;
use task_core::Event;
use task_core::store::TaskWithEvents;

/// agent の種類・案件ごとの 1 行。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageRow {
    /// `worker`・`reviewer`・`planner`（`WorkerFinished.role`）。
    pub agent: String,
    /// 案件の表示名（slug か id）。案件の無い task は `None`。
    pub project: Option<String>,
    /// 計測した run の数（分母）。
    pub measured_runs: u32,
    /// 検索か取得を 1 回以上した run の数（分子）。
    pub read_runs: u32,
    pub search: u64,
    pub get: u64,
    pub record: u64,
    /// `read_runs / measured_runs`。
    pub read_rate: Option<f64>,
}

/// 1 回の curation の適用（`KnowledgeCurationApplied`）か、その合計。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CurationRow {
    /// 適用日。合計の行は `None`。
    pub date: Option<String>,
    pub new: u64,
    pub merged: u64,
    pub deleted: u64,
    pub fixed: u64,
    /// 統合 ÷（新規＋統合）。分母 0 なら `None`。
    pub merge_ratio: Option<f64>,
}

impl CurationRow {
    fn add(&mut self, new: u32, merged: u32, deleted: u32, fixed: u32) {
        self.new += u64::from(new);
        self.merged += u64::from(merged);
        self.deleted += u64::from(deleted);
        self.fixed += u64::from(fixed);
        let denominator = self.new + self.merged;
        self.merge_ratio = (denominator > 0).then(|| self.merged as f64 / denominator as f64);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageReport {
    pub runs: Vec<UsageRow>,
    pub curation: Vec<CurationRow>,
    pub curation_total: CurationRow,
}

/// events から report を作る。`project_label` は案件 id を表示名に直す（呼び出し側が store から引く）。
pub fn report(
    tasks: &[TaskWithEvents],
    project_label: impl Fn(task_core::ProjectId) -> String,
) -> UsageReport {
    let mut rows: BTreeMap<(String, Option<String>), UsageRow> = BTreeMap::new();
    let mut curation = Vec::new();
    let mut curation_total = CurationRow::default();
    for (task, events) in tasks {
        let project = task.project_id.map(&project_label);
        for (_, event) in events {
            match event {
                Event::WorkerFinished { role, metrics, .. } => {
                    let Some(used) = metrics.and_then(|m| m.knowledge) else {
                        continue;
                    };
                    let agent = role.map(|r| r.as_str()).unwrap_or("worker").to_string();
                    let row = rows
                        .entry((agent.clone(), project.clone()))
                        .or_insert_with(|| UsageRow {
                            agent,
                            project: project.clone(),
                            ..UsageRow::default()
                        });
                    row.measured_runs += 1;
                    row.read_runs += u32::from(used.read_any());
                    row.search += u64::from(used.search);
                    row.get += u64::from(used.get);
                    row.record += u64::from(used.record);
                    row.read_rate = Some(f64::from(row.read_runs) / f64::from(row.measured_runs));
                }
                Event::KnowledgeCurationApplied {
                    date,
                    merged,
                    new,
                    deleted,
                    fixed,
                    ..
                } => {
                    let mut row = CurationRow {
                        date: Some(date.clone()),
                        ..CurationRow::default()
                    };
                    row.add(*new, *merged, *deleted, *fixed);
                    curation_total.add(*new, *merged, *deleted, *fixed);
                    curation.push(row);
                }
                _ => {}
            }
        }
    }
    curation.sort_by(|a, b| a.date.cmp(&b.date));
    UsageReport {
        runs: rows.into_values().collect(),
        curation,
        curation_total,
    }
}

#[cfg(test)]
#[path = "knowledge_usage_tests.rs"]
mod tests;
