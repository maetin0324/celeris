//! ADR 2026-10-27-project-stagnation-inbox: 案件単位の停滞の決定的な判定（LLM なし）。
//!
//! - [`gather`] が store と `releases_dir` から事実（[`StagnationFacts`]）を読み、[`detect`] が閾値
//!   （[`StagnationPolicy`]）と `now` だけで `project_stalled`・`release_unpromoted`・`task_unscheduled` を決める。
//!   dispatcher が `scan_interval_secs` ごとに呼び、結果を `DaemonSnapshot.stagnation` に載せる（D3）。
//! - `human_wait_aging` は受信箱を組むときに項目の `age_secs` から決める（[`human_wait_aging`]。D1）。
//!
//! 止まり始めた時刻（`since`）は events の `ts` と release の file から毎回求める。メモリに状態を持たないので、
//! 同じ状態は何度判定しても同じ論理キー・同じ `since`（= revision）になり、受信箱の項目は増えない（D2）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{ProjectId, ProjectStatus, Status, Task, TaskId, TaskStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::OpsError;
use crate::wait::MemoryWaits;

pub const DEFAULT_PROJECT_IDLE_SECS: u64 = 6 * 3600;
pub const DEFAULT_HUMAN_WAIT_SECS: u64 = 2 * 3600;
pub const DEFAULT_RELEASE_UNPROMOTED_SECS: u64 = 3600;
pub const DEFAULT_TASK_UNSCHEDULED_SECS: u64 = 1800;
pub const DEFAULT_SCAN_INTERVAL_SECS: u64 = 60;

/// D4: `[maintenance.stagnation]` の閾値（秒）。`0` でその種類を止める。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StagnationPolicy {
    pub project_idle_secs: u64,
    pub human_wait_secs: u64,
    pub release_unpromoted_secs: u64,
    pub task_unscheduled_secs: u64,
}

impl Default for StagnationPolicy {
    fn default() -> Self {
        Self {
            project_idle_secs: DEFAULT_PROJECT_IDLE_SECS,
            human_wait_secs: DEFAULT_HUMAN_WAIT_SECS,
            release_unpromoted_secs: DEFAULT_RELEASE_UNPROMOTED_SECS,
            task_unscheduled_secs: DEFAULT_TASK_UNSCHEDULED_SECS,
        }
    }
}

/// D1: 停滞の種類（受信箱の kind と同じ名前）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StagnationKind {
    ProjectStalled,
    ReleaseUnpromoted,
    TaskUnscheduled,
}

impl StagnationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StagnationKind::ProjectStalled => "project_stalled",
            StagnationKind::ReleaseUnpromoted => "release_unpromoted",
            StagnationKind::TaskUnscheduled => "task_unscheduled",
        }
    }
}

/// D3: snapshot に載る判定の 1 件。`key` が受信箱の項目の id、`since` が `created_at`（revision）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StagnationFinding {
    pub kind: StagnationKind,
    /// 論理キー（`<kind>-<対象>`）。同じ対象の同じ種類は常に 1 件。
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    /// 対象の表示名（案件名・task の題・release の sha12）。
    pub subject: String,
    /// 状態が始まった時刻（RFC 3339）。同じ状態の間は変わらない。
    pub since: String,
    pub detail: String,
}

/// 案件の事実（`active` で開いている task がある案件だけ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFact {
    pub project_id: ProjectId,
    pub title: String,
    /// 終端でも draft でもない task の数。
    pub open_tasks: u32,
    /// 案件の task の最後の event の時刻。
    pub last_event_at: OffsetDateTime,
}

/// verify 済み（`ok = true`）で昇格されていない、最新の昇格済みより新しい release。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseFact {
    pub sha12: String,
    pub verified_at: OffsetDateTime,
}

