//! 知識ベースと受信箱の日次整理 job の入力準備と終端処理（ADR-0131 付記 D10 (2)(3)(5)(6)）。
//!
//! **LLM はここに一切無い**。`knowledge_maint.rs` と同じ形で tick から同期で呼ばれ、cron が作った
//! 日次整理 task（`harness = knowledge-curation`）を見て、決定的に次の 2 つを行う。
//!
//! - **prepare**: run を始める前に、worker の作業場所へ `inputs/kb/`（本番 KB の写し）・`inputs/inbox.json`
//!   （`task_ops::inbox` の attention と suppressed、判断候補）・`inputs/reports.json`（前回の日次整理以降に
//!   終端になった task の報告の抜粋）・`inputs/manifest.json`（発火時に固定した mode と job/run id）を書く。
//! - **finish**: task が `done` になったら `artifacts/curation-plan.json` を
//!   [`task_ops::knowledge_curation::validate_with_inbox`] で検証し、`daily-summary.md` を task の報告 1 件にする。
//!   - `dry_run`: 本番 KB は書かない。`curation.diff` が無ければ [`task_ops::knowledge_curation::dry_run`] で作る。
//!   - `apply`: 計画と差分のハッシュを報告に添え、人の決定（`curation-apply`）で同じハッシュの選択肢
//!     （`approve-<hash>`）が選ばれた後にだけ、再検証して [`task_ops::knowledge_curation::apply`] を呼ぶ。
//!     未承認・ハッシュ不一致・元ページの変更時は適用しない。
//!
//! 受信箱の状態（cancel 等）は変えない。報告のまとめ・追加の通知も作らない（報告は 1 件だけ）。
//! 進み具合は daemon の状態ファイル（`<db>.knowledge-curation.json`）に task ごとに残す（worker が書ける
//! 作業場所には置かない）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use task_core::report::{self, Report, ReportKind};
use task_core::{
    CronJob, CronJobRun, CronRunOutcome, CronTz, DecisionStatus, Event, Status, StoreError, Task,
    TaskId, TaskStore, WorkspaceSpec,
};
use task_ops::knowledge_curation::{self as curation, Action, CurationPlan, ValidatedPlan};
use task_ops::view::ViewContext;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// 人の承認の決定の key（D10 (5)）。
pub const APPROVAL_KEY: &str = "curation-apply";
/// 承認の選択肢の接頭辞（`approve-<hash の先頭 12 桁>`）。
pub const APPROVE_PREFIX: &str = "approve-";
/// 却下の選択肢。
pub const REJECT_OPTION: &str = "reject";
/// 見る cron 履歴の件数（job ごと）。
const RUN_SCAN_LIMIT: usize = 30;
/// 状態ファイルに残す task の件数の上限（古い順に捨てる）。
const STATE_KEEP: usize = 200;
/// `reports.json` に入れる task の上限と、報告本文の抜粋の上限（字数）。
const REPORTS_MAX: usize = 200;
const REPORT_EXCERPT_CHARS: usize = 600;
/// この日数より古い未読の報告を「古い報告」の判断候補にする。
const OLD_REPORT_DAYS: i64 = 7;
/// 報告本文に写す worker の要約の上限（字数）。
const SUMMARY_MAX_CHARS: usize = 4000;

/// task ごとの進み具合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// 入力を書いた（run の終端待ち）。
    Prepared,
    /// 報告を作った（dry_run・検証失敗・失敗終了。これ以上は何もしない）。
    Reported,
    /// apply の報告を作り、人の承認を待っている。
    AwaitingApproval,
    /// 承認された計画を本番 KB に反映した。
    Applied,
    /// 却下された。
    Rejected,
    /// 承認時に計画・差分・元ページが報告時と違った（適用せず、再度 dry-run を要する）。
    Stale,
}

