//! root task の完了を CoS がまとめて Discord に知らせる（ADR 2026-10-15-root-done-cos-summary D1〜D6）。
//!
//! dispatcher は LLM を呼ばない。やるのは決定的な仕事だけ:
//!
//! 1. 検知（D2）: events の `Transitioned { to: Done }` を cursor（`feed_cursor` `root_done_summary`）で読み、
//!    `task_ops::root_done::is_root_done` を満たす task だけを `root_done_summaries` に 1 行（task + done の seq）
//!    入れる。子 task・support task・parent を持つ task は入らない。cursor が無い初回は現在の末尾から始める
//!    （過去の done を遡って鳴らさない）。
//! 2. 起動（D1/D3）: 事実ブロック（D4）を、その root 専用のスレッドに user message として置く。これは通常の
//!    CoS chat の queue（`chat_run_claim_next`）に乗り、既存の経路が CoS run を起こす。
//! 3. 通知（D5/D6）: run が completed で最終文がスレッドに書けたら、その文とスレッド・task へのリンクで
//!    `notifications`（kind `root_done_summary`、`UNIQUE(kind,key)` が重複排除）に 1 行作る。CoS が無効・run が
//!    失敗・止まった・未着手のまま待ちすぎたときは、事実から決定的に作った短文で同じ行を作る。
//!
//! 行の状態は一方向（new → queued → notified | fallback）。DB に残るので、再起動後は未確定の行から続く。

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

use task_core::chat::{ChatPostMessageRequest, ChatRunState, ChatSendMode, cos_thread_label};
use task_core::{
    DeliveryState, DeliveryStore, Event, NoticeStore, NotificationKind, NotificationStore,
    RootDoneState, RootDoneSummary, TaskId, TaskStore, WorkspaceSpec,
};
use task_ops::root_done::{self, ChildFact, RootDoneFacts, RootDoneInputs};
use time::OffsetDateTime;

use super::launch::CosChatLaunch;
use crate::dispatcher::Dispatcher;

/// `feed_cursor` の名前。
pub const ROOT_DONE_CURSOR: &str = "root_done_summary";
const EVENT_BATCH: usize = 512;
const EVENT_PAGES_PER_TICK: usize = 8;
/// 取り込み・release 準備が動いている間、事実が出揃うのを待つ上限。これを過ぎたら今ある事実で進める。
const DELIVERY_WAIT_SECS: i64 = 3600;
/// Discord 1 通の上限（2000 字）より手前。
const TEXT_MAX_CHARS: usize = 1900;
/// 通知に載せる CoS のまとめの字数。
const SUMMARY_MAX_CHARS: usize = 900;

/// CoS への指示。事実だけを根拠にさせ、task を変更させない。
const COS_INSTRUCTION: &str = "root task が完了しました。下は DB と進捗ファイルから決定的に集めた事実です。\
この事実だけを根拠に、(1) 何を目的にした仕事か (2) 何をやったか (3) 何が出たか（統合 commit・成果物・受け入れ判定）\
(4) 人が次にやること（本番への移行・未解決事項・提案・follow-up）を、人が数十秒で読める分量（日本語で 10 行以内）で\
このスレッドに書いてください。事実に無いことは書かず、分からない点は「不明」と書く。task や組織は変更しないこと。";

fn message_key(task_id: TaskId, seq: u64) -> String {
    format!("root-done:{}", root_done::summary_key(task_id, seq))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 統合 commit の tree（delivery の head）から、この task を `tasks:` に持つ進捗ファイルを探して本文を返す。
/// 読めなければ `None`（事実には「不明」と出る）。
fn progress_markdown(repo: &std::path::Path, head: &str, task_id: TaskId) -> Option<String> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let listing = git(&[
        "ls-tree",
        "-r",
        "--name-only",
        head,
        "--",
        "agent-docs/progress",
    ])?;
    for path in listing.lines().filter(|p| p.ends_with(".md")) {
        let Some(text) = git(&["show", &format!("{head}:{path}")]) else {
            continue;
        };
        if root_done::progress_mentions_task(&text, task_id) {
            return Some(text);
        }
    }
    None
}

