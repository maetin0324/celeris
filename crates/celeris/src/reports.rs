//! 報告の圧縮（ADR-0033 D3。Phase 25。監査による修正は ADR-0034 D2/D7）。
//!
//! 親ノードにたまった子の報告を、**レビュー用の run 1 回**で 1 件にまとめる。判断（何件たまったか、
//! 最古が何時間経ったか）は決定的で、まとめる中身だけが LLM の仕事（`Check::Reviewer` と同じ「別 run」の形）。
//!
//! ここは `tick_loop` の中から同期で呼ばれる（B1: チャネルに送らない）。やることは
//! 「`reports` を読む」「まとめのタスクを `tasks` に 1 件作る」だけで、LLM もワーカーも起動しない。
//! 起こした run が `done` になると、ディスパッチャ（`task-dispatch` の `reports` モジュール）が
//! その `summary` を親ノードの報告にし、`sources` に子の報告を入れる。
//!
//! 監査 H-2: 同じノード・同じ案件のまとめタスクが直近失敗していれば、`compress_after_secs` が
//! 経つまで次のまとめを作らない(バックオフ)。子の報告は `sources` に入るまで「レビュー待ち」のままなので、
//! まとめが失敗し続けても tick ごとに新しいタスクが積み上がることはない。

use serde::Deserialize;
use task_core::report::{self, Report};
use task_core::{
    Budget, GenreSpec, ListFilter, ListOrder, OrgNode, ProjectId, RoleSpec, Status, StoreError,
    TaskId, TaskKind, TaskStore,
};
use task_ops::add::NewTaskSpec;
use time::OffsetDateTime;

/// まとめの run の予算（短い読み書きだけなので小さく取る）。
const COMPACTION_BUDGET: Budget = Budget {
    max_turns: 8,
    max_wall_secs: 600,
    max_retries: 1,
};

/// 開いているまとめタスクを探すときに見る件数の上限。
const OPEN_TASK_SCAN: usize = 500;

/// `[reports]`（ADR-0033 D3）。閾値だけを持つ。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReportsConfig {
    /// 子の報告がこの件数たまったら、まとめの run を 1 回起こす。
    #[serde(default = "default_compress_after")]
    pub compress_after: usize,
    /// 件数に満たなくても、最古の報告がこの秒数を過ぎたら起こす（既定 2 時間）。
    #[serde(default = "default_compress_after_secs")]
    pub compress_after_secs: u64,
}

fn default_compress_after() -> usize {
    report::DEFAULT_COMPRESS_AFTER
}

fn default_compress_after_secs() -> u64 {
    report::DEFAULT_COMPRESS_AFTER_SECS
}

impl Default for ReportsConfig {
    fn default() -> Self {
        Self {
            compress_after: default_compress_after(),
            compress_after_secs: default_compress_after_secs(),
        }
    }
}

/// 親になっているノード（子を 1 つ以上持つノード）。
fn parents(org: &[OrgNode]) -> Vec<&OrgNode> {
    org.iter()
        .filter(|n| {
            org.iter()
                .any(|c| c.parent_id.as_deref() == Some(n.id.as_str()))
        })
        .collect()
}

/// 案件ごとに分ける（`None` = 案件なし）。並びは入力（古い順）のまま。
fn group_by_project(pending: Vec<Report>) -> Vec<(Option<ProjectId>, Vec<Report>)> {
    let mut groups: Vec<(Option<ProjectId>, Vec<Report>)> = Vec::new();
    for report in pending {
        match groups.iter_mut().find(|(p, _)| *p == report.project_id) {
            Some((_, items)) => items.push(report),
            None => groups.push((report.project_id, vec![report])),
        }
    }
    groups
}

/// そのノード・その案件のまとめタスクが既に開いている（終端でない）か。
/// 同じ報告を二重にまとめないための決定的な検査。
fn has_open_compaction_task(
    store: &dyn TaskStore,
    node_id: &str,
    project_id: Option<ProjectId>,
) -> Result<bool, StoreError> {
    let filter = ListFilter {
        statuses: vec![
            Status::Draft,
            Status::Ready,
            Status::Running,
            Status::Blocked,
            Status::Reviewing,
        ],
        project_id,
        ..ListFilter::default()
    };
    let page = store.list_page(&filter, ListOrder::UpdatedDesc, None, OPEN_TASK_SCAN)?;
    Ok(page.items.iter().any(|t| {
        t.role.as_deref() == Some(report::COMPACTION_ROLE)
            && t.assignee.as_deref() == Some(node_id)
            && t.project_id == project_id
    }))
}

/// 監査 H-2: 同じノード・同じ案件のまとめタスクが直近 `within_secs` 以内に `Failed` / `Cancelled` で
/// 終わっていれば、次のまとめを作らない(バックオフ)。まとめの run が失敗し続けても、子の報告がたまる
/// たびに無限にタスクが作られるのを防ぐ。
fn has_recently_failed_compaction_task(
    store: &dyn TaskStore,
    node_id: &str,
    project_id: Option<ProjectId>,
    now: OffsetDateTime,
    within_secs: u64,
) -> Result<bool, StoreError> {
    let filter = ListFilter {
        statuses: vec![Status::Failed, Status::Cancelled],
        project_id,
        ..ListFilter::default()
    };
    let page = store.list_page(&filter, ListOrder::UpdatedDesc, None, OPEN_TASK_SCAN)?;
    let within = i64::try_from(within_secs).unwrap_or(i64::MAX);
    Ok(page.items.iter().any(|t| {
        t.role.as_deref() == Some(report::COMPACTION_ROLE)
            && t.assignee.as_deref() == Some(node_id)
            && t.project_id == project_id
            && (now - t.updated_at).whole_seconds() < within
    }))
}

