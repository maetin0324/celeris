//! ADR 2026-10-15-root-done-cos-summary: root task の完了を CoS がまとめて Discord に知らせるための、
//! **決定的な**部品（LLM なし・I/O なし）。
//!
//! - 対象判定 `is_root_done`（D2。parent なし・木の子でない・support task でない done）はここ 1 か所にある。
//! - 「完了の事実」（D4）は DB の行（task・events・delivery・子 task）と進捗ファイルの本文だけから `collect` が作る。
//!   worker の `worker_finished.outcome`（自己申告）は参考として添えるだけで、単独の根拠にしない。
//! - `facts_block` は CoS スレッドに置く system message（CoS 入力にもなる）、`fallback_body` は CoS が使えない
//!   ときに Discord へ送る短文（D6。題名・統合 commit・成果物の有無・本番への移行節の有無・リンク）。
//!
//! 読み出し（store・`git show`）と送信・CoS run の起動は呼び出し側（dispatcher / notifier）が行う。

use std::collections::BTreeMap;

use task_core::{Delivery, DeliveryState, Event, Status, Task, TaskId};

/// D2: root 完了まとめの対象か。done で、`parent_id` が無く、木の子でなく、support task でない。
pub fn is_root_done(task: &Task) -> bool {
    task.status == Status::Done
        && task.parent_id.is_none()
        && !task_core::tree::is_tree_child(task)
        && task_core::support_kind(task).is_none()
}

/// D2: 最後の `Transitioned { to: Done }` の seq（reopen 後の再 done は新しい seq）。
pub fn done_transition_seq(events: &[(u64, Event)]) -> Option<u64> {
    events.iter().rev().find_map(|(seq, e)| match e {
        Event::Transitioned {
            to: Status::Done, ..
        } => Some(*seq),
        _ => None,
    })
}

/// D2: `RootDoneSummary` の重複排除 key（`<task_id>:<done 遷移の seq>`。`notify::transition_key` と同じ形）。
pub fn summary_key(task_id: TaskId, done_seq: u64) -> String {
    format!("{task_id}:{done_seq}")
}

/// 子 task 1 件の事実（題名と状態）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildFact {
    pub title: String,
    pub status: Status,
}

/// 段の統合 1 件（events `phase_integrated`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseFact {
    pub phase: String,
    pub head: String,
}

/// 進捗ファイルの節の有無（D4）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgressSections {
    /// 「## 本番への移行」節があり、本文が「なし」「不要」だけではない（人が本番で手を動かす必要がある）。
    pub migration_required: bool,
    /// 「## 未解決事項」節に中身がある。
    pub has_unresolved: bool,
    /// 「## 提案」節に中身がある。
    pub has_proposal: bool,
}

/// `collect` の入力。読み出しは呼び出し側が行う。
#[derive(Debug, Clone, Copy)]
pub struct RootDoneInputs<'a> {
    pub task: &'a Task,
    pub events: &'a [(u64, Event)],
    pub delivery: Option<&'a Delivery>,
    pub children: &'a [ChildFact],
    /// 統合 commit の tree から読んだ、この task の進捗ファイルの本文（無ければ `None`）。
    pub progress_markdown: Option<&'a str>,
}

/// D4: 決定的に集めた「完了の事実」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootDoneFacts {
    pub task_id: TaskId,
    pub title: String,
    pub objective: String,
    pub done_seq: Option<u64>,
    pub children: Vec<ChildFact>,
    pub phases: Vec<PhaseFact>,
    /// main への統合 commit（最後の `integration_merged.head_sha`、無ければ delivery が ready の `head`）。
    pub integration_commit: Option<String>,
    pub release: Option<String>,
    /// delivery の状態（`None` は delivery なし）。
    pub delivery_state: Option<DeliveryState>,
    /// 取り込まれなかった理由（最後の `delivery_skipped` の code と詳細）。
    pub delivery_skipped: Option<String>,
    /// 成果物の path（`artifact_produced`、出現順・重複なし）。
    pub artifacts: Vec<String>,
    /// 受け入れ条件ごとの最後の判定（`review_verdict`、条件番号の昇順）。
    pub acceptance: Vec<(usize, bool)>,
    /// 進捗ファイルの節（`None` は進捗ファイルが読めなかった）。
    pub progress: Option<ProgressSections>,
    /// 参考: worker の最後の完了申告（`done: ` を外したもの）。単独の根拠にしない。
    pub worker_outcome: Option<String>,
}