impl CosChatLaunch {
    /// One pass: detect, start, finish. Errors are logged and retried next tick (the rows are durable).
    pub(crate) fn root_done_tick(&mut self, dispatcher: &Dispatcher) {
        let now = dispatcher.now_utc();
        if let Err(error) = self.root_done_ingest(now) {
            tracing::warn!(%error, "root-done intake failed");
            return;
        }
        if let Err(error) = self.root_done_advance_all(now) {
            tracing::warn!(%error, "root-done summary pass failed");
        }
    }

    /// D2: root の done 遷移を行にする。cursor は入れた後に進める（入れ直しは `INSERT OR IGNORE`）。
    fn root_done_ingest(&mut self, now: OffsetDateTime) -> Result<(), String> {
        let store = self.store.clone();
        let Some(raw) = store
            .feed_cursor_get(ROOT_DONE_CURSOR)
            .map_err(|e| e.to_string())?
        else {
            // 導入時: 過去に done になった root を遡って鳴らさない。
            let head = store.latest_event_id().map_err(|e| e.to_string())?;
            return store
                .feed_cursor_set(ROOT_DONE_CURSOR, &head.to_string())
                .map_err(|e| e.to_string());
        };
        let mut cursor: u64 = raw
            .parse()
            .map_err(|_| format!("invalid root-done cursor {raw}"))?;
        for _ in 0..EVENT_PAGES_PER_TICK {
            let rows = store
                .events_since(cursor, EVENT_BATCH)
                .map_err(|e| e.to_string())?;
            let Some(last) = rows.last().map(|r| r.id) else {
                break;
            };
            let mut seen: HashSet<TaskId> = HashSet::new();
            for row in &rows {
                if !matches!(
                    row.event,
                    Event::Transitioned {
                        to: task_core::Status::Done,
                        ..
                    }
                ) || !seen.insert(row.task_id)
                {
                    continue;
                }
                let Some(task) = store.get(row.task_id).map_err(|e| e.to_string())? else {
                    continue;
                };
                if !root_done::is_root_done(&task) {
                    continue;
                }
                let events = store.events_for(task.id).map_err(|e| e.to_string())?;
                if let Some(seq) = root_done::done_transition_seq(&events)
                    && store
                        .root_done_insert(task.id, seq, now)
                        .map_err(|e| e.to_string())?
                {
                    tracing::info!(task = %task.id, seq, "root task done: summary queued");
                }
            }
            store
                .feed_cursor_set(ROOT_DONE_CURSOR, &last.to_string())
                .map_err(|e| e.to_string())?;
            cursor = last;
            if rows.len() < EVENT_BATCH {
                break;
            }
        }
        Ok(())
    }

    fn root_done_advance_all(&mut self, now: OffsetDateTime) -> Result<(), String> {
        for row in self.store.root_done_open().map_err(|e| e.to_string())? {
            if let Err(error) = self.root_done_advance(&row, now) {
                let key = message_key(row.task_id, row.done_seq);
                if self.triage.root_done_warned.insert(key) {
                    tracing::warn!(%error, task = %row.task_id, "root-done summary step failed (retrying)");
                }
            }
        }
        Ok(())
    }

    /// D4: DB の行と進捗ファイルだけから事実を集める。
    fn root_done_facts(&self, task_id: TaskId) -> Result<RootDoneFacts, String> {
        let store = &self.store;
        let task = store
            .get(task_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("task {task_id} is gone"))?;
        let events = store.events_for(task_id).map_err(|e| e.to_string())?;
        let delivery = store.delivery_get(task_id).map_err(|e| e.to_string())?;
        let children: Vec<ChildFact> = store
            .children(task_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|c| ChildFact {
                title: c.title,
                status: c.status,
            })
            .collect();
        let progress = delivery.as_ref().and_then(|d| {
            let repo = store.repo_get(d.repo_id).ok().flatten()?;
            let path: PathBuf = match repo.location {
                WorkspaceSpec::Local { path, .. } => path,
                _ => return None,
            };
            progress_markdown(&path, &d.head, task_id)
        });
        Ok(root_done::collect(RootDoneInputs {
            task: &task,
            events: &events,
            delivery: delivery.as_ref(),
            children: &children,
            progress_markdown: progress.as_deref(),
        }))
    }

