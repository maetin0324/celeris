//! ADR 2026-11-12-knowledge-usage-per-run D4: KB の使われ方の report（`celerisctl knowledge usage`）。
//!
//! worker run の入力は events（正本）、CoS run は `chat_runs.knowledge_use_json`。worker run は `WorkerFinished.metrics.knowledge`（入口で数えた回数。D1〜D3）、
//! curation は `KnowledgeCurationApplied` の件数。読み取り率の分母は計測した run（`knowledge` が `Some`）だけ。

use std::collections::BTreeMap;

use serde::Serialize;
use task_core::Event;
use task_core::store::TaskWithEvents;

/// agent の種類・案件ごとの 1 行（`KnowledgeKnowledgeUsageRow`。campaign の `UsageRow` とは別の意味）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct KnowledgeUsageRow {
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

impl KnowledgeUsageRow {
    fn add(&mut self, used: &task_core::KnowledgeUse) {
        self.measured_runs += 1;
        self.read_runs += u32::from(used.read_any());
        self.search += u64::from(used.search);
        self.get += u64::from(used.get);
        self.record += u64::from(used.record);
        self.read_rate = Some(f64::from(self.read_runs) / f64::from(self.measured_runs));
    }
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
pub struct KnowledgeUsageReport {
    pub runs: Vec<KnowledgeUsageRow>,
    pub curation: Vec<CurationRow>,
    pub curation_total: CurationRow,
}

/// events から report を作る。`project_label` は案件 id を表示名に直す（呼び出し側が store から引く）。
pub fn usage_report(
    tasks: &[TaskWithEvents],
    project_label: impl Fn(task_core::ProjectId) -> String,
) -> KnowledgeUsageReport {
    let mut rows: BTreeMap<(String, Option<String>), KnowledgeUsageRow> = BTreeMap::new();
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
                        .or_insert_with(|| KnowledgeUsageRow {
                            agent,
                            project: project.clone(),
                            ..KnowledgeUsageRow::default()
                        });
                    row.add(&used);
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
    KnowledgeUsageReport {
        runs: rows.into_values().collect(),
        curation,
        curation_total,
    }
}

/// CoS run の行（agent は `cos`）。入力は `KnowledgeUseStore::chat_run_knowledge_uses`（計測した run だけ）。
pub fn cos_rows(
    uses: &[(Option<task_core::ProjectId>, task_core::KnowledgeUse)],
    project_label: impl Fn(task_core::ProjectId) -> String,
) -> Vec<KnowledgeUsageRow> {
    let mut rows: BTreeMap<Option<String>, KnowledgeUsageRow> = BTreeMap::new();
    for (project, used) in uses {
        let project = project.map(&project_label);
        let row = rows
            .entry(project.clone())
            .or_insert_with(|| KnowledgeUsageRow {
                agent: "cos".into(),
                project,
                ..KnowledgeUsageRow::default()
            });
        row.add(used);
    }
    rows.into_values().collect()
}

#[cfg(test)]
#[path = "knowledge_usage_tests.rs"]
mod tests;