/// D4: DB の行と進捗ファイルだけから事実を集める（純粋関数）。
pub fn collect(inputs: RootDoneInputs<'_>) -> RootDoneFacts {
    let task = inputs.task;
    let mut phases = Vec::new();
    let mut integration_commit = None;
    let mut delivery_skipped = None;
    let mut artifacts: Vec<String> = Vec::new();
    let mut verdicts: BTreeMap<usize, bool> = BTreeMap::new();
    let mut worker_outcome = None;
    for (_, event) in inputs.events {
        match event {
            Event::PhaseIntegrated { phase, head, .. } => phases.push(PhaseFact {
                phase: phase.clone(),
                head: head.clone(),
            }),
            Event::IntegrationMerged { head_sha, .. } => {
                integration_commit = Some(head_sha.clone())
            }
            Event::DeliverySkipped { reason, detail, .. } => {
                delivery_skipped = Some(if detail.trim().is_empty() {
                    reason.code().to_string()
                } else {
                    format!("{}: {}", reason.code(), detail.trim())
                })
            }
            Event::ArtifactProduced { artifact, .. } => {
                if !artifacts.contains(&artifact.path) {
                    artifacts.push(artifact.path.clone());
                }
            }
            Event::ReviewVerdict {
                criterion_idx,
                pass,
                ..
            } => {
                verdicts.insert(*criterion_idx, *pass);
            }
            Event::WorkerFinished {
                outcome,
                role: None,
                ..
            } => {
                if let Some(s) = outcome.strip_prefix("done: ") {
                    worker_outcome = Some(s.to_string());
                }
            }
            _ => {}
        }
    }
    if integration_commit.is_none() {
        integration_commit = inputs
            .delivery
            .filter(|d| d.state == DeliveryState::Ready && d.release.is_some())
            .map(|d| d.head.clone());
    }
    RootDoneFacts {
        task_id: task.id,
        title: task.title.clone(),
        objective: task.objective.clone(),
        done_seq: done_transition_seq(inputs.events),
        children: inputs.children.to_vec(),
        phases,
        integration_commit,
        release: inputs.delivery.and_then(|d| d.release.clone()),
        delivery_state: inputs.delivery.map(|d| d.state),
        delivery_skipped,
        artifacts,
        acceptance: verdicts.into_iter().collect(),
        progress: inputs.progress_markdown.map(progress_sections),
        worker_outcome,
    }
}

/// 進捗ファイルの front matter の `tasks:` に `task_id` があるか（D4: 統合 commit の tree から進捗ファイルを引く）。
pub fn progress_mentions_task(markdown: &str, task_id: TaskId) -> bool {
    let id = task_id.to_string();
    let mut lines = markdown.lines();
    if lines.next().map(str::trim) != Some("---") {
        return false;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("tasks:") {
            return rest
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .any(|t| t.trim().trim_matches(['"', '\'']) == id);
        }
    }
    false
}

/// 進捗ファイルの節の有無（見出しは `## ` の完全一致で比べる。本文は次の `## ` まで）。
pub fn progress_sections(markdown: &str) -> ProgressSections {
    let mut sections: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut current: Option<&str> = None;
    let mut in_code = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if !in_code && let Some(h) = line.strip_prefix("## ") {
            let h = h.trim();
            current = Some(h);
            sections.entry(h).or_default();
            continue;
        }
        if !in_code && line.starts_with("# ") {
            current = None;
            continue;
        }
        if let Some(h) = current {
            sections.entry(h).or_default().push(line);
        }
    }
    let body = |name: &str| -> String {
        sections
            .get(name)
            .map(|ls| ls.join("\n").trim().to_string())
            .unwrap_or_default()
    };
    let migration = body("本番への移行");
    ProgressSections {
        migration_required: !migration.is_empty() && !is_none_note(&migration),
        has_unresolved: !body("未解決事項").is_empty() && !is_none_note(&body("未解決事項")),
        has_proposal: !body("提案").is_empty() && !is_none_note(&body("提案")),
    }
}

/// 節の本文が「なし」「不要」だけか（箇条書きの記号・句点・括弧の補足は無視する）。
fn is_none_note(body: &str) -> bool {
    let first = body.trim().trim_start_matches(['-', '*', ' ']).trim_start();
    ["なし", "無し", "不要", "特になし", "ありません"]
        .iter()
        .any(|w| first.starts_with(w))
        && body.lines().filter(|l| !l.trim().is_empty()).count() == 1
}

fn status_label(status: Status) -> &'static str {
    match status {
        Status::Draft => "draft",
        Status::Ready => "ready",
        Status::Running => "running",
        Status::Blocked => "blocked",
        Status::Reviewing => "reviewing",
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
    }
}