/// `ready` の task の事実。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyTaskFact {
    pub task_id: TaskId,
    pub project_id: Option<ProjectId>,
    pub title: String,
    pub last_event_at: OffsetDateTime,
    /// run が走っている（dispatcher の in-flight にある）。
    pub in_flight: bool,
    /// 待ちの理由（`task_ops::wait::task_waits`）が 1 件以上ある。
    pub waiting: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StagnationFacts {
    pub projects: Vec<ProjectFact>,
    pub releases: Vec<ReleaseFact>,
    pub ready_tasks: Vec<ReadyTaskFact>,
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn reached(since: OffsetDateTime, threshold_secs: u64, now: OffsetDateTime) -> bool {
    threshold_secs > 0
        && (now - since).whole_seconds() >= i64::try_from(threshold_secs).unwrap_or(i64::MAX)
}

fn duration_ja(secs: i64) -> String {
    let secs = secs.max(0);
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h > 0 {
        format!("{h} 時間 {m} 分")
    } else {
        format!("{m} 分")
    }
}

/// D1: 事実と閾値と `now` だけで判定する（純関数）。結果は `key` 順、`key` ごとに 1 件。
pub fn detect(
    facts: &StagnationFacts,
    policy: &StagnationPolicy,
    now: OffsetDateTime,
) -> Vec<StagnationFinding> {
    let mut out: BTreeMap<String, StagnationFinding> = BTreeMap::new();
    for p in &facts.projects {
        if p.open_tasks == 0 || !reached(p.last_event_at, policy.project_idle_secs, now) {
            continue;
        }
        let key = format!("project_stalled-{}", p.project_id);
        out.insert(
            key.clone(),
            StagnationFinding {
                kind: StagnationKind::ProjectStalled,
                key,
                project_id: Some(p.project_id),
                task_id: None,
                subject: p.title.clone(),
                since: rfc3339(p.last_event_at),
                detail: format!(
                    "開いている task が {} 件あるが、最後の出来事から {} 何も起きていない（閾値 {} 秒）",
                    p.open_tasks,
                    duration_ja((now - p.last_event_at).whole_seconds()),
                    policy.project_idle_secs
                ),
            },
        );
    }
    let mut releases: Vec<&ReleaseFact> = facts.releases.iter().collect();
    releases.sort_by(|a, b| {
        a.verified_at
            .cmp(&b.verified_at)
            .then(a.sha12.cmp(&b.sha12))
    });
    if let (Some(oldest), Some(newest)) = (releases.first(), releases.last())
        && reached(oldest.verified_at, policy.release_unpromoted_secs, now)
    {
        let key = "release_unpromoted-selfdeploy".to_string();
        let shas: Vec<&str> = releases.iter().map(|r| r.sha12.as_str()).collect();
        out.insert(
            key.clone(),
            StagnationFinding {
                kind: StagnationKind::ReleaseUnpromoted,
                key,
                project_id: None,
                task_id: None,
                subject: newest.sha12.clone(),
                since: rfc3339(oldest.verified_at),
                detail: format!(
                    "verify 済みで昇格されていない release が {} 件（{}）。最初の verify から {}（閾値 {} 秒）",
                    shas.len(),
                    shas.join(", "),
                    duration_ja((now - oldest.verified_at).whole_seconds()),
                    policy.release_unpromoted_secs
                ),
            },
        );
    }
    for t in &facts.ready_tasks {
        if t.in_flight || t.waiting || !reached(t.last_event_at, policy.task_unscheduled_secs, now)
        {
            continue;
        }
        let key = format!("task_unscheduled-{}", t.task_id);
        out.insert(
            key.clone(),
            StagnationFinding {
                kind: StagnationKind::TaskUnscheduled,
                key,
                project_id: t.project_id,
                task_id: Some(t.task_id),
                subject: t.title.clone(),
                since: rfc3339(t.last_event_at),
                detail: format!(
                    "ready のまま run も待ちの理由も無く、最後の出来事から {}（閾値 {} 秒）",
                    duration_ja((now - t.last_event_at).whole_seconds()),
                    policy.task_unscheduled_secs
                ),
            },
        );
    }
    out.into_values().collect()
}

fn parse_ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// D3: store と `releases_dir` から事実を読む。`in_flight` は run が走っている task、`excluded` は生存確認の
/// `stalled` に載っている task（同じ止まりを 2 件にしない）、`memory` は dispatcher のメモリの待ちの理由。
/// 待ちの理由は閾値を過ぎた `ready` の task だけについて求める（読みを小さくする。判定は [`detect`] が改めて行う）。
#[allow(clippy::too_many_arguments)]
pub fn gather(
    store: &dyn TaskStore,
    releases_dir: Option<&Path>,
    policy: &StagnationPolicy,
    now: OffsetDateTime,
    in_flight: &HashSet<TaskId>,
    excluded: &HashSet<TaskId>,
    memory: &MemoryWaits,
) -> Result<StagnationFacts, OpsError> {
    let all_tasks = store.list(None)?;
    let mut facts = StagnationFacts::default();

    // 案件: active・アーカイブされていない案件の task の最後の event。
    let active: HashMap<ProjectId, String> = if policy.project_idle_secs > 0 {
        store
            .project_list()?
            .into_iter()
            .filter(|p| p.status == ProjectStatus::Active && p.archived_at.is_none())
            .map(|p| (p.id, p.title))
            .collect()
    } else {
        HashMap::new()
    };
    let mut by_project: BTreeMap<ProjectId, Vec<&Task>> = BTreeMap::new();
    for t in &all_tasks {
        if let Some(p) = t.project_id
            && active.contains_key(&p)
        {
            by_project.entry(p).or_default().push(t);
        }
    }
    by_project.retain(|_, tasks| tasks.iter().any(|t| is_open(t)));
    let ready: Vec<&Task> = if policy.task_unscheduled_secs > 0 {
        all_tasks
            .iter()
            .filter(|t| t.status == Status::Ready && !excluded.contains(&t.id))
            .collect()
    } else {
        Vec::new()
    };
    let ids: Vec<TaskId> = by_project
        .values()
        .flatten()
        .map(|t| t.id)
        .chain(ready.iter().map(|t| t.id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let latest = store.latest_event_ts(&ids)?;
    let last_at = |t: &Task| {
        latest
            .get(&t.id)
            .and_then(|s| parse_ts(s))
            .unwrap_or(t.updated_at)
    };

    for (project_id, tasks) in &by_project {
        let Some(last_event_at) = tasks.iter().map(|t| last_at(t)).max() else {
            continue;
        };
        facts.projects.push(ProjectFact {
            project_id: *project_id,
            title: active.get(project_id).cloned().unwrap_or_default(),
            open_tasks: tasks.iter().filter(|t| is_open(t)).count() as u32,
            last_event_at,
        });
    }

    let candidates: Vec<Task> = ready
        .iter()
        .filter(|t| !in_flight.contains(&t.id))
        .filter(|t| reached(last_at(t), policy.task_unscheduled_secs, now))
        .map(|t| (*t).clone())
        .collect();
    let waits = crate::wait::task_waits(store, &candidates, &all_tasks, memory)?;
    for t in &candidates {
        facts.ready_tasks.push(ReadyTaskFact {
            task_id: t.id,
            project_id: t.project_id,
            title: t.title.clone(),
            last_event_at: last_at(t),
            in_flight: false,
            waiting: waits.get(&t.id).is_some_and(|w| !w.is_empty()),
        });
    }

    if policy.release_unpromoted_secs > 0
        && let Some(dir) = releases_dir
    {
        facts.releases = read_unpromoted_releases(dir);
    }
    Ok(facts)
}

fn is_open(t: &Task) -> bool {
    !t.status.is_terminal() && t.status != Status::Draft
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// D1 `release_unpromoted` の事実: `verify.json` の `ok = true`・`promoted.json` が無い・最新の昇格済み release
/// （`promoted_at` 最大）より `built_at` が新しい release。読めない file の release は飛ばす（落ちない）。
pub fn read_unpromoted_releases(releases_dir: &Path) -> Vec<ReleaseFact> {
    struct Entry {
        sha12: String,
        built_at: Option<OffsetDateTime>,
        promoted_at: Option<OffsetDateTime>,
        verified_at: Option<OffsetDateTime>,
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(releases_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with(".partial") {
            continue;
        }
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let built_at = read_json(&dir.join("manifest.json"))
            .and_then(|v| v.get("built_at")?.as_str().and_then(parse_ts));
        let promoted_at = read_json(&dir.join("promoted.json"))
            .and_then(|v| v.get("promoted_at")?.as_str().and_then(parse_ts));
        let verify_path = dir.join("verify.json");
        let verified_at = read_json(&verify_path)
            .filter(|v| v.get("ok").and_then(serde_json::Value::as_bool) == Some(true))
            .and_then(|v| {
                v.get("at")
                    .and_then(|a| a.as_str())
                    .and_then(parse_ts)
                    .or_else(|| {
                        std::fs::metadata(&verify_path)
                            .and_then(|m| m.modified())
                            .ok()
                            .map(OffsetDateTime::from)
                    })
            });
        entries.push(Entry {
            sha12: name,
            built_at,
            promoted_at,
            verified_at,
        });
    }
    let current_built = entries
        .iter()
        .filter(|e| e.promoted_at.is_some())
        .max_by_key(|e| e.promoted_at)
        .and_then(|e| e.built_at);
    let mut out: Vec<ReleaseFact> = entries
        .into_iter()
        .filter(|e| e.promoted_at.is_none())
        .filter(|e| match (current_built, e.built_at) {
            (None, _) => true,
            (Some(cur), Some(b)) => b > cur,
            (Some(_), None) => false,
        })
        .filter_map(|e| {
            Some(ReleaseFact {
                verified_at: e.verified_at?,
                sha12: e.sha12,
            })
        })
        .collect();
    out.sort_by(|a, b| a.sha12.cmp(&b.sha12));
    out
}

/// D1 `human_wait_aging` の入力: 受信箱の人の操作待ちの項目 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanWait {
    pub item_id: String,
    pub title: String,
    pub project_id: Option<String>,
    pub created_at: String,
    pub age_secs: u64,
}

/// D1 `human_wait_aging` の 1 件（案件ごと）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanWaitAging {
    /// 論理キー（`human_wait_aging-<案件 id | none>`）。
    pub key: String,
    pub project_id: Option<String>,
    /// 最も古い待ちの `created_at`（revision）。
    pub since: String,
    /// 閾値を過ぎた待ち（古い順）。
    pub waits: Vec<HumanWait>,
}

/// D1: 閾値（`human_wait_secs`）を過ぎた人の操作待ちを案件ごとに 1 件へ束ねる（純関数。`key` 順）。
pub fn human_wait_aging(waits: &[HumanWait], threshold_secs: u64) -> Vec<HumanWaitAging> {
    if threshold_secs == 0 {
        return Vec::new();
    }
    let mut groups: BTreeMap<Option<String>, Vec<HumanWait>> = BTreeMap::new();
    for w in waits.iter().filter(|w| w.age_secs >= threshold_secs) {
        groups
            .entry(w.project_id.clone())
            .or_default()
            .push(w.clone());
    }
    groups
        .into_iter()
        .map(|(project_id, mut waits)| {
            waits.sort_by(|a, b| {
                b.age_secs
                    .cmp(&a.age_secs)
                    .then(a.created_at.cmp(&b.created_at))
                    .then(a.item_id.cmp(&b.item_id))
            });
            HumanWaitAging {
                key: format!(
                    "human_wait_aging-{}",
                    project_id.as_deref().unwrap_or("none")
                ),
                since: waits[0].created_at.clone(),
                project_id,
                waits,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
