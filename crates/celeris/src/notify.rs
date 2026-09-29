//! 人の判断が要るときだけ Discord に知らせる（ADR-0037。Phase 39）。
//!
//! ADR-0050: 通常仕事の完了・全体対話も通知する。未解決の待ちは再起動後も対象、
//! 終端イベントの走査時刻はDBに永続化する。以下のPhase記録の起動時刻制限を更新した。
//!
//! ここには 2 つのことしか無い:
//!
//! 1. **判定**（`scan` / `schedule`）— DB を読んで「人の手が要る」5 種の条件を**決定的に**見つけ、
//!    `notifications` にまだ無い `(kind, key)` を pending として 1 件だけ作る。LLM は関与しない。
//!    `tick_loop` の中から同期で呼ばれる（B1: チャネルに送らず、その場で store を見る）。
//! 2. **送信**（`spawn_send` / `post_webhook`）— pending を Discord の webhook へ POST する。
//!    tick をブロックしないよう `tokio::spawn` で送り、**結果は次の tick で** `notification_mark` する
//!    （送信結果は `mpsc` でループへ戻る。ADR-0022 D2 の `check` と同じ形）。
//!
//! 秘密（webhook URL）の規律（ADR-0037 D3）: **URL はログにもエラー文にも応答にも出さない**。
//! `reqwest::Error` の `Display` は URL を含むので、そのまま文字列にしてはいけない（`safe_error` を使う）。
//!
//! Phase 40（実機 2026-09-18。ADR-0037 D5）: 最初の走査で `bad_news` の履歴 8 件が同じ秒に一斉送信され
//! 429 が 3 件出た一方、Go 待ちの draft がある途中目標では `milestone_ready` が一生鳴らなかった。
//! これを受けて 3 つを直した: (1) `milestone_ready` は「動いているものが無く、人の手が要る」状態
//! （ready/running/reviewing/blocked が 0、done が 1 件以上）で鳴る。全終端である必要はない。
//! Phase 41（ADR-0038 D4）: `milestone_ready` は**秘書のレビューの返事が付いてから**送り、文面に
//! 「結果 → 次の提案」の要約（返事の先頭 300 字と提案の題名）を載せる（状態だけを知らせても、人は
//! GUI で成果物を読んで自分で次を考えなければならなかった）。条件の判定そのものは
//! `crate::milestone_review::ready_milestones` に移した（レビューの run を起こす側と同じ 1 か所）。
//! **ADR-0079 D13（Phase R5a）: `milestone_ready` と途中目標の判定 run は廃止**（途中目標は凍結。root の完了は
//! `task_ready`、人の判断は決定の要求・計画の承認で鳴る）。`NotificationKind::MilestoneReady` は既存の行を読むためだけに残る。
//!
//! (2) `bad_news`/`approval_pending`/`question_blocked`/`secretary_reply` は celeris の起動時刻より
//! 後にできたものだけを対象にする（backfill 禁止）。(3) 送信は 1 tick に最大 1 通、`bad_news` は
//! 束ねる（`select_batch`）、429 は attempts に数えず `Retry-After` の間だけ待つ。

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use task_core::message::MessageRole;
use task_core::notify::{MAX_NOTIFY_ATTEMPTS, NotificationId, NotificationKind};
use task_core::report::{ReportFilter, ReportKind};
use task_core::{
    ListFilter, ListOrder, Notification, ProjectStatus, Status, StoreError, TaskStore,
};
use time::OffsetDateTime;

/// `[notify] interval_secs` の既定（ADR-0037 D3）。
pub const DEFAULT_INTERVAL_SECS: u64 = 30;
/// webhook への POST のタイムアウト（ADR-0037 D3）。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Discord の 1 通の上限（2000 字）より少し手前で切る。
pub const CONTENT_MAX_CHARS: usize = 1900;
/// 文面に載せる質問・見出しの字数（ADR-0037 D1「先頭 120 字」）。
pub const EXCERPT_CHARS: usize = 120;
/// `milestone_ready` に載せる秘書のまとめの字数（ADR-0038 D4「先頭 300 字」）。
pub const REVIEW_EXCERPT_CHARS: usize = 300;
/// 1 tick で見る案件の上限（案件はそう多くない。暴走しないための上限）。
const PROJECT_SCAN: usize = 200;
/// 1 案件あたりに見るタスクの上限。
const TASK_SCAN: usize = 1_000;
/// 悪い知らせを探すときに見る報告の件数。
const REPORT_SCAN: usize = 200;
/// 秘書の返事を探すときに読む対話の件数。
const MESSAGE_SCAN: usize = 50;
/// 送信の本文に付ける名前（ADR-0037 D3）。
const WEBHOOK_USERNAME: &str = "Celeris";
/// `Retry-After` が読めないときに待つ既定の秒数（ADR-0037 D3。実機 2026-09-18: 8 件一斉送信で 429）。
pub const DEFAULT_RETRY_AFTER_SECS: u64 = 5;
/// `bad_news` を束ねるときの接頭辞（`scan_bad_news` が単発送信用に付けたものを剥がして束ねる）。
const BAD_NEWS_PREFIX: &str = "悪い知らせ: ";

/// `[notify]`（ADR-0037 D2 / D3）。秘密の id と間隔とリンクの根だけを持つ。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    /// `[secrets]` の中の webhook URL の id。無ければ何も送らない（エラーにしない）。
    #[serde(default = "default_webhook_secret")]
    pub discord_webhook_secret: String,
    /// 判定と送信を行う間隔（秒）。
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    /// 例: `"http://192.168.1.103:7700"`。無ければ文面にリンクを入れない。
    #[serde(default)]
    pub gui_base_url: Option<String>,
}

fn default_webhook_secret() -> String {
    task_core::notify::DEFAULT_WEBHOOK_SECRET_ID.to_string()
}

fn default_interval_secs() -> u64 {
    DEFAULT_INTERVAL_SECS
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            discord_webhook_secret: default_webhook_secret(),
            interval_secs: default_interval_secs(),
            gui_base_url: None,
        }
    }
}

impl NotifyConfig {
    /// 末尾の `/` を落とした GUI の根（空文字列は「無い」とみなす）。
    pub fn base_url(&self) -> Option<&str> {
        self.gui_base_url
            .as_deref()
            .map(|u| u.trim_end_matches('/'))
            .filter(|u| !u.is_empty())
    }
}