fn delivery_state_label(state: DeliveryState) -> &'static str {
    match state {
        DeliveryState::Reviewing => "reviewing",
        DeliveryState::MergeQueued => "merge_queued",
        DeliveryState::Merging => "merging",
        DeliveryState::Preparing => "preparing",
        DeliveryState::Ready => "ready",
        DeliveryState::Blocked => "blocked",
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn excerpt(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let head: String = flat.chars().take(max).collect();
    format!("{head}…")
}

/// 字数の上限（題名・目的・申告の抜粋）。
const TITLE_CHARS: usize = 120;
const OBJECTIVE_CHARS: usize = 600;
const OUTCOME_CHARS: usize = 300;
/// 事実ブロックに並べる子 task・段・成果物の上限（暴走しないため）。
const LIST_MAX: usize = 20;

/// web のリンク（`<base>/tasks/<id>`）。`base` の末尾の `/` は外す。
pub fn task_url(base_url: &str, task_id: TaskId) -> String {
    format!("{}/tasks/{task_id}", base_url.trim_end_matches('/'))
}

/// D5: スレッドへのリンク（`<base>/?thread=<thread_id>`）。
pub fn thread_url(base_url: &str, thread_id: &str) -> String {
    format!("{}/?thread={thread_id}", base_url.trim_end_matches('/'))
}

fn integration_line(facts: &RootDoneFacts) -> String {
    match (&facts.integration_commit, &facts.release) {
        (Some(c), Some(r)) => format!("{}（release {}）", short_sha(c), short_sha(r)),
        (Some(c), None) => short_sha(c).to_string(),
        (None, _) => match &facts.delivery_skipped {
            Some(why) => format!("取り込みなし（{}）", excerpt(why, TITLE_CHARS)),
            None => match facts.delivery_state {
                Some(state) => format!("取り込み途中（delivery {}）", delivery_state_label(state)),
                None => "取り込みなし".to_string(),
            },
        },
    }
}

fn migration_line(facts: &RootDoneFacts) -> &'static str {
    match facts.progress {
        Some(p) if p.migration_required => "あり（進捗ファイルの『本番への移行』を見る）",
        Some(_) => "なし",
        None => "不明（進捗ファイルなし）",
    }
}

/// D4: CoS スレッドに置く事実ブロック（system message。CoS 入力にもなる）。`base_url` があれば task へのリンクを入れる。
pub fn facts_block(facts: &RootDoneFacts, base_url: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "root task『{}』が完了しました（{}）。\n",
        excerpt(&facts.title, TITLE_CHARS),
        facts.task_id
    ));
    if let Some(base) = base_url {
        out.push_str(&format!("task: {}\n", task_url(base, facts.task_id)));
    }
    out.push_str(&format!(
        "\n目的: {}\n",
        excerpt(&facts.objective, OBJECTIVE_CHARS)
    ));
    if !facts.phases.is_empty() {
        out.push_str("\n段の統合:\n");
        for p in facts.phases.iter().take(LIST_MAX) {
            let head = if p.head.is_empty() {
                "-"
            } else {
                short_sha(&p.head)
            };
            out.push_str(&format!("- {} → {head}\n", p.phase));
        }
    }
    if !facts.children.is_empty() {
        out.push_str("\n子 task:\n");
        for c in facts.children.iter().take(LIST_MAX) {
            out.push_str(&format!(
                "- {}（{}）\n",
                excerpt(&c.title, TITLE_CHARS),
                status_label(c.status)
            ));
        }
    }
    out.push_str(&format!("\n統合 commit: {}\n", integration_line(facts)));
    if facts.artifacts.is_empty() {
        out.push_str("成果物: なし\n");
    } else {
        out.push_str("成果物:\n");
        for a in facts.artifacts.iter().take(LIST_MAX) {
            out.push_str(&format!("- {a}\n"));
        }
    }
    if facts.acceptance.is_empty() {
        out.push_str("受け入れ判定: 記録なし\n");
    } else {
        let parts: Vec<String> = facts
            .acceptance
            .iter()
            .map(|(i, pass)| format!("{i}:{}", if *pass { "合格" } else { "不合格" }))
            .collect();
        out.push_str(&format!("受け入れ判定: {}\n", parts.join(" ")));
    }
    out.push_str(&format!("本番への移行: {}\n", migration_line(facts)));
    if let Some(p) = facts.progress {
        if p.has_unresolved {
            out.push_str("未解決事項: あり\n");
        }
        if p.has_proposal {
            out.push_str("提案: あり\n");
        }
    }
    if let Some(o) = &facts.worker_outcome {
        out.push_str(&format!(
            "\n（参考・worker の申告）{}\n",
            excerpt(o, OUTCOME_CHARS)
        ));
    }
    out
}

/// D6: CoS が使えないときの Discord 向け短文（決定的）。`base_url` は必須で、空ならエラー（D5: リンク無しで
/// 成功扱いにしない）。スレッドがあればスレッドへのリンクも付ける。
pub fn fallback_body(
    facts: &RootDoneFacts,
    base_url: &str,
    thread_id: Option<&str>,
) -> Result<String, &'static str> {
    if base_url.trim().is_empty() {
        return Err("notify.web_base_url is missing or invalid");
    }
    let artifacts = if facts.artifacts.is_empty() {
        "なし".to_string()
    } else {
        format!("{} 件", facts.artifacts.len())
    };
    let mut out = format!(
        "完了: {}\n統合 commit: {}\n成果物: {artifacts}\n本番への移行: {}\n（CoS のまとめは無し: 決定的な短文）\n{}",
        excerpt(&facts.title, TITLE_CHARS),
        integration_line(facts),
        migration_line(facts),
        task_url(base_url, facts.task_id)
    );
    if let Some(thread) = thread_id {
        out.push('\n');
        out.push_str(&thread_url(base_url, thread));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