    fn root_done_advance(
        &mut self,
        row: &RootDoneSummary,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let age = (now - row.created_at).whole_seconds();
        match row.state {
            RootDoneState::New => {
                let facts = self.root_done_facts(row.task_id)?;
                // 取り込み・release 準備が動いている間は、統合 commit・release が出るのを待つ（上限あり）。
                if matches!(
                    facts.delivery_state,
                    Some(
                        DeliveryState::Reviewing
                            | DeliveryState::MergeQueued
                            | DeliveryState::Merging
                            | DeliveryState::Preparing
                    )
                ) && age < DELIVERY_WAIT_SECS
                {
                    return Ok(());
                }
                if !self.config.enabled || self.config.unavailable_reason.is_some() {
                    return self.root_done_fallback(row, &facts, None, now);
                }
                let key = message_key(row.task_id, row.done_seq);
                // ADR 2026-10-22-cos-fixed-threads D3: the project's CoS thread (or the general
                // CoS); the input's client_message_id keeps it idempotent. No new thread.
                let thread = self
                    .store
                    .cos_thread_for_task(&row.task_id.to_string(), now)
                    .map_err(|e| e.to_string())?;
                let text = format!(
                    "{COS_INSTRUCTION}\n\n{}",
                    root_done::facts_block(&facts, self.config.web_base_url.as_deref())
                );
                let posted = self
                    .store
                    .chat_message_post(
                        &thread.id,
                        &ChatPostMessageRequest {
                            client_message_id: key,
                            text,
                            attachment_ids: Vec::new(),
                            reply_to_id: None,
                            mode: ChatSendMode::Queue,
                            resume_queue: false,
                        },
                        now,
                    )
                    .map_err(|e| e.to_string())?;
                self.store
                    .root_done_advance(
                        row.task_id,
                        row.done_seq,
                        RootDoneState::New,
                        RootDoneState::Queued,
                        Some((&thread.id, &posted.response.message.id)),
                        now,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
            RootDoneState::Queued => {
                let (Some(thread_id), Some(input_id)) = (&row.thread_id, &row.input_message_id)
                else {
                    return Err("queued row without thread".into());
                };
                let runs = self
                    .store
                    .chat_run_list(thread_id, None, Some(50))
                    .map_err(|e| e.to_string())?;
                let run = runs.items.iter().find(|r| &r.input_message_id == input_id);
                match run.map(|r| (r.state, r)) {
                    Some((ChatRunState::Completed, run)) => {
                        let text = match &run.output_message_id {
                            Some(id) => self
                                .store
                                .chat_message_get(thread_id, id)
                                .map(|m| m.text)
                                .map_err(|e| e.to_string())?,
                            None => String::new(),
                        };
                        let facts = self.root_done_facts(row.task_id)?;
                        if text.trim().is_empty() {
                            return self.root_done_fallback(row, &facts, Some(thread_id), now);
                        }
                        self.root_done_notify_summary(row, &facts, thread_id, &text, now)
                    }
                    Some((
                        ChatRunState::Failed | ChatRunState::Stopped | ChatRunState::Interrupted,
                        _,
                    )) => {
                        let facts = self.root_done_facts(row.task_id)?;
                        self.root_done_fallback(row, &facts, Some(thread_id), now)
                    }
                    Some(_) => Ok(()),
                    None => {
                        let limit = i64::try_from(self.config.triage.unavailable_after_secs)
                            .unwrap_or(i64::MAX);
                        if age <= limit {
                            return Ok(());
                        }
                        // 未着手のまま待ちすぎた（容量待ち・停止）: 入力を取り下げて決定的な短文に切り替える。
                        let _ = self.store.chat_message_cancel(thread_id, input_id, now);
                        let facts = self.root_done_facts(row.task_id)?;
                        self.root_done_fallback(row, &facts, Some(thread_id), now)
                    }
                }
            }
            RootDoneState::Notified | RootDoneState::Fallback => Ok(()),
        }
    }

    /// ADR 2026-10-22-cos-fixed-threads D3: 「[統括 CoS] 」「[CoS: <案件>] 」。引けなければ空。
    fn thread_label_prefix(&self, thread_id: &str) -> String {
        self.store
            .chat_thread_get(thread_id)
            .ok()
            .flatten()
            .and_then(|t| cos_thread_label(&t))
            .map(|label| format!("{label} "))
            .unwrap_or_default()
    }

    /// D5: CoS の最終文がスレッドに書けた後に、本文を確定して通知の行を作る。リンクが作れなければ作らない。
    fn root_done_notify_summary(
        &mut self,
        row: &RootDoneSummary,
        facts: &RootDoneFacts,
        thread_id: &str,
        summary: &str,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let base = self
            .config
            .web_base_url
            .as_deref()
            .filter(|b| !b.trim().is_empty())
            .ok_or("notify.web_base_url is missing or invalid")?;
        let flat = summary.trim();
        let tail = format!(
            "\nスレッド: {}\ntask: {}",
            root_done::thread_url(base, thread_id),
            root_done::task_url(base, row.task_id)
        );
        let budget = TEXT_MAX_CHARS.saturating_sub(tail.chars().count());
        let head = format!(
            "{}完了: {}\n",
            self.thread_label_prefix(thread_id),
            truncate(&facts.title, 120)
        );
        let room = budget
            .saturating_sub(head.chars().count())
            .min(SUMMARY_MAX_CHARS);
        let body = format!("{head}{}{tail}", truncate(flat, room));
        self.root_done_finish(row, body, RootDoneState::Notified, facts, now)
    }

    /// D6: CoS が使えない・失敗したときの決定的な短文。スレッドがあれば system message も残す。
    fn root_done_fallback(
        &mut self,
        row: &RootDoneSummary,
        facts: &RootDoneFacts,
        thread_id: Option<&String>,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let base = self.config.web_base_url.clone().unwrap_or_default();
        let body = root_done::fallback_body(facts, &base, thread_id.map(String::as_str))
            .map_err(str::to_owned)?;
        let body = format!(
            "{}{body}",
            thread_id.map_or_else(String::new, |t| self.thread_label_prefix(t))
        );
        if let Some(thread) = thread_id {
            let key = format!(
                "root-done-fallback:{}",
                root_done::summary_key(row.task_id, row.done_seq)
            );
            self.store
                .chat_system_message_add_once(
                    thread,
                    &key,
                    "CoS のまとめは書けなかったので、決定的な短文で通知しました。",
                    &[],
                    now,
                )
                .map_err(|e| e.to_string())?;
        }
        self.root_done_finish(
            row,
            truncate(&body, TEXT_MAX_CHARS),
            RootDoneState::Fallback,
            facts,
            now,
        )
    }

    /// 通知の行（`UNIQUE(kind,key)` が重複排除）を作ってから状態を確定する。途中で落ちても次の tick で同じ分岐に
    /// 戻り、行は `INSERT OR IGNORE` で 2 つにならない。
    fn root_done_finish(
        &mut self,
        row: &RootDoneSummary,
        body: String,
        to: RootDoneState,
        _facts: &RootDoneFacts,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let project = self
            .store
            .get(row.task_id)
            .map_err(|e| e.to_string())?
            .and_then(|t| t.project_id);
        self.store
            .notification_upsert_pending(
                NotificationKind::RootDoneSummary,
                &root_done::summary_key(row.task_id, row.done_seq),
                &body,
                project,
                now,
            )
            .map_err(|e| e.to_string())?;
        self.store
            .root_done_advance(row.task_id, row.done_seq, row.state, to, None, now)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