/// 判定が見つけた 1 件（まだ DB には入っていない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub kind: NotificationKind,
    pub key: String,
    pub body: String,
    /// GUI がリンクを作るための案件 id（ADR-0037 D6 / GUI 依頼 G13i-P1）。`milestone_ready` はその
    /// 途中目標の案件、`secretary_reply` はその案件自身、他の種は `None`。
    pub project_id: Option<task_core::ProjectId>,
}

/// 送信の結果（spawn した先から tick ループへ戻る）。`ids` は 1 件のことも、
/// `bad_news` を束ねた複数件のこともある（ADR-0037 D4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendResult {
    pub ids: Vec<NotificationId>,
    pub outcome: SendOutcome,
}

/// 1 回の POST の結末。**429 は失敗ではない**（ADR-0037 D3 / 実機 2026-09-18）: attempts に数えず、
/// `retry_after` の間だけ次の送信を控える。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// 送れた。
    Sent,
    /// Discord の 1 秒あたりの上限に当たった。この時間が経つまで次を送らない。
    RateLimited(Duration),
    /// 送れなかった（**URL・ホスト名は含まない**短い理由）。
    Failed(String),
}

// ---- 文面の部品（決定的。LLM は呼ばない）----

/// 文字数で切って `…` を付ける（バイトではない）。
fn excerpt(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let head: String = flat.chars().take(max).collect();
    format!("{head}…")
}

/// Discord の 1 通に収める。
fn clamp_content(text: &str) -> String {
    if text.chars().count() <= CONTENT_MAX_CHARS {
        return text.to_string();
    }
    text.chars().take(CONTENT_MAX_CHARS).collect()
}

fn link(base: Option<&str>, path: &str) -> String {
    match base {
        Some(base) => format!("\n{base}{path}"),
        None => String::new(),
    }
}

/// 組織のノードの表示名（知らない id はそのまま）。
fn node_name(org: &[task_core::OrgNode], id: &str) -> String {
    org.iter()
        .find(|n| n.id == id)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| id.to_string())
}

// ---- 判定（ADR-0037 D1 の 5 種。決定的）----

/// tick ごとに DB を読み、「人の判断が要る」条件に当たるものを全部返す（重複排除はまだしない）。
/// 並びは `NotificationKind::ALL` の順、同じ種の中は id の昇順で決定的。
///
/// `started_at` は終端イベントの走査下限。schedule は永続化済みの走査時刻を渡す。
pub fn scan(
    store: &dyn TaskStore,
    started_at: OffsetDateTime,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    scan_at(store, started_at, base_url, OffsetDateTime::now_utc())
}

fn scan_at(
    store: &dyn TaskStore,
    started_at: OffsetDateTime,
    base_url: Option<&str>,
    now: OffsetDateTime,
) -> Result<Vec<Candidate>, StoreError> {
    let org = store.org_list()?;
    let mut out = Vec::new();
    // ADR-0079 D13（Phase R5a）: `milestone_ready`（途中目標の判定の通知）は廃止。root の完了は `task_ready` で鳴る。
    out.extend(scan_approval_pending(store, &org, started_at, base_url)?);
    out.extend(scan_question_blocked(store, &org, started_at, base_url)?);
    out.extend(scan_bad_news(store, started_at, base_url)?);
    out.extend(scan_secretary_reply(store, &org, started_at, base_url)?);
    out.extend(scan_task_ready(store, started_at, base_url)?);
    out.extend(scan_task_failed(store, started_at, base_url)?);
    out.extend(scan_phase_checkpoint(store, base_url)?);
    out.extend(scan_cluster_login_needed(store, started_at, base_url)?);
    out.extend(scan_decisions(store, started_at, base_url, now)?);
    out.extend(scan_plan_approvals(store, base_url)?);
    // リリースの引き渡しは最新の対話・途中目標・走査時刻に隠されない。
    for delivery in store.delivery_list()? {
        if let Some(id) = delivery.notification {
            let ready = delivery.state == task_core::DeliveryState::Ready;
            if !ready
                && (delivery.state != task_core::DeliveryState::Blocked
                    || !delivery.detail.trim_start().starts_with("[needs-human]"))
            {
                continue;
            }
            out.push(Candidate {
                kind: NotificationKind::SecretaryReply,
                key: format!("message:{id}"),
                body: format!(
                    "{}: {}{}",
                    if ready {
                        "デプロイ準備完了"
                    } else {
                        "CoSから確認が必要です"
                    },
                    excerpt(&delivery.detail, REVIEW_EXCERPT_CHARS),
                    link(
                        base_url,
                        &if ready {
                            "/releases".into()
                        } else {
                            format!("/tasks/{}?tab=changes", delivery.task_id)
                        }
                    )
                ),
                project_id: Some(delivery.project_id),
            });
        }
    }
    Ok(out)
}

/// 未決の認可。解決するまで再起動に関係なく対象（台帳で重複排除）。
fn scan_approval_pending(
    store: &dyn TaskStore,
    org: &[task_core::OrgNode],
    _started_at: OffsetDateTime,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    let mut out = Vec::new();
    let mut pending: Vec<_> = store
        .approval_list(Some(true), None, None)?
        .into_iter()
        .collect();
    pending.sort_by_key(|a| a.id);
    for approval in pending {
        let body = format!(
            "認可の要求: {}『{}』{}",
            node_name(org, &approval.node_id),
            excerpt(&approval.question, EXCERPT_CHARS),
            link(base_url, "/approvals")
        );
        out.push(Candidate {
            kind: NotificationKind::ApprovalPending,
            key: approval.id.to_string(),
            body,
            project_id: None,
        });
    }
    Ok(out)
}