/// 状態ファイルの 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub phase: Phase,
    pub mode: String,
    pub job_id: String,
    pub run_id: String,
    /// `inputs/inbox.json` に載せた task id（計画の `inbox` 提案の宛先の検証に使う）。
    #[serde(default)]
    pub inbox_task_ids: BTreeSet<String>,
    /// D7 の規則別の抑止件数（要約の固定節）。
    #[serde(default)]
    pub suppressed: BTreeMap<String, u32>,
    /// 承認に使うハッシュ（`sha256(plan) + sha256(diff)` の sha256 の先頭 12 桁）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub tasks: BTreeMap<String, Entry>,
}

/// 1 回の [`tick`] でしたこと（ログと試験のため）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickOutcome {
    pub prepared: Vec<TaskId>,
    pub reported: Vec<TaskId>,
    pub applied: Vec<TaskId>,
}

/// 状態ファイルの置き場所（DB の隣。`knowledge_gc` と同じ流儀）。
pub fn state_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("knowledge-curation.json")
}

fn load_state(path: &Path) -> State {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_state(path: &Path, state: &State) -> Result<(), String> {
    let mut state = state.clone();
    if state.tasks.len() > STATE_KEEP {
        // task id（ULID）は作成順なので、古いものから捨てる。
        let drop: Vec<String> = state
            .tasks
            .keys()
            .take(state.tasks.len() - STATE_KEEP)
            .cloned()
            .collect();
        for key in drop {
            state.tasks.remove(&key);
        }
    }
    let text = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn truncate_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// worker の作業場所（`Local` の相対 path は `workspace_root` 基準）。remote は扱わない。
fn workspace_dir(task: &Task, workspace_root: &Path) -> Option<PathBuf> {
    match &task.workspace {
        WorkspaceSpec::Local { path, .. } if path.is_absolute() => Some(path.clone()),
        WorkspaceSpec::Local { path, .. } if !path.as_os_str().is_empty() => {
            Some(workspace_root.join(path))
        }
        WorkspaceSpec::Local { .. } => Some(workspace_root.join(task.id.to_string())),
        _ => None,
    }
}

/// tick ごとに 1 回呼ぶ。cron の日次整理 job が作った task を見て、入力の準備・終端処理・承認後の反映を行う。
/// 1 件の失敗で他を止めない（警告して次へ進む）。
pub fn tick(
    store: &dyn TaskStore,
    knowledge_root: &Path,
    workspace_root: &Path,
    state_file: &Path,
    view: &ViewContext,
    now: OffsetDateTime,
) -> Result<TickOutcome, StoreError> {
    let mut out = TickOutcome::default();
    let jobs: Vec<CronJob> = store
        .cron_job_list()?
        .into_iter()
        .filter(|j| j.template.harness.as_deref() == Some(curation::CURATION_HARNESS))
        .collect();
    if jobs.is_empty() {
        return Ok(out);
    }
    let mut state = load_state(state_file);
    let before = state.clone();
    for job in &jobs {
        let runs = store.cron_job_runs(job.id, Some(RUN_SCAN_LIMIT))?;
        for (i, run) in runs.iter().enumerate() {
            let Some(task_id) = run
                .task_id
                .filter(|_| run.outcome == CronRunOutcome::Created)
            else {
                continue;
            };
            let Some(task) = store.get(task_id)? else {
                continue;
            };
            if !curation::is_curation_task(&task) {
                continue;
            }
            let key = task_id.to_string();
            let entry = state.tasks.get(&key).cloned();
            match (entry.as_ref().map(|e| e.phase), task.status) {
                (None, s) if !s.is_terminal() => {
                    // 直前の `created` 履歴（新しい順の後ろ側）が前回の基準。
                    let previous = runs[i + 1..]
                        .iter()
                        .find(|r| r.outcome == CronRunOutcome::Created && r.task_id.is_some());
                    match prepare(
                        store,
                        knowledge_root,
                        workspace_root,
                        view,
                        job,
                        run,
                        previous,
                        &task,
                        now,
                    ) {
                        Ok(entry) => {
                            state.tasks.insert(key, entry);
                            out.prepared.push(task_id);
                        }
                        Err(e) => {
                            tracing::warn!(%task_id, error = %e, "knowledge curation: could not prepare the inputs")
                        }
                    }
                }
                (Some(Phase::Prepared) | None, s) if s.is_terminal() => {
                    let mut entry = entry.unwrap_or_else(|| Entry {
                        phase: Phase::Prepared,
                        mode: task_ops::cron_jobs::task_mode(&task).to_string(),
                        job_id: job.id.to_string(),
                        run_id: run.id.to_string(),
                        inbox_task_ids: BTreeSet::new(),
                        suppressed: BTreeMap::new(),
                        approval_hash: None,
                        detail: None,
                        updated_at: rfc3339(now),
                    });
                    match finish(
                        store,
                        knowledge_root,
                        workspace_root,
                        job,
                        &task,
                        &mut entry,
                        now,
                    ) {
                        Ok(()) => {
                            state.tasks.insert(key, entry);
                            out.reported.push(task_id);
                        }
                        Err(e) => {
                            tracing::warn!(%task_id, error = %e, "knowledge curation: could not finish the run")
                        }
                    }
                }
                (Some(Phase::AwaitingApproval), _) => {
                    let Some(mut entry) = entry else { continue };
                    match check_approval(
                        store,
                        knowledge_root,
                        workspace_root,
                        job,
                        &task,
                        &mut entry,
                        now,
                    ) {
                        Ok(applied) => {
                            if applied {
                                out.applied.push(task_id);
                            }
                            state.tasks.insert(key, entry);
                        }
                        Err(e) => {
                            tracing::warn!(%task_id, error = %e, "knowledge curation: could not check the approval")
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if state != before
        && let Err(e) = save_state(state_file, &state)
    {
        tracing::warn!(error = %e, "knowledge curation: could not save the state file");
    }
    Ok(out)
}

// ---- prepare ----

/// `inputs/inbox.json` の判断候補 1 件（古い報告・規則に当たらない failed）。
#[derive(Debug, Clone, Serialize)]
struct Candidate {
    task_id: String,
    kind: &'static str,
    title: String,
    at: String,
    reason: String,
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    store: &dyn TaskStore,
    knowledge_root: &Path,
    workspace_root: &Path,
    view: &ViewContext,
    job: &CronJob,
    run: &CronJobRun,
    previous: Option<&CronJobRun>,
    task: &Task,
    now: OffsetDateTime,
) -> Result<Entry, String> {
    let dir = workspace_dir(task, workspace_root)
        .ok_or_else(|| "日次整理は local の作業場所だけを扱う".to_string())?;
    let inputs = dir.join("inputs");
    std::fs::create_dir_all(&inputs).map_err(|e| format!("{}: {e}", inputs.display()))?;
    let mode = task_ops::cron_jobs::task_mode(task);

    // 1. KB の写し（symlink と .git は写さない）。前回の写しは消してから作り直す。
    let kb_copy = inputs.join("kb");
    if kb_copy.exists() {
        std::fs::remove_dir_all(&kb_copy).map_err(|e| format!("{}: {e}", kb_copy.display()))?;
    }
    if knowledge_root.is_dir() {
        copy_tree(knowledge_root, &kb_copy)?;
    } else {
        std::fs::create_dir_all(&kb_copy).map_err(|e| e.to_string())?;
    }

    // 2. 受信箱（attention と suppressed、判断候補）。
    let inbox = task_ops::inbox::inbox(store, None, view, now, &|_, _| Vec::new())
        .map_err(|e| e.to_string())?;
    let mut candidates = Vec::new();
    for item in &inbox.attention {
        if let task_ops::inbox::AttentionItem::Failed {
            task: t,
            reason,
            at,
            ..
        } = item
        {
            candidates.push(Candidate {
                task_id: t.id.to_string(),
                kind: "failed",
                title: t.title.clone(),
                at: at.clone(),
                reason: format!("R1〜R4 に当たらない failed: {reason}"),
            });
        }
    }
    let old_before = now - time::Duration::days(OLD_REPORT_DAYS);
    let unread = store
        .report_list(&task_core::ReportFilter {
            unread_only: true,
            limit: 500,
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    let mut seen_old = BTreeSet::new();
    for r in unread.iter().filter(|r| r.created_at < old_before) {
        let Some(id) = r.task_id else { continue };
        if !seen_old.insert(id) {
            continue;
        }
        candidates.push(Candidate {
            task_id: id.to_string(),
            kind: "old_report",
            title: r.headline.clone(),
            at: rfc3339(r.created_at),
            reason: format!("{OLD_REPORT_DAYS} 日より古い未読の報告"),
        });
    }
    let mut inbox_task_ids: BTreeSet<String> =
        candidates.iter().map(|c| c.task_id.clone()).collect();
    let attention_json = serde_json::to_value(&inbox.attention).map_err(|e| e.to_string())?;
    if let Some(items) = attention_json.as_array() {
        for item in items {
            if let Some(id) = item.pointer("/task/id").and_then(|v| v.as_str()) {
                inbox_task_ids.insert(id.to_string());
            }
        }
    }
    let inbox_json = serde_json::json!({
        "generated_at": rfc3339(now),
        "attention": attention_json,
        "suppressed": inbox.suppressed,
        "candidates": candidates,
    });
    write_json(&inputs.join("inbox.json"), &inbox_json)?;

    // 3. 前回の日次整理以降に終端になった task の報告の抜粋。
    let since = match previous {
        Some(p) => p.recorded_at,
        None => job.created_at,
    };
    let reports = reports_since(store, since, now).map_err(|e| e.to_string())?;
    write_json(
        &inputs.join("reports.json"),
        &serde_json::json!({
            "since": rfc3339(since),
            "until": rfc3339(now),
            "reports": reports,
        }),
    )?;

    // 4. 発火時に固定した mode と job/run id（D10 (3)）。
    write_json(
        &inputs.join("manifest.json"),
        &serde_json::json!({
            "mode": mode,
            "job_id": job.id.to_string(),
            "job_name": job.name,
            "run_id": run.id.to_string(),
            "scheduled_for": rfc3339(run.scheduled_for),
            "prepared_at": rfc3339(now),
        }),
    )?;
    Ok(Entry {
        phase: Phase::Prepared,
        mode: mode.to_string(),
        job_id: job.id.to_string(),
        run_id: run.id.to_string(),
        inbox_task_ids,
        suppressed: inbox.suppressed,
        approval_hash: None,
        detail: None,
        updated_at: rfc3339(now),
    })
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        let from = entry.path();
        let to = dst.join(&name);
        if ft.is_dir() {
            copy_tree(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// 支援 task（報告のまとめ・task ごとの知識整理）と日次整理 task 自身は除く。
fn is_support(task: &Task) -> bool {
    curation::is_curation_task(task)
        || matches!(
            task.role.as_deref(),
            Some(report::COMPACTION_ROLE) | Some(report::KNOWLEDGE_ROLE)
        )
}

fn reports_since(
    store: &dyn TaskStore,
    since: OffsetDateTime,
    until: OffsetDateTime,
) -> Result<Vec<serde_json::Value>, StoreError> {
    let mut tasks: Vec<Task> = Vec::new();
    for status in [Status::Done, Status::Failed, Status::Cancelled] {
        tasks.extend(
            store
                .list(Some(status))?
                .into_iter()
                .filter(|t| t.updated_at > since && t.updated_at <= until && !is_support(t)),
        );
    }
    // 終端時刻・task id の順に重複なく並べる。
    tasks.sort_by(|a, b| a.updated_at.cmp(&b.updated_at).then(a.id.cmp(&b.id)));
    tasks.dedup_by_key(|t| t.id);
    tasks.truncate(REPORTS_MAX);
    let mut out = Vec::with_capacity(tasks.len());
    for t in tasks {
        let report = store
            .report_list(&task_core::ReportFilter {
                task_id: Some(t.id),
                limit: 1,
                ..Default::default()
            })?
            .into_iter()
            .next();
        out.push(serde_json::json!({
            "task_id": t.id.to_string(),
            "title": t.title,
            "status": t.status,
            "finished_at": rfc3339(t.updated_at),
            "headline": report.as_ref().map(|r| r.headline.clone()),
            "excerpt": report.as_ref().map(|r| truncate_chars(&r.body, REPORT_EXCERPT_CHARS)),
        }));
    }
    Ok(out)
}

// ---- finish ----

/// 検証の結果（報告の固定節に使う）。
struct Checked {
    plan: CurationPlan,
    validated: ValidatedPlan,
    diff: String,
    hash: String,
}

fn artifacts_dir(task: &Task, workspace_root: &Path) -> Option<PathBuf> {
    let dir = workspace_dir(task, workspace_root)?;
    Some(task_core::artifacts::artifacts_dir_for(task, &dir))
}

/// 計画を読み、本番 KB に対して検証し、差分（daemon が作る決定的な diff）とハッシュを求める。
fn check_plan(knowledge_root: &Path, artifacts: &Path, entry: &Entry) -> Result<Checked, String> {
    let raw = std::fs::read_to_string(artifacts.join("curation-plan.json"))
        .map_err(|e| format!("curation-plan.json を読めない: {e}"))?;
    let plan: CurationPlan =
        serde_json::from_str(&raw).map_err(|e| format!("curation-plan.json の形が違う: {e}"))?;
    let known = (!entry.inbox_task_ids.is_empty()).then_some(&entry.inbox_task_ids);
    let validated = if known.is_some() {
        curation::validate_with_inbox(knowledge_root, &plan, known)?
    } else if plan.inbox.is_empty() {
        curation::validate(knowledge_root, &plan)?
    } else {
        return Err("入力に無い task への受信箱の提案がある".into());
    };
    let diff = curation::dry_run(knowledge_root, &plan)?;
    let hash = approval_hash(&raw, &diff);
    Ok(Checked {
        plan,
        validated,
        diff,
        hash,
    })
}

/// 承認に使うハッシュ（計画の原文と daemon の差分の両方に結び付ける）。
pub fn approval_hash(plan_raw: &str, diff: &str) -> String {
    let joined = format!(
        "{}\n{}",
        curation::content_hash(plan_raw),
        curation::content_hash(diff)
    );
    curation::content_hash(&joined)[..12].to_string()
}

fn local_date(job: &CronJob, now: OffsetDateTime) -> String {
    CronTz::iana(&job.timezone)
        .or_else(|_| CronTz::posix(&job.timezone))
        .and_then(|tz| tz.local_date(now))
        .unwrap_or_else(|_| {
            format!(
                "{:04}-{:02}-{:02}",
                now.year(),
                u8::from(now.month()),
                now.day()
            )
        })
}

fn finish(
    store: &dyn TaskStore,
    knowledge_root: &Path,
    workspace_root: &Path,
    job: &CronJob,
    task: &Task,
    entry: &mut Entry,
    now: OffsetDateTime,
) -> Result<(), String> {
    entry.updated_at = rfc3339(now);
    if task.status != Status::Done {
        // 失敗・取り下げの報告は dispatcher の bad_news が既に 1 件ある。ここでは何も足さない。
        entry.phase = Phase::Reported;
        entry.detail = Some(format!("task が {:?} で終わった", task.status));
        return Ok(());
    }
    let artifacts = artifacts_dir(task, workspace_root)
        .ok_or_else(|| "日次整理は local の作業場所だけを扱う".to_string())?;
    let date = local_date(job, now);
    let checked = check_plan(knowledge_root, &artifacts, entry);
    let worker_summary = std::fs::read_to_string(artifacts.join("daily-summary.md")).ok();
    match &checked {
        Ok(c) => {
            if !artifacts.join("curation.diff").exists() {
                std::fs::write(artifacts.join("curation.diff"), &c.diff)
                    .map_err(|e| format!("curation.diff: {e}"))?;
            }
            if entry.mode == "apply" {
                entry.phase = Phase::AwaitingApproval;
                entry.approval_hash = Some(c.hash.clone());
            } else {
                entry.phase = Phase::Reported;
            }
        }
        Err(e) => {
            entry.phase = Phase::Reported;
            entry.detail = Some(e.clone());
        }
    }
    let summary = render_summary(&date, entry, checked.as_ref().ok(), checked.as_ref().err());
    if worker_summary.is_none() {
        std::fs::write(artifacts.join("daily-summary.md"), &summary)
            .map_err(|e| format!("daily-summary.md: {e}"))?;
    }
    append_report(store, task, entry, &summary, worker_summary.as_deref(), now)
        .map_err(|e| e.to_string())?;
    if entry.phase == Phase::AwaitingApproval
        && let Ok(c) = &checked
    {
        request_approval(store, task, &c.hash).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// D8 の固定節: KB の件数（統合・新規・削除・修正・保留）、削除するページ、受信箱（抑止件数と提案）、
/// 人が判断すべき残り。承認待ちの apply は実施済みと書かず提案数として示す。
fn render_summary(
    date: &str,
    entry: &Entry,
    checked: Option<&Checked>,
    error: Option<&String>,
) -> String {
    let mut out = format!("# 日次整理 {date}（{}）\n\n", entry.mode);
    let Some(c) = checked else {
        out.push_str("## 検証\n\n");
        out.push_str(&format!(
            "- 計画を検証できなかったので、本番 KB は変えていない: {}\n",
            error.map(String::as_str).unwrap_or("理由不明")
        ));
        push_inbox(&mut out, entry, None);
        out.push_str("\n## 人が判断すべき残り\n\n- 計画の作り直し（次回の日次整理）\n");
        return out;
    };
    let count = |a: Action| c.validated.kb.iter().filter(|k| k.action == a).count();
    let verb = match (entry.mode.as_str(), entry.phase) {
        ("apply", Phase::Applied) => "反映した",
        ("apply", _) => "提案（承認待ち・未反映）",
        _ => "提案（dry-run・本番 KB は不変）",
    };
    out.push_str(&format!("## KB（{verb}）\n\n"));
    out.push_str(&format!(
        "- 統合 {}・新規 {}・削除 {}・修正 {}・保持 {}・保留（人の判断） {}\n",
        count(Action::Merge),
        count(Action::New),
        count(Action::Delete),
        count(Action::Fix),
        count(Action::Keep),
        c.validated.human_decisions.len()
    ));
    let deleted: Vec<_> = c
        .validated
        .kb
        .iter()
        .filter(|k| matches!(k.action, Action::Delete | Action::Merge))
        .collect();
    out.push_str("\n## 削除するページ\n\n");
    if deleted.is_empty() {
        out.push_str("- なし\n");
    }
    for k in deleted.iter().take(10) {
        out.push_str(&format!(
            "- `{}`: {}\n",
            k.path,
            truncate_chars(&k.reason.replace('\n', " "), 120)
        ));
    }
    if deleted.len() > 10 {
        out.push_str(&format!("- ほか {} 件\n", deleted.len() - 10));
    }
    push_inbox(&mut out, entry, Some(&c.plan));
    out.push_str("\n## 人が判断すべき残り\n\n");
    let mut remaining = c.validated.human_decisions.len() + c.plan.inbox.len();
    if entry.mode == "apply" && entry.phase == Phase::AwaitingApproval {
        remaining += 1;
        out.push_str(&format!(
            "- 本番 KB への反映の承認: 決定 `{APPROVAL_KEY}` で `{APPROVE_PREFIX}{}` を選ぶ（計画と差分のハッシュ）\n",
            c.hash
        ));
    }
    for d in c.validated.human_decisions.iter().take(10) {
        out.push_str(&format!(
            "- `{}`: {}（{}）\n",
            d.subject,
            truncate_chars(&d.proposal, 80),
            truncate_chars(&d.reason, 80)
        ));
    }
    out.push_str(&format!("- 計 {remaining} 件\n"));
    out
}

fn push_inbox(out: &mut String, entry: &Entry, plan: Option<&CurationPlan>) {
    out.push_str("\n## 受信箱\n\n");
    let suppressed: u32 = entry.suppressed.values().sum();
    let rules = entry
        .suppressed
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join("・");
    if suppressed == 0 {
        out.push_str("- 規則で外した件数: 0\n");
    } else {
        out.push_str(&format!("- 規則で外した件数: {suppressed}（{rules}）\n"));
    }
    let proposals = plan.map(|p| p.inbox.as_slice()).unwrap_or_default();
    out.push_str(&format!(
        "- 片付けの提案: {} 件（状態は変えていない）\n",
        proposals.len()
    ));
    for p in proposals.iter().take(10) {
        out.push_str(&format!(
            "  - {}: {}（{}）\n",
            p.task_id,
            truncate_chars(&p.proposal, 80),
            truncate_chars(&p.reason, 80)
        ));
    }
}

/// task の報告を 1 件だけ作る（既にこの task の報告があれば作らない）。
fn append_report(
    store: &dyn TaskStore,
    task: &Task,
    entry: &Entry,
    summary: &str,
    worker_summary: Option<&str>,
    now: OffsetDateTime,
) -> Result<Option<Report>, StoreError> {
    let Some(assignee) = task.assignee.as_deref() else {
        tracing::warn!(task_id = %task.id, "knowledge curation: the task has no assignee; no report");
        return Ok(None);
    };
    let existing = store.report_list(&task_core::ReportFilter {
        task_id: Some(task.id),
        limit: 50,
        ..Default::default()
    })?;
    if existing.iter().any(|r| r.kind != ReportKind::BadNews) {
        return Ok(None);
    }
    let org = store.org_list()?;
    if !org.iter().any(|n| n.id == assignee) {
        tracing::warn!(task_id = %task.id, assignee, "knowledge curation: assignee is not a known org node; no report");
        return Ok(None);
    }
    let level = report::level_of(&org, assignee);
    let headline = summary
        .lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches('#')
        .trim()
        .to_string();
    let mut body = format!("タスク: {}\n\n{}", task.title, summary.trim_end());
    if let Some(ws) = worker_summary.filter(|s| !s.trim().is_empty()) {
        body.push_str("\n\n## 担当の要約（daily-summary.md）\n\n");
        body.push_str(&truncate_chars(ws, SUMMARY_MAX_CHARS));
    }
    body.push('\n');
    let r = Report {
        id: report::ReportId::new(),
        project_id: task.project_id,
        node_id: assignee.to_string(),
        task_id: Some(task.id),
        kind: ReportKind::Result,
        level,
        headline: if headline.is_empty() {
            format!("日次整理（{}）", entry.mode)
        } else {
            headline
        },
        body,
        sources: Vec::new(),
        read_at: None,
        created_at: now,
    };
    store.report_append(&r)?;
    Ok(Some(r))
}

/// D10 (5): 人の承認の決定（同じハッシュの選択肢）を出す。同じ task に未回答があれば増やさない。
fn request_approval(store: &dyn TaskStore, task: &Task, hash: &str) -> Result<(), StoreError> {
    if open_or_answered(store, task)?.is_some() {
        return Ok(());
    }
    let path = task_ops::tree::decision_path(store, task).map_err(|e| match e {
        task_ops::error::OpsError::Store(e) => e,
        other => StoreError::Invalid(other.to_string()),
    })?;
    let approve = format!("{APPROVE_PREFIX}{hash}");
    let request = task_core::DecisionRequest {
        id: ulid::Ulid::new().to_string(),
        key: APPROVAL_KEY.to_string(),
        kind: task_core::DecisionKind::Choice,
        question: format!(
            "日次整理の計画（ハッシュ {hash}）を本番 KB に反映するか。差分は artifacts/curation.diff、要約は報告にある"
        ),
        options: vec![
            task_core::DecisionOption {
                key: approve.clone(),
                label: format!("計画 {hash} を本番 KB に反映する"),
                consequence: Some(
                    "反映前に同じ計画を再検証する。元ページが変わっていれば反映しない".to_string(),
                ),
            },
            task_core::DecisionOption {
                key: REJECT_OPTION.to_string(),
                label: "反映しない".to_string(),
                consequence: None,
            },
        ],
        recommended: REJECT_OPTION.to_string(),
        cost_of_reversal: task_core::CostOfReversal::Medium,
        cost_note: Some("削除したページは _curation/ の記録と KB の履歴から戻す".to_string()),
        needed_before: vec![task_core::decision::NEEDED_BEFORE_SELF.to_string()],
        path,
        raised_by: task_core::DecisionRaisedBy {
            task_id: task.id,
            run_id: None,
            origin: task_core::DecisionOrigin::Daemon,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    store.append_event(
        task.id,
        &Event::DecisionRequested {
            decision: Box::new(request),
        },
    )?;
    Ok(())
}

fn open_or_answered(
    store: &dyn TaskStore,
    task: &Task,
) -> Result<Option<task_core::DecisionRow>, StoreError> {
    let root = task_core::tree::root_id_of(task);
    Ok(store
        .decisions_list(Some(root))?
        .into_iter()
        .filter(|d| d.task_id == task.id && d.key == APPROVAL_KEY)
        .find(|d| d.status != DecisionStatus::Withdrawn))
}

/// 承認待ちの task: 決定が `approve-<同じハッシュ>` で答えられたときだけ、再検証して反映する。
fn check_approval(
    store: &dyn TaskStore,
    knowledge_root: &Path,
    workspace_root: &Path,
    job: &CronJob,
    task: &Task,
    entry: &mut Entry,
    now: OffsetDateTime,
) -> Result<bool, String> {
    let Some(row) = open_or_answered(store, task).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    if row.status != DecisionStatus::Answered {
        return Ok(false);
    }
    let option = row
        .request
        .answer
        .as_ref()
        .map(|a| a.option.clone())
        .unwrap_or_default();
    entry.updated_at = rfc3339(now);
    let Some(expected) = entry.approval_hash.clone() else {
        entry.phase = Phase::Stale;
        entry.detail = Some("承認に使うハッシュが無い".into());
        return Ok(false);
    };
    if option != format!("{APPROVE_PREFIX}{expected}") {
        entry.phase = Phase::Rejected;
        entry.detail = Some(format!("人の回答: {option}"));
        tracing::info!(task_id = %task.id, option, "knowledge curation: the plan was not approved");
        return Ok(false);
    }
    let artifacts = artifacts_dir(task, workspace_root)
        .ok_or_else(|| "日次整理は local の作業場所だけを扱う".to_string())?;
    let checked = match check_plan(knowledge_root, &artifacts, entry) {
        Ok(c) if c.hash == expected => c,
        Ok(c) => {
            entry.phase = Phase::Stale;
            entry.detail = Some(format!(
                "承認したハッシュ {expected} と今の計画・差分 {} が違うので反映しない（再度 dry-run が要る）",
                c.hash
            ));
            tracing::warn!(task_id = %task.id, "knowledge curation: the approved plan changed; not applied");
            return Ok(false);
        }
        Err(e) => {
            entry.phase = Phase::Stale;
            entry.detail = Some(format!("再検証に失敗したので反映しない: {e}"));
            tracing::warn!(task_id = %task.id, error = %e, "knowledge curation: re-validation failed; not applied");
            return Ok(false);
        }
    };
    let outcome = curation::apply(knowledge_root, &checked.plan, &local_date(job, now))?;
    entry.phase = Phase::Applied;
    entry.detail = Some(format!(
        "統合 {}・新規 {}・削除 {}・修正 {}・保持 {}・保留 {}",
        outcome.merged,
        outcome.new,
        outcome.deleted,
        outcome.fixed,
        outcome.kept,
        outcome.skipped_human
    ));
    tracing::info!(task_id = %task.id, merged = outcome.merged, new = outcome.new, deleted = outcome.deleted, fixed = outcome.fixed, "knowledge curation: the approved plan was applied to the KB");
    Ok(true)
}

#[cfg(test)]
#[path = "knowledge_curation/tests.rs"]
mod tests;