/// まとめの run のタスクの指定（`kind = execute`、担当は親ノード）。
/// 受け入れ条件は置かない: 出力は「1 件の報告」そのもので、決定的に確かめられるものが無いため
/// （レビューは条件ゼロで全 pass = `done`。報告自体は run の終端の時点で作られる）。
///
/// 監査 L-4（Phase 27）: tier / アダプタ / 分野は自分で決めず、`task_ops::add::create_support_task` の
/// 解決順（タスクの値 > 役割の既定 > `assignee` 由来 > 分野の既定 > 全体の既定）に任せる。
/// `role = report-compressor` が `[[roles]]` に無い構成でも落ちない（既定が埋まらないだけ）。
fn compaction_spec(
    node: &OrgNode,
    project_id: Option<ProjectId>,
    pending: &[Report],
) -> NewTaskSpec {
    NewTaskSpec {
        repos: Vec::new(),
        title: format!("報告のまとめ: {}", node.name),
        objective: report::compaction_objective(node, pending),
        acceptance: Vec::new(),
        kind: TaskKind::Execute,
        tier: None,
        priority: Some(task_ops::add::PriorityInput::Number(0)),
        parent: None,
        depends_on: Vec::new(),
        max_turns: Some(COMPACTION_BUDGET.max_turns),
        max_wall_secs: Some(COMPACTION_BUDGET.max_wall_secs),
        max_retries: COMPACTION_BUDGET.max_retries,
        role: Some(report::COMPACTION_ROLE.to_string()),
        genre: None,
        aggregate: false,
        // ADR-0046 D2 / D4（Phase 59）: 裏方のまとめ run に能力タグは要らない（担当は親ノード固定）。
        skills: Vec::new(),
        mode: None,
        project_id,
        milestone_id: None,
        assignee: Some(node.id.clone()),
        // 作業ディレクトリは `workspace_root/<task_id>`（相対パスをディスパッチャが解決する）。
        workspace: None,
        cluster: None,
        // ADR-0006 Phase 115 D3: diff を作らない裏方タスクなので worktree を作らない
        // （`workspace_mode: Shared` で `resolve_repos` の primary 継承もしない。
        // `work_dir = workspace` のまま走る。本番障害: 01M3915FARENW8M0JM11XVF6W0）。
        workspace_mode: Some(task_core::WorkspaceMode::Shared),
        adapter: None,
        // ADR-0044 D3: 裏方（報告のまとめ）にラベル・種類は付けない。
        labels: Vec::new(),
        category: None,
        status: None,
        features: None,
        execution: None,
        pause_after: None,
        stages_hint: Vec::new(),
        provenance: task_ops::add::SpecProvenance::system(),
    }
}

/// tick ごとに 1 回呼ぶ。閾値を超えた親ノード × 案件ごとに、まとめのタスクを 1 件ずつ作る。
/// 作ったタスクの id を返す（テストとログのため）。**LLM は呼ばない**。
pub fn schedule_report_compaction(
    store: &dyn TaskStore,
    config: &ReportsConfig,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<Vec<TaskId>, StoreError> {
    let org = store.org_list()?;
    let mut created = Vec::new();
    for node in parents(&org) {
        let mut pending = Vec::new();
        for report in store.report_unreviewed_children(&node.id)? {
            // 配送対象の実装詳細は部署内で扱う。CoSへはdeliveryの人向け引き渡しだけを送る。
            if let Some(id) = report.task_id
                && store.delivery_get(id)?.is_some()
            {
                continue;
            }
            // ADR-0131 付記 D10 (6): 日次整理の報告は 1 日 1 件で完結させ、報告のまとめに入れない。
            if let Some(id) = report.task_id
                && store
                    .get(id)?
                    .is_some_and(|t| task_ops::knowledge_curation::is_curation_task(&t))
            {
                continue;
            }
            pending.push(report);
        }
        if pending.is_empty() {
            continue;
        }
        for (project_id, items) in group_by_project(pending) {
            if !report::compaction_due(
                &items,
                now,
                config.compress_after,
                config.compress_after_secs,
            ) {
                continue;
            }
            if has_open_compaction_task(store, &node.id, project_id)? {
                continue;
            }
            // 監査 H-2: 直近で失敗したまとめタスクがあれば、`compress_after_secs` 経つまでバックオフする。
            if has_recently_failed_compaction_task(
                store,
                &node.id,
                project_id,
                now,
                config.compress_after_secs,
            )? {
                tracing::debug!(node = %node.id, ?project_id, "reports: backing off after a recently failed compaction run");
                continue;
            }
            let spec = compaction_spec(node, project_id, &items);
            // 作成経路の検証（案件が消えている等）で落ちたら、その組だけ諦めて次へ進む
            // （1 つの案件の不整合で他のノードのまとめまで止めない）。
            let task = match task_ops::add::create_support_task(store, spec, roles, genres, now) {
                Ok(task) => task,
                Err(e) => {
                    tracing::warn!(node = %node.id, ?project_id, error = %e, "reports: could not create the compaction task");
                    continue;
                }
            };
            tracing::info!(
                node = %node.id,
                task_id = %task.id,
                reports = items.len(),
                "reports: scheduled a compaction run"
            );
            created.push(task.id);
        }
    }
    Ok(created)
}

#[cfg(test)]
#[path = "reports/tests.rs"]
mod tests;