/// 未解決の質問。同じタスクの未決認可のみを approval_pending に任せる。
fn scan_question_blocked(
    store: &dyn TaskStore,
    org: &[task_core::OrgNode],
    _started_at: OffsetDateTime,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    let with_approval: HashSet<String> = store
        .approval_list(Some(true), None, None)?
        .into_iter()
        .filter_map(|a| a.task_id.map(|t| t.to_string()))
        .collect();
    let filter = ListFilter {
        statuses: vec![Status::Blocked],
        ..ListFilter::default()
    };
    let page = store.list_page(&filter, ListOrder::CreatedDesc, None, TASK_SCAN)?;
    let mut tasks: Vec<_> = page.items.into_iter().collect();
    tasks.sort_by_key(|a| a.id);
    let mut out = Vec::new();
    for task in tasks {
        if with_approval.contains(&task.id.to_string()) {
            continue;
        }
        let events = store.events_for(task.id)?;
        // ADR-0074 D2.4（Phase F3 途中確認）: 工程の後の途中確認は質問ではない
        // （`scan_phase_checkpoint` が鳴らす。二重に鳴らさない）。
        // ADR-0079 D8（Phase R3b）: root の計画の承認待ちも質問ではない（`scan_plan_approvals` が鳴らす）。
        if task_ops::plan_gate::is_human_gate(&task, &events) {
            continue;
        }
        let who = task
            .assignee
            .as_deref()
            .map(|id| node_name(org, id))
            .unwrap_or_else(|| "担当".to_string());
        let question = events
            .iter()
            .rev()
            .find_map(|(_, event)| match event {
                task_core::Event::QuestionRaised { text, .. } => Some(text.as_str()),
                task_core::Event::WorkerFinished {
                    outcome,
                    role: None,
                    ..
                } => outcome.strip_prefix("question: "),
                _ => None,
            })
            .unwrap_or(&task.title);
        let where_to = link(base_url, &format!("/tasks/{}", task.id));
        let body = format!(
            "{who} が質問で止まっています: {}{where_to}",
            excerpt(question, EXCERPT_CHARS)
        );
        out.push(Candidate {
            kind: NotificationKind::QuestionBlocked,
            key: transition_key(&task, &events),
            body,
            project_id: task.project_id,
        });
    }
    Ok(out)
}

/// 秘書レベル（level 0）の `bad_news` 報告。**celeris の起動時刻より後にできたものだけ**（backfill 禁止。
/// 実機 2026-09-18: 起動直後に今日の履歴 8 件が一斉送信され、429 で 3 件落ちた）。
fn scan_bad_news(
    store: &dyn TaskStore,
    started_at: OffsetDateTime,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    let filter = ReportFilter {
        level: Some(0),
        limit: REPORT_SCAN,
        ..ReportFilter::default()
    };
    let mut reports: Vec<_> = store
        .report_list(&filter)?
        .into_iter()
        .filter(|r| {
            r.kind == ReportKind::BadNews
                && r.created_at >= started_at
                // ADR-0053 D3（Phase 66）: クラスタの TOTP ログイン待ちは `cluster_login_needed`
                // として別に知らせる（scan_cluster_login_needed）。ここでは二重に鳴らさない。
                && !task_core::report::is_cluster_login_needed_headline(&r.headline)
        })
        .collect();
    reports.sort_by_key(|a| a.id);
    // ADR-0079 D11（Phase R3b）: 木の子 task の run の悪い知らせ（run の失敗）は親が吸収するので鳴らさない
    // （報告の流れには残る）。基盤の障害は `tree-infra:` / `tree-stall:` の `TaskFailed` が知らせる。
    let mut tree_child: HashSet<task_core::TaskId> = HashSet::new();
    for id in reports.iter().filter_map(|r| r.task_id) {
        if !tree_child.contains(&id)
            && store
                .get(id)?
                .is_some_and(|t| task_core::tree::is_tree_child(&t))
        {
            tree_child.insert(id);
        }
    }
    reports.retain(|r| r.task_id.is_none_or(|id| !tree_child.contains(&id)));
    Ok(reports
        .into_iter()
        .map(|report| Candidate {
            kind: NotificationKind::BadNews,
            key: report.id.to_string(),
            body: format!(
                "悪い知らせ: {}{}",
                excerpt(&report.headline, EXCERPT_CHARS),
                link(base_url, "/reports")
            ),
            project_id: None,
        })
        .collect())
}

/// ADR-0053 D3（Phase 66）: クラスタの ssh master が落ち、鍵認証も失敗した（人の TOTP が要る）。
/// `record_cluster_login_needed_report`（`task_core::report::report_for_cluster_login_needed`）が
/// 書いた `bad_news` の中から、見出しでこれだけを拾う（`ReportKind` は増やさない）。
/// `key` = 報告 id（Dispatcher が outage ごとに 1 回だけ報告を作るので、これで「1 outage = 1 通知」になる）。
fn scan_cluster_login_needed(
    store: &dyn TaskStore,
    started_at: OffsetDateTime,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    let filter = ReportFilter {
        level: Some(0),
        limit: REPORT_SCAN,
        ..ReportFilter::default()
    };
    let mut reports: Vec<_> = store
        .report_list(&filter)?
        .into_iter()
        .filter(|r| {
            r.kind == ReportKind::BadNews
                && r.created_at >= started_at
                && task_core::report::is_cluster_login_needed_headline(&r.headline)
        })
        .collect();
    reports.sort_by_key(|a| a.id);
    Ok(reports
        .into_iter()
        .map(|report| Candidate {
            kind: NotificationKind::ClusterLoginNeeded,
            key: report.id.to_string(),
            body: format!(
                "クラスタのログインが必要: {}{}",
                excerpt(&report.headline, EXCERPT_CHARS),
                link(base_url, "/clusters")
            ),
            project_id: None,
        })
        .collect())
}

/// 全体対話と継続中の案件の最新の返事。案件ではなく message id で重複排除する。
fn scan_secretary_reply(
    store: &dyn TaskStore,
    org: &[task_core::OrgNode],
    since: OffsetDateTime,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    let mut projects = store.project_list()?;
    projects.sort_by_key(|p| p.id);
    let scopes = std::iter::once(None).chain(
        projects
            .iter()
            .filter(|p| {
                p.archived_at.is_none()
                    && matches!(p.status, ProjectStatus::Proposed | ProjectStatus::Active)
            })
            .take(PROJECT_SCAN)
            .map(|p| Some(p.id)),
    );
    let mut out = Vec::new();
    for scope in scopes {
        for node in org {
            let messages = store.message_list(&node.id, scope, MESSAGE_SCAN)?;
            let Some(reply) = messages.iter().max_by_key(|m| (m.created_at, m.id)) else {
                continue;
            };
            if reply.role != MessageRole::Node || reply.created_at < since {
                continue;
            }
            // 自動引き渡しの機械的メッセージは専用の判定で通知する。
            if let Some(id) = reply.task_id
                && let Some(delivery) = store.delivery_get(id)?
                && (delivery.notification == Some(reply.id)
                    || reply.run_id.as_deref() == Some(delivery.review_run.as_str()))
            {
                continue;
            }
            out.push(Candidate {
                kind: NotificationKind::SecretaryReply,
                key: format!("message:{}", reply.id),
                body: format!(
                    "{} から返事が届きました: {}{}",
                    node.name,
                    excerpt(&reply.text, REVIEW_EXCERPT_CHARS),
                    link(
                        base_url,
                        &scope
                            .map(|id| format!("/?scope=project:{id}"))
                            .unwrap_or_else(|| "/".into())
                    )
                ),
                project_id: scope,
            });
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

/// 更新時刻（コメント等でも動く）ではなく状態遷移ごとに通知する。旧データには id を使う。
fn transition_key(task: &task_core::Task, events: &[(u64, task_core::Event)]) -> String {
    match events
        .iter()
        .rev()
        .find(|(_, e)| matches!(e, task_core::Event::Transitioned { to, .. } if *to == task.status))
    {
        Some((seq, _)) => format!("{}:{seq}", task.id),
        None => task.id.to_string(),
    }
}

fn scan_task_ready(
    store: &dyn TaskStore,
    since: OffsetDateTime,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    let tasks = store.list_page(
        &ListFilter {
            statuses: vec![Status::Done],
            ..Default::default()
        },
        ListOrder::UpdatedDesc,
        None,
        TASK_SCAN,
    )?;
    let mut out = Vec::new();
    // ADR-0079 D6 / D11（Phase R1c）: 木の子 task の done は「親の段階で取り込まれる準備ができた」だけで、
    // 人に確認を求めない（鳴るのは root の done だけ）。
    for task in tasks.items.into_iter().filter(|t| {
        t.updated_at >= since
            && t.milestone_id.is_none()
            && task_core::support_kind(t).is_none()
            && !task_core::tree::is_tree_child(t)
    }) {
        let events = store.events_for(task.id)?;
        let summary = events
            .iter()
            .rev()
            .find_map(|(_, e)| match e {
                task_core::Event::WorkerFinished {
                    outcome,
                    role: None,
                    ..
                } => outcome.strip_prefix("done: "),
                _ => None,
            })
            .unwrap_or("");
        // 部署内の取り込み中には人の判断を求めない。検証後のdelivery通知にまとめる。
        if store.delivery_get(task.id)?.is_some() {
            continue;
        }
        out.push(Candidate {
            kind: NotificationKind::TaskReady,
            key: transition_key(&task, &events),
            body: format!(
                "仕事『{}』{}{}{}",
                excerpt(&task.title, EXCERPT_CHARS),
                "が完了しました。成果を確認してください。",
                excerpt(summary, REVIEW_EXCERPT_CHARS),
                link(base_url, &format!("/tasks/{}", task.id))
            ),
            project_id: task.project_id,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

/// ADR-0070 D1（Phase 116）: タスクが `failed` になった。**celeris の起動時刻より後**（backfill 禁止。
/// ADR-0037 D5 と同じ規律）。分類（`infra`/`work`）と理由 1 行、main に取り込み済み（`deliveries` に `release` が
/// 付いた記録がある）なら「成果は main に取り込み済み（release <sha12>）だがレビューで不合格」を文面に組み立てる
/// （判定は task_ops の純粋関数、文面の組み立てだけをここで行う）。`key` は `transition_key`（同じ
/// タスクが後で再び failed になれば新しい key になる）。
fn scan_task_failed(
    store: &dyn TaskStore,
    since: OffsetDateTime,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    let tasks = store.list_page(
        &ListFilter {
            statuses: vec![Status::Failed],
            ..Default::default()
        },
        ListOrder::UpdatedDesc,
        None,
        TASK_SCAN,
    )?;
    let mut out = Vec::new();
    // ADR-0079 D11（Phase R3b）: 木の子 task の失敗は親が吸収する（work の失敗は親の replan、基盤の失敗は作り直しと
    // `tree-infra:` の障害通知）ので鳴らさない。鳴るのは root の失敗だけ（key の形は変えない）。
    for task in tasks.items.into_iter().filter(|t| {
        t.updated_at >= since
            && task_core::support_kind(t).is_none()
            && !task_core::tree::is_tree_child(t)
    }) {
        let events = store.events_for(task.id)?;
        let (class, reason) = task_ops::derive::classify_task_failure(&events);
        let delivered_release = store.delivery_get(task.id)?.and_then(|d| d.release);
        let delivered_note = match &delivered_release {
            Some(sha12) => {
                format!("成果は main に取り込み済み（release {sha12}）だがレビューで不合格。")
            }
            None => String::new(),
        };
        let body = format!(
            "失敗（{}）: {}{delivered_note}{}{}",
            class.as_str(),
            excerpt(&task.title, EXCERPT_CHARS),
            excerpt(&reason, EXCERPT_CHARS),
            link(base_url, &format!("/tasks/{}", task.id))
        );
        out.push(Candidate {
            kind: NotificationKind::TaskFailed,
            key: transition_key(&task, &events),
            body,
            project_id: task.project_id,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

/// ADR-0079 D7（Phase R3a）: 未回答の決定の要求が 24 時間たったら 1 回だけ再通知する。
pub const DECISION_REMINDER_AFTER: time::Duration = time::Duration::hours(24);

/// ADR-0079 D7: 決定の path のパンくず（「『root』› 段階 › 子 › …」。`stage` は次の段が属する段階）。
pub fn decision_breadcrumb(path: &[task_core::DecisionPathEntry]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (i, p) in path.iter().enumerate() {
        if i == 0 {
            parts.push(format!("『{}』", excerpt(&p.title, EXCERPT_CHARS)));
        } else {
            parts.push(excerpt(&p.title, EXCERPT_CHARS));
        }
        if let Some(stage) = &p.stage {
            parts.push(stage.clone());
        }
    }
    parts.join(" › ")
}

fn cost_label(cost: task_core::CostOfReversal) -> &'static str {
    match cost {
        task_core::CostOfReversal::Low => "小",
        task_core::CostOfReversal::Medium => "中",
        task_core::CostOfReversal::High => "大",
    }
}

/// ADR-0079 D7: 決定 1 件の文面の 1 行（「<path>: <question>（推奨: <label>、後戻り: 中）」）。
fn decision_line(row: &task_core::DecisionRow) -> String {
    let r = &row.request;
    let recommended = r
        .options
        .iter()
        .find(|o| o.key == r.recommended)
        .map(|o| o.label.as_str())
        .unwrap_or(r.recommended.as_str());
    format!(
        "{}: {}（推奨: {}、後戻り: {}）",
        decision_breadcrumb(&r.path),
        excerpt(&r.question, REVIEW_EXCERPT_CHARS),
        excerpt(recommended, EXCERPT_CHARS),
        cost_label(r.cost_of_reversal)
    )
}

/// ADR-0079 D7（Phase R3a）: 人への決定の要求（`NotificationKind::DecisionRequested`）。
///
/// - **束ね**: 同じ run（計画の採用の planner run・worker の run）で出た決定は 1 通にする（`key` =
///   `plan:<plan_id>:decisions`〈その run が採用した計画〉/ `run:<run_id>:decisions`）。run を持たない daemon の決定
///   （木の run の上限など）は `decision:<id>` の 1 通。
/// - 初回は `started_at`（走査の下限。backfill 禁止）より後に出た決定だけ。**未回答のまま 24 時間たったら** 同じ束で
///   1 回だけ再通知（`key` の先頭に `reminder:`。状態で判定するので下限は使わない）。回答・取り下げ・節点が終端の
///   決定は鳴らさない。
fn scan_decisions(
    store: &dyn TaskStore,
    started_at: OffsetDateTime,
    base_url: Option<&str>,
    now: OffsetDateTime,
) -> Result<Vec<Candidate>, StoreError> {
    use std::collections::BTreeMap;
    use time::format_description::well_known::Rfc3339;
    let mut groups: BTreeMap<String, Vec<task_core::DecisionRow>> = BTreeMap::new();
    let mut terminal: BTreeMap<task_core::TaskId, Option<task_core::Task>> = BTreeMap::new();
    let mut plan_of_run: BTreeMap<String, Option<String>> = BTreeMap::new();
    for row in store.decisions_list(None)? {
        if row.status != task_core::DecisionStatus::Open {
            continue;
        }
        let node = match terminal.get(&row.task_id) {
            Some(n) => n.clone(),
            None => {
                let n = store.get(row.task_id)?;
                terminal.insert(row.task_id, n.clone());
                n
            }
        };
        if node.as_ref().is_none_or(|t| t.status.is_terminal()) {
            continue;
        }
        let key = match row.request.raised_by.run_id.as_deref() {
            Some(run) => {
                let plan = match plan_of_run.get(run) {
                    Some(p) => p.clone(),
                    None => {
                        let p = store
                            .execution_plan_list(row.task_id)?
                            .into_iter()
                            .find(|p| p.planner_run_id.as_deref() == Some(run))
                            .map(|p| p.id);
                        plan_of_run.insert(run.to_string(), p.clone());
                        p
                    }
                };
                match plan {
                    Some(plan_id) => format!("plan:{plan_id}:decisions"),
                    None => format!("run:{run}:decisions"),
                }
            }
            // ADR-0079 R5b-prep: 人の計画（origin human、run を持たない）の決定は、その決定を持つ人の計画の版に
            // 束ねる（planner の計画と同じ `plan:<plan_id>:decisions`。PUT で出た h4 / h5 / h7 が 1 通になる）。
            None if row.request.raised_by.origin == task_core::DecisionOrigin::Human => {
                let plan = store
                    .execution_plan_list(row.task_id)?
                    .into_iter()
                    .rev()
                    .find(|p| {
                        p.origin == task_core::PlanOrigin::Human
                            && task_core::normalized_decisions(&p.spec)
                                .iter()
                                .any(|d| d.key == row.key)
                    })
                    .map(|p| p.id);
                match plan {
                    Some(plan_id) => format!("plan:{plan_id}:decisions"),
                    None => format!("decision:{}", row.id),
                }
            }
            None => format!("decision:{}", row.id),
        };
        groups.entry(key).or_default().push(row);
    }
    let mut out = Vec::new();
    for (key, rows) in groups {
        let Some(first) = rows.first() else {
            continue;
        };
        // ADR-0079 D8（Phase R3b）: 承認を待つ root の計画の決定は、計画の承認の通知（`scan_plan_approvals`）に束ねる。
        if let Some(plan_id) = key
            .strip_prefix("plan:")
            .and_then(|k| k.strip_suffix(":decisions"))
            && let Some(Some(node)) = terminal.get(&first.task_id)
            && task_ops::plan_gate::latest_plan_approval(node, &store.events_for(node.id)?)
                .is_some_and(|a| a.plan_id == plan_id)
        {
            continue;
        }
        let created =
            |r: &task_core::DecisionRow| OffsetDateTime::parse(&r.created_at, &Rfc3339).ok();
        let oldest = rows.iter().filter_map(created).min();
        let project_id = terminal
            .get(&first.task_id)
            .and_then(|t| t.as_ref())
            .and_then(|t| t.project_id);
        let link_path = format!("/tasks/{}", first.root_id);
        let body = |prefix: &str| -> String {
            if rows.len() == 1 {
                format!(
                    "{prefix}人の決定が必要: {}{}",
                    decision_line(first),
                    link(base_url, &link_path)
                )
            } else {
                let lines = rows
                    .iter()
                    .enumerate()
                    .map(|(i, r)| format!("{}) {}", i + 1, decision_line(r)))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "{prefix}人の決定が {} 件必要:\n{lines}{}",
                    rows.len(),
                    link(base_url, &link_path)
                )
            }
        };
        // 初回（backfill 禁止: この走査の下限より後に出たものが 1 件でもある束）。
        if rows.iter().filter_map(created).any(|t| t >= started_at) {
            out.push(Candidate {
                kind: NotificationKind::DecisionRequested,
                key: key.clone(),
                body: clamp_content(&body("")),
                project_id,
            });
        }
        // 24 時間後の再通知（1 回だけ: `reminder:<key>` の重複排除）。
        if oldest.is_some_and(|t| now - t >= DECISION_REMINDER_AFTER) {
            out.push(Candidate {
                kind: NotificationKind::DecisionRequested,
                key: format!("reminder:{key}"),
                body: clamp_content(&body("（24 時間未回答）")),
                project_id,
            });
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

/// ADR-0079 D8（Phase R3b）: root の計画が人の承認を待っている（`blocked(awaiting_plan_approval)`）。`key` =
/// `plan:<plan_id>:approval`（同じ task の replan の版がまた承認を待てば新しい key）。本文に段階の一覧・承認の理由と、
/// その計画の未回答の決定（`decision_line`、パンくずと推奨）を 1 行ずつ束ねる（決定は別に鳴らさない。`scan_decisions`）。
/// 状態で判定するので backfill の下限は使わない（`scan_phase_checkpoint` と同じ。重複排除で 1 回だけ鳴る）。
fn scan_plan_approvals(
    store: &dyn TaskStore,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    let filter = ListFilter {
        statuses: vec![Status::Blocked],
        ..ListFilter::default()
    };
    let page = store.list_page(&filter, ListOrder::CreatedDesc, None, TASK_SCAN)?;
    let mut out = Vec::new();
    for task in page.items {
        let events = store.events_for(task.id)?;
        let Some(info) = task_ops::plan_gate::latest_plan_approval(&task, &events) else {
            continue;
        };
        let stages = store
            .execution_plan_list(task.id)?
            .into_iter()
            .find(|p| p.id == info.plan_id)
            .map(|p| {
                p.spec
                    .stages
                    .iter()
                    .map(|s| excerpt(&s.title, EXCERPT_CHARS))
                    .collect::<Vec<_>>()
                    .join(" → ")
            })
            .unwrap_or_default();
        let root_id = task_core::tree::root_id_of(&task);
        let decisions: Vec<String> = store
            .decisions_list(Some(root_id))?
            .into_iter()
            .filter(|d| d.task_id == task.id && d.status == task_core::DecisionStatus::Open)
            .map(|d| decision_line(&d))
            .collect();
        let mut body = format!(
            "計画の承認が必要: 『{}』（{}）。理由: {}。承認 / replan / 取り下げ",
            excerpt(&task.title, EXCERPT_CHARS),
            stages,
            task_ops::plan_gate::describe_reasons(&info.reasons)
        );
        if !decisions.is_empty() {
            body.push_str(&format!("\n人の決定 {} 件:", decisions.len()));
            for (i, line) in decisions.iter().enumerate() {
                body.push_str(&format!("\n{}) {line}", i + 1));
            }
        }
        body.push_str(&link(base_url, &format!("/tasks/{}", task.id)));
        out.push(Candidate {
            kind: NotificationKind::PlanApproval,
            key: format!("plan:{}:approval", info.plan_id),
            body: clamp_content(&body),
            project_id: task.project_id,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

/// ADR-0074 D2.4（Phase F3 途中確認）: Task が工程の後の途中確認で止まっている（`blocked(awaiting_human)`）。
/// `scan_task_failed` と同じ形。`key` は `transition_key`（task id:遷移番号。同じ Task が次の工程の後で
/// また止まれば新しい key になる）。止まっている間ずっと判定に当たるが、重複排除で 1 回だけ鳴る。
/// 状態で判定するので backfill の下限（`started_at`）は使わない（`scan_question_blocked` と同じ）。
fn scan_phase_checkpoint(
    store: &dyn TaskStore,
    base_url: Option<&str>,
) -> Result<Vec<Candidate>, StoreError> {
    let filter = ListFilter {
        statuses: vec![Status::Blocked],
        ..ListFilter::default()
    };
    let page = store.list_page(&filter, ListOrder::CreatedDesc, None, TASK_SCAN)?;
    let mut out = Vec::new();
    for task in page.items {
        let events = store.events_for(task.id)?;
        let Some(info) = task_ops::phase_gate::latest_phase_checkpoint(&task, &events) else {
            continue;
        };
        let phase = if info.report.phase_title.is_empty() {
            info.report.phase.clone()
        } else {
            info.report.phase_title.clone()
        };
        out.push(Candidate {
            kind: NotificationKind::PhaseCheckpoint,
            key: format!("{}:{}", task.id, info.transition_seq),
            body: format!(
                "『{}』が工程『{}』まで進みました。続ける / replan / 取り下げ{}",
                excerpt(&task.title, EXCERPT_CHARS),
                excerpt(&phase, EXCERPT_CHARS),
                link(base_url, &format!("/tasks/{}", task.id))
            ),
            project_id: task.project_id,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

/// 判定して、まだ知らせていない `(kind, key)` を pending として登録する。作った行を返す
/// （**2 回目の tick では何も作らない**: 重複排除は `notifications` の `UNIQUE(kind, key)`）。
///
/// `started_at`（ADR-0037 D5）: celeris の起動時刻。backfill 禁止の基準（`scan` を見よ）。
pub fn schedule(
    store: &dyn TaskStore,
    config: &NotifyConfig,
    started_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<Vec<Notification>, StoreError> {
    let mut created = Vec::new();
    let since = store.notification_scan_at()?.unwrap_or(started_at);
    for candidate in scan_at(store, since, config.base_url(), now)? {
        if let Some(row) = store.notification_upsert_pending(
            candidate.kind,
            &candidate.key,
            &candidate.body,
            candidate.project_id,
            now,
        )? {
            created.push(row);
        }
    }
    store.notification_scan_mark(now)?;
    Ok(created)
}

/// 1 通に束ねた送信の材料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendBatch {
    pub ids: Vec<NotificationId>,
    pub content: String,
}

/// 1 tick で送る 1 通だけを選ぶ（ADR-0037 D3「1 tick に最大 1 通」/ D4「bad_news は束ねる」）。
/// `pending` は `notification_pending()`（古い順）。`bad_news` が 2 件以上あれば、それらをまとめて
/// 1 通にする（他の種が混ざっていても、その tick は bad_news を優先する）。それ以外は最古の 1 件だけ。
pub fn select_batch(pending: &[Notification]) -> Option<SendBatch> {
    let bad_news: Vec<&Notification> = pending
        .iter()
        .filter(|n| n.kind == NotificationKind::BadNews)
        .collect();
    if bad_news.len() >= 2 {
        let joined = bad_news
            .iter()
            .map(|n| {
                n.body
                    .strip_prefix(BAD_NEWS_PREFIX)
                    .unwrap_or(n.body.as_str())
            })
            .collect::<Vec<_>>()
            .join("／");
        let content = format!("悪い知らせ {} 件: {joined}", bad_news.len());
        return Some(SendBatch {
            ids: bad_news.iter().map(|n| n.id).collect(),
            content,
        });
    }
    pending.first().map(|n| SendBatch {
        ids: vec![n.id],
        content: n.body.clone(),
    })
}

// ---- 送信（ADR-0037 D3）----

/// ADR-0030 D1 の流儀で `[secrets] dir` から webhook の URL を読む。無ければ `None`
/// （**値もパスもログに出さない**。秘密が無いのは異常ではないので警告も出さない）。
pub fn webhook_url(secrets_dir: Option<&Path>, secret_id: &str) -> Option<String> {
    let dir = secrets_dir?;
    let text = std::fs::read_to_string(dir.join(secret_id)).ok()?;
    let value = text.trim_end_matches(['\n', '\r']).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// `reqwest::Error` の `Display` は URL を含むので使わない。種別だけの短い文にする。
fn safe_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".to_string()
    } else if e.is_connect() {
        "could not connect".to_string()
    } else if e.is_body() || e.is_decode() {
        "bad response body".to_string()
    } else if e.is_request() {
        "invalid request".to_string()
    } else {
        "request failed".to_string()
    }
}

/// `Retry-After` ヘッダ（秒。小数もある）か、無ければ Discord の JSON 本文の `retry_after`
/// （秒。ミリ秒ではない）を読む。どちらも無ければ `DEFAULT_RETRY_AFTER_SECS`。
async fn read_retry_after(response: reqwest::Response) -> Duration {
    let from_header = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok());
    if let Some(secs) = from_header {
        return Duration::from_secs_f64(secs.max(0.0));
    }
    let body = response.text().await.unwrap_or_default();
    let from_body = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("retry_after").and_then(|r| r.as_f64()));
    match from_body {
        Some(secs) => Duration::from_secs_f64(secs.max(0.0)),
        None => Duration::from_secs(DEFAULT_RETRY_AFTER_SECS),
    }
}

/// webhook の URL へ 1 通 POST する。返すのは**種別だけ**の短いエラー（URL・ホスト名は入らない）。
/// 429 は `SendOutcome::RateLimited`（失敗ではない。ADR-0037 D3）。
pub async fn post_webhook(client: &reqwest::Client, url: &str, content: &str) -> SendOutcome {
    let payload = serde_json::json!({
        "content": clamp_content(content),
        "username": WEBHOOK_USERNAME,
    });
    let response = match client
        .post(url)
        .json(&payload)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return SendOutcome::Failed(safe_error(&e)),
    };
    let status = response.status();
    if status.is_success() {
        SendOutcome::Sent
    } else if status.as_u16() == 429 {
        SendOutcome::RateLimited(read_retry_after(response).await)
    } else {
        SendOutcome::Failed(format!("http status {}", status.as_u16()))
    }
}

/// tick をブロックせずに 1 通送る（B1。`bad_news` の束ねなら複数 id）。結果は `tx` に流れ、
/// **次の tick** で `notification_mark` される。
pub fn spawn_send(
    client: reqwest::Client,
    url: String,
    batch: &SendBatch,
    tx: tokio::sync::mpsc::Sender<SendResult>,
) {
    let ids = batch.ids.clone();
    let content = batch.content.clone();
    tokio::spawn(async move {
        let outcome = post_webhook(&client, &url, &content).await;
        let _ = tx.send(SendResult { ids, outcome }).await;
    });
}

/// 送信の結果を台帳に書く（`tick_loop` が次の tick の先頭で呼ぶ）。3 回目の失敗で諦める（`ok = false`）。
/// `RateLimited` は台帳に触れない（**attempts に数えない**。次の tick でそのまま再挑戦する）。
pub fn record(
    store: &dyn TaskStore,
    pending: &[Notification],
    result: &SendResult,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    match &result.outcome {
        SendOutcome::Sent => {
            for id in &result.ids {
                store.notification_mark(*id, Some(true), None, now)?;
            }
        }
        SendOutcome::RateLimited(_) => {}
        SendOutcome::Failed(error) => {
            for id in &result.ids {
                // 直前に読んだ pending の `attempts` で「これが最後の試行か」を決める（決定的）。
                let attempts_before = pending
                    .iter()
                    .find(|n| n.id == *id)
                    .map(|n| n.attempts)
                    .unwrap_or(0);
                let give_up = attempts_before + 1 >= MAX_NOTIFY_ATTEMPTS;
                store.notification_mark(*id, None, Some(error), now)?;
                if give_up {
                    let reason = format!("gave up after {MAX_NOTIFY_ATTEMPTS} attempts: {error}");
                    store.notification_mark(*id, Some(false), Some(&reason), now)?;
                }
            }
        }
    }
    Ok(())
}

/// `POST /notify/test` で送る定型文（ADR-0037 D4）。
pub const TEST_CONTENT: &str =
    "celeris のテスト送信です。ここに「人の判断が要るとき」だけ通知が届きます。";

/// `POST /notify/test` の結果（ADR-0037 D4）。**どの枝にも URL・ホスト名は入らない**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestSend {
    /// 送れた。
    Sent,
    /// 送り先には届いたが失敗した（種別だけの短い理由）。
    Failed(String),
    /// 秘密が無い・HTTP クライアントが無い（API は 409 `notify_unavailable`）。
    NotConfigured(String),
}

/// テスト送信 1 回（`AdminRequest::NotifyTest` の中身）。秘密を読むのも POST するのも celeris 側。
pub async fn send_test(
    client: Option<&reqwest::Client>,
    secrets_dir: Option<&Path>,
    secret_id: &str,
) -> TestSend {
    let Some(client) = client else {
        return TestSend::NotConfigured(
            "the HTTP client could not be created, so nothing can be sent".to_string(),
        );
    };
    let Some(url) = webhook_url(secrets_dir, secret_id) else {
        return TestSend::NotConfigured(format!(
            "no Discord webhook is registered; add it as the secret `{secret_id}`"
        ));
    };
    match post_webhook(client, &url, TEST_CONTENT).await {
        SendOutcome::Sent => TestSend::Sent,
        SendOutcome::RateLimited(_) => TestSend::Failed("rate limited".to_string()),
        SendOutcome::Failed(detail) => TestSend::Failed(detail),
    }
}

/// 秘密が無いときに pending を畳む理由（GUI の「直近の送信」に出る。URL は含まない）。
pub const NOT_CONFIGURED: &str = "discord webhook is not configured";

/// 秘密が無い間は**送らず、pending も溜めない**（ADR-0037 D2）。判定はしたが送れなかったことを
/// 台帳に残して畳む（後から秘密を登録しても、その間の出来事は蒸し返さない）。
pub fn discard_pending(
    store: &dyn TaskStore,
    pending: &[Notification],
    now: OffsetDateTime,
) -> Result<usize, StoreError> {
    let mut discarded = 0;
    for row in pending {
        if store.notification_mark(row.id, Some(false), Some(NOT_CONFIGURED), now)? {
            discarded += 1;
        }
    }
    Ok(discarded)
}

/// 10 秒のタイムアウトを持つ HTTP クライアント（rustls。ADR-0037 D3）。組み立てに失敗したら
/// `None`（通知だけが止まり、celeris は動き続ける）。
pub fn client() -> Option<reqwest::Client> {
    match reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("celeris/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => Some(client),
        Err(e) => {
            tracing::warn!(error = %e, "notify: cannot build the HTTP client; notifications are disabled");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0074 §6 F3 (c)（途中確認）: 工程の後の途中確認（`blocked(awaiting_human)`）は
    /// `PhaseCheckpoint` として 1 回だけ鳴り、`QuestionBlocked` は鳴らない。質問で止まった Task は
    /// 従来どおり `QuestionBlocked` だけ。
    #[test]
    fn phase_checkpoint_is_not_a_question() {
        use task_core::{
            Budget, Check, Criterion, SqliteStore, Task, TaskId, TaskKind, Tier, Trigger,
            WorkerHint, WorkspaceSpec,
        };
        fn task(title: &str) -> Task {
            let now = OffsetDateTime::now_utc();
            Task {
                tree: None,
                paused_at: None,
                routing: None,
                mode: Default::default(),
                skills: Vec::new(),
                repos: Vec::new(),
                id: TaskId::new(),
                parent_id: None,
                kind: TaskKind::Execute,
                title: title.into(),
                objective: "o".into(),
                acceptance: vec![Criterion {
                    text: "c".into(),
                    check: Check::Human,
                }],
                inputs: vec![],
                depends_on: vec![],
                status: Status::Ready,
                priority: 0,
                worker_hint: WorkerHint {
                    tier: Tier::Standard,
                    adapter: None,
                },
                workspace: WorkspaceSpec::Local {
                    path: "ws".into(),
                    mode: None,
                },
                budget: Budget {
                    max_turns: 1,
                    max_wall_secs: 1,
                    max_retries: 0,
                },
                attempts: 0,
                lease: None,
                created_at: now,
                updated_at: now,
                role: None,
                genre: None,
                aggregate: false,
                project_id: None,
                milestone_id: None,
                assignee: None,
                conversation: None,
                labels: Vec::new(),
                category: Default::default(),
            }
        }
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let store = SqliteStore::open(&dir.path().join("celeris.db"))
            .unwrap_or_else(|e| panic!("open: {e}"));
        let paused = task("二工程の仕事");
        store.insert(&paused).unwrap_or_else(|e| panic!("{e}"));
        store
            .apply_transition(paused.id, Trigger::Dispatch, None)
            .unwrap_or_else(|e| panic!("{e}"));
        store
            .apply_transition_with_events(
                paused.id,
                Trigger::PhaseGate {
                    phase: "design".into(),
                },
                vec![task_core::Event::PhaseReported {
                    phase: "design".into(),
                    report: Box::new(task_core::PhaseReport {
                        phase: "design".into(),
                        phase_title: "設計".into(),
                        next_phase: Some("build".into()),
                        ..Default::default()
                    }),
                }],
            )
            .unwrap_or_else(|e| panic!("{e}"));
        let asked = task("質問の仕事");
        store.insert(&asked).unwrap_or_else(|e| panic!("{e}"));
        store
            .apply_transition(asked.id, Trigger::Dispatch, None)
            .unwrap_or_else(|e| panic!("{e}"));
        store
            .apply_transition(asked.id, Trigger::WorkerQuestion, None)
            .unwrap_or_else(|e| panic!("{e}"));

        let started = OffsetDateTime::now_utc() - time::Duration::hours(1);
        let schedule = || {
            schedule(&store, &NotifyConfig::default(), started, started)
                .unwrap_or_else(|e| panic!("schedule: {e}"))
        };
        let first = schedule();
        let checkpoints: Vec<&Notification> = first
            .iter()
            .filter(|n| n.kind == NotificationKind::PhaseCheckpoint)
            .collect();
        assert_eq!(checkpoints.len(), 1, "{first:?}");
        assert!(
            checkpoints[0].key.starts_with(&paused.id.to_string()),
            "{:?}",
            checkpoints[0]
        );
        assert!(
            checkpoints[0].body.contains(
                "『二工程の仕事』が工程『設計』まで進みました。続ける / replan / 取り下げ"
            ),
            "{}",
            checkpoints[0].body
        );
        let questions: Vec<&Notification> = first
            .iter()
            .filter(|n| n.kind == NotificationKind::QuestionBlocked)
            .collect();
        assert_eq!(questions.len(), 1, "{first:?}");
        assert!(
            questions[0].key.starts_with(&asked.id.to_string()),
            "途中確認の Task は QuestionBlocked を鳴らさない: {questions:?}"
        );
        let second = schedule();
        assert!(
            !second
                .iter()
                .any(|n| n.kind == NotificationKind::PhaseCheckpoint),
            "2 回目の tick では鳴らない: {second:?}"
        );
    }

    #[test]
    fn excerpt_cuts_by_characters_and_flattens_newlines() {
        assert_eq!(excerpt("あいうえお", 3), "あいう…");
        assert_eq!(excerpt("あいう", 3), "あいう");
        assert_eq!(excerpt("a\n b", 10), "a b");
    }

    #[test]
    fn links_are_omitted_without_a_base_url() {
        assert_eq!(link(None, "/approvals"), "");
        assert_eq!(
            link(Some("http://h:7700"), "/approvals"),
            "\nhttp://h:7700/approvals"
        );
    }

    #[test]
    fn base_url_drops_the_trailing_slash_and_treats_empty_as_absent() {
        let config = NotifyConfig {
            gui_base_url: Some("http://h:7700/".into()),
            ..NotifyConfig::default()
        };
        assert_eq!(config.base_url(), Some("http://h:7700"));
        let empty = NotifyConfig {
            gui_base_url: Some(String::new()),
            ..NotifyConfig::default()
        };
        assert_eq!(empty.base_url(), None);
        assert_eq!(NotifyConfig::default().base_url(), None);
    }

    #[test]
    fn defaults_match_the_adr() {
        let config = NotifyConfig::default();
        assert_eq!(config.discord_webhook_secret, "discord-webhook");
        assert_eq!(config.interval_secs, 30);
        assert!(config.gui_base_url.is_none());
    }

    #[test]
    fn content_is_clamped_to_one_discord_message() {
        let long = "あ".repeat(5_000);
        assert_eq!(clamp_content(&long).chars().count(), CONTENT_MAX_CHARS);
    }

    #[test]
    fn webhook_url_is_none_without_a_secrets_dir_or_file() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        assert_eq!(webhook_url(None, "discord-webhook"), None);
        assert_eq!(webhook_url(Some(dir.path()), "discord-webhook"), None);
        std::fs::write(
            dir.path().join("discord-webhook"),
            "https://example.invalid/hook\n",
        )
        .unwrap_or_else(|e| panic!("write: {e}"));
        assert_eq!(
            webhook_url(Some(dir.path()), "discord-webhook").as_deref(),
            Some("https://example.invalid/hook")
        );
        // 空の秘密は「無い」と同じ扱い。
        std::fs::write(dir.path().join("empty"), "\n").unwrap_or_else(|e| panic!("write: {e}"));
        assert_eq!(webhook_url(Some(dir.path()), "empty"), None);
    }
}
