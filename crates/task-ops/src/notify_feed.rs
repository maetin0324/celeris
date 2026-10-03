//! ADR-0133 D1/D3: 判断を要しない出来事だけをアプリ内通知へ同期する。
//! 冪等性と束ねは `NoticeStore::notice_record` が一つの transaction で保証する。
use task_core::{
    Event, NoticeEvent, NoticeKind, NoticeLink, NoticeRecordOutcome, NoticeStore, NoticeTarget,
    ReportKind, ReportStore, Status, StoreError, TaskStore,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const EVENT_BATCH: usize = 512;

/// 別領域で確定した結果。`auto_recovered` の入力は ADR-0131 の
/// `attention_suppression` が R1/R4 を返したものだけにする。
/// ここではその片付け規則を再判定しない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedObservation {
    Release {
        id: String,
        summary: String,
        at: OffsetDateTime,
    },
    CronRun {
        run_id: String,
        job_id: String,
        summary: String,
        at: OffsetDateTime,
    },
    AutoRecovered {
        task_id: String,
        root_id: String,
        summary: String,
        at: OffsetDateTime,
    },
    RequeueLimitNear {
        task_id: String,
        summary: String,
        at: OffsetDateTime,
    },
}

/// release ファイルの観測・cron 履歴・ADR-0131 の抑制結果など、`TaskStore` の
/// event 以外から確定した通知を同じ冪等な store へ渡す入口。
pub fn record_observations(
    store: &impl NoticeStore,
    now: OffsetDateTime,
    observations: &[FeedObservation],
) -> Result<u64, StoreError> {
    let mut added = 0;
    for source in observations {
        let notice = match source {
            FeedObservation::Release { id, summary, at } => event(
                format!("release:{id}"),
                NoticeKind::Release,
                "",
                summary.clone(),
                None,
                None,
                Some(NoticeTarget {
                    kind: "release".into(),
                    id: id.clone(),
                }),
                Vec::new(),
                *at,
            ),
            FeedObservation::CronRun {
                run_id,
                job_id,
                summary,
                at,
            } => event(
                format!("cron_run:{run_id}"),
                NoticeKind::CronRun,
                job_id,
                summary.clone(),
                None,
                None,
                Some(NoticeTarget {
                    kind: "cron_job".into(),
                    id: job_id.clone(),
                }),
                Vec::new(),
                *at,
            ),
            FeedObservation::AutoRecovered {
                task_id,
                root_id,
                summary,
                at,
            } => event(
                format!("auto_recovered:{task_id}"),
                NoticeKind::AutoRecovered,
                &format!("root:{root_id}"),
                summary.clone(),
                None,
                Some(task_id.clone()),
                Some(NoticeTarget {
                    kind: "task".into(),
                    id: task_id.clone(),
                }),
                vec![NoticeLink {
                    label: "タスク".into(),
                    href: format!("/tasks/{task_id}"),
                }],
                *at,
            ),
            FeedObservation::RequeueLimitNear {
                task_id,
                summary,
                at,
            } => event(
                format!("requeue_limit_near:{task_id}"),
                NoticeKind::RequeueLimitNear,
                "",
                summary.clone(),
                None,
                Some(task_id.clone()),
                Some(NoticeTarget {
                    kind: "task".into(),
                    id: task_id.clone(),
                }),
                vec![NoticeLink {
                    label: "タスク".into(),
                    href: format!("/tasks/{task_id}"),
                }],
                *at,
            ),
        };
        if notice.at <= now {
            let mut stats = FeedSyncStats::default();
            record(store, &notice, &mut stats)?;
            added += stats.recorded;
        }
    }
    Ok(added)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one argument per stored notice field"
)]
fn event(
    source_key: String,
    kind: NoticeKind,
    scope: &str,
    title: String,
    project_id: Option<String>,
    task_id: Option<String>,
    target: Option<NoticeTarget>,
    links: Vec<NoticeLink>,
    at: OffsetDateTime,
) -> NoticeEvent {
    NoticeEvent {
        source_key,
        kind,
        group_key: if scope.is_empty() {
            kind.as_str().to_string()
        } else {
            format!("{}:{scope}", kind.as_str())
        },
        summary: title.clone(),
        title,
        project_id,
        task_id,
        target,
        links,
        at,
    }
}

/// store の events・reports・対話・delivery 状態のうち前回より後の分だけを読んで通知を作る。
/// `now` より後の出来事は次の走査に残す。戻り値は新規記録した出来事の件数。
/// 受信箱へ分類された Event はここでは無視する。
pub fn sync_notifications<S>(store: &S, now: OffsetDateTime) -> Result<u64, StoreError>
where
    S: TaskStore + ReportStore + NoticeStore + ?Sized,
{
    Ok(sync_notifications_counted(store, now)?.recorded)
}

/// `sync_notifications` 1 回で読んだ行と書き込みの回数（ADR-0133 付記の回帰試験が固定する）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeedSyncStats {
    /// 読んだ `events` の行（`EVENT_BUDGET` 以下）。
    pub events_scanned: u64,
    /// 読んだ報告（level 0、前回の位置以後）。
    pub reports_scanned: u64,
    /// 読んだ対話の発言（前回の位置以後）。
    pub messages_scanned: u64,
    /// 読んだ delivery の行。
    pub deliveries_scanned: u64,
    /// `notice_record` を呼んだ回数（書き込みの transaction の数）。記録済みの出来事には呼ばない。
    pub record_calls: u64,
    /// 新しく数えた出来事。
    pub recorded: u64,
}

/// ADR-0133 付記: 1 回の同期で読む `events` の上限。tick の中で events 全体を読まない
/// （残りは次の tick が続きの位置から読む）。
pub const EVENT_BUDGET: usize = 4 * EVENT_BATCH;
/// 1 回の同期で読む報告・発言の上限。
pub const SOURCE_PAGE: usize = 256;

const CURSOR_EVENTS: &str = "events";
const CURSOR_REPORTS: &str = "reports";
const CURSOR_MESSAGES: &str = "messages";

/// ADR-0133 付記: 差分同期の本体。走査位置は `feed_cursor`（events は `id`、報告・発言は
/// `created_at`）に持ち、記録済みかどうかは読み取り接続で先に一括して確かめる。
/// 書き込み（`notice_record`・走査位置の更新）は新しい出来事と位置が進んだときだけ。
pub fn sync_notifications_counted<S>(
    store: &S,
    now: OffsetDateTime,
) -> Result<FeedSyncStats, StoreError>
where
    S: TaskStore + ReportStore + NoticeStore + ?Sized,
{
    let mut stats = FeedSyncStats::default();
    let mut tasks = TaskCache::default();
    sync_events(store, now, &mut tasks, &mut stats)?;
    sync_reports(store, now, &mut stats)?;
    let deliveries = store.delivery_list()?;
    stats.deliveries_scanned = deliveries.len() as u64;
    sync_messages(store, now, &deliveries, &mut stats)?;
    sync_deliveries(store, now, &deliveries, &mut tasks, &mut stats)?;
    Ok(stats)
}

/// 同期 1 回の間だけ持つ task の写し（必要な task だけを 1 回ずつ読む）。
#[derive(Default)]
struct TaskCache(std::collections::HashMap<task_core::TaskId, Option<task_core::Task>>);

impl TaskCache {
    fn get<S: TaskStore + ?Sized>(
        &mut self,
        store: &S,
        id: task_core::TaskId,
    ) -> Result<Option<&task_core::Task>, StoreError> {
        let task = match self.0.entry(id) {
            std::collections::hash_map::Entry::Occupied(slot) => slot.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => slot.insert(store.get(id)?),
        };
        Ok(task.as_ref())
    }
}

fn sync_events<S>(
    store: &S,
    now: OffsetDateTime,
    tasks: &mut TaskCache,
    stats: &mut FeedSyncStats,
) -> Result<(), StoreError>
where
    S: TaskStore + NoticeStore + ?Sized,
{
    let start = store
        .feed_cursor_get(CURSOR_EVENTS)?
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let mut cursor = start;
    let mut done = Vec::new();
    'events: while (stats.events_scanned as usize) < EVENT_BUDGET {
        let rows = store.events_since(cursor, EVENT_BATCH)?;
        let len = rows.len();
        for row in rows {
            let at = OffsetDateTime::parse(&row.ts, &Rfc3339)
                .map_err(|e| StoreError::Invalid(format!("invalid event timestamp: {e}")))?;
            if at > now {
                // `events` は追記順であり時刻順ではない。未来の行だけ飛ばすと
                // 二度と拾えないので、この行で走査を止める。
                break 'events;
            }
            stats.events_scanned += 1;
            if matches!(
                row.event,
                Event::Transitioned {
                    to: Status::Done,
                    ..
                }
            ) {
                done.push((row.id, row.task_id, at));
            }
            cursor = row.id;
        }
        if len < EVENT_BATCH {
            break;
        }
    }
    let keys: Vec<String> = done.iter().map(|(id, ..)| format!("event:{id}")).collect();
    let known = store.notice_sources_known(&keys)?;
    for ((_, task_id, at), key) in done.into_iter().zip(keys) {
        if known.contains(&key) {
            continue;
        }
        let Some(task) = tasks.get(store, task_id)? else {
            continue;
        };
        if task.parent_id.is_some()
            || task_core::tree::is_tree_child(task)
            || task_core::support_kind(task).is_some()
        {
            continue;
        }
        let project_id = task.project_id.map(|p| p.to_string());
        let notice = event(
            key,
            NoticeKind::TaskDone,
            &format!("project:{}", project_id.as_deref().unwrap_or("none")),
            format!("{} が完了しました", task.title),
            project_id,
            Some(task.id.to_string()),
            Some(NoticeTarget {
                kind: "task".into(),
                id: task.id.to_string(),
            }),
            vec![NoticeLink {
                label: "タスク".into(),
                href: format!("/tasks/{}", task.id),
            }],
            at,
        );
        record(store, &notice, stats)?;
    }
    // 位置は通知を記録した後に 1 回だけ進める（途中で落ちても記録は冪等なので読み直してよい）。
    if cursor != start {
        store.feed_cursor_set(CURSOR_EVENTS, &cursor.to_string())?;
    }
    Ok(())
}

fn cursor_time(
    store: &(impl NoticeStore + ?Sized),
    name: &str,
) -> Result<Option<OffsetDateTime>, StoreError> {
    Ok(store
        .feed_cursor_get(name)?
        .and_then(|s| OffsetDateTime::parse(&s, &Rfc3339).ok()))
}

fn set_cursor_time(
    store: &(impl NoticeStore + ?Sized),
    name: &str,
    previous: Option<OffsetDateTime>,
    next: Option<OffsetDateTime>,
) -> Result<(), StoreError> {
    match next {
        Some(next) if Some(next) != previous => {
            let value = next
                .format(&Rfc3339)
                .map_err(|e| StoreError::Invalid(format!("feed cursor time: {e}")))?;
            store.feed_cursor_set(name, &value)
        }
        _ => Ok(()),
    }
}

fn sync_reports<S>(
    store: &S,
    now: OffsetDateTime,
    stats: &mut FeedSyncStats,
) -> Result<(), StoreError>
where
    S: ReportStore + NoticeStore + ?Sized,
{
    // 位置は閉区間で読み、同じ時刻の報告は `source_key` の既知判定で飛ばす。
    let since = cursor_time(store, CURSOR_REPORTS)?;
    let mut next = since;
    let mut fresh = Vec::new();
    for report in store.report_page_since(0, since, SOURCE_PAGE)? {
        if report.created_at > now {
            break;
        }
        stats.reports_scanned += 1;
        next = Some(report.created_at);
        fresh.push(report);
    }
    let keys: Vec<String> = fresh.iter().map(|r| format!("report:{}", r.id)).collect();
    let known = store.notice_sources_known(&keys)?;
    for (report, key) in fresh.into_iter().zip(keys) {
        if known.contains(&key) {
            continue;
        }
        let kind = match report.kind {
            ReportKind::Progress | ReportKind::Result => NoticeKind::Report,
            ReportKind::BadNews => NoticeKind::BadNews,
            ReportKind::Question | ReportKind::Proposal => continue,
        };
        let project_id = report.project_id.map(|p| p.to_string());
        let notice = event(
            key,
            kind,
            &format!("project:{}", project_id.as_deref().unwrap_or("none")),
            report.headline.clone(),
            project_id,
            report.task_id.map(|id| id.to_string()),
            Some(NoticeTarget {
                kind: "report".into(),
                id: report.id.to_string(),
            }),
            vec![NoticeLink {
                label: "報告".into(),
                href: format!("/reports/{}", report.id),
            }],
            report.created_at,
        );
        record(store, &notice, stats)?;
    }
    set_cursor_time(store, CURSOR_REPORTS, since, next)
}

fn sync_messages<S>(
    store: &S,
    now: OffsetDateTime,
    deliveries: &[task_core::Delivery],
    stats: &mut FeedSyncStats,
) -> Result<(), StoreError>
where
    S: TaskStore + NoticeStore + ?Sized,
{
    let since = cursor_time(store, CURSOR_MESSAGES)?;
    // `message_page` の `after` は文字列の比較なので、小数秒の桁の違いで取りこぼさないよう 1 秒前から読み、
    // 位置より前の行は捨てる。位置が無ければ最初から（`after` 無しは新しい側の 1 頁になるので空文字を渡す）。
    let after = since
        .map(|t| (t - time::Duration::seconds(1)).format(&Rfc3339))
        .transpose()
        .map_err(|e| StoreError::Invalid(format!("feed cursor time: {e}")))?
        .unwrap_or_default();
    let mut next = since;
    let mut fresh = Vec::new();
    for message in store.message_page(None, None, Some(&after), SOURCE_PAGE)? {
        if message.created_at > now {
            break;
        }
        if since.is_some_and(|s| message.created_at < s) {
            continue;
        }
        stats.messages_scanned += 1;
        next = Some(message.created_at);
        // 既存の secretary_reply と同じく、対話の Node 発言のみを対象にする。
        if message.role == task_core::MessageRole::Node {
            fresh.push(message);
        }
    }
    let keys: Vec<String> = fresh.iter().map(|m| format!("message:{}", m.id)).collect();
    let known = store.notice_sources_known(&keys)?;
    for (message, key) in fresh.into_iter().zip(keys) {
        if known.contains(&key) {
            continue;
        }
        // delivery の機械的な引き渡しメッセージは除外する。
        if let Some(task_id) = message.task_id
            && deliveries.iter().any(|delivery| {
                delivery.task_id == task_id
                    && (delivery.notification == Some(message.id)
                        || message.run_id.as_deref() == Some(delivery.review_run.as_str()))
            })
        {
            continue;
        }
        let project_id = message.project_id.map(|p| p.to_string());
        let notice = event(
            key,
            NoticeKind::SecretaryReply,
            &format!("project:{}", project_id.as_deref().unwrap_or("none")),
            "返事が届きました".into(),
            project_id,
            message.task_id.map(|id| id.to_string()),
            Some(NoticeTarget {
                kind: "message".into(),
                id: message.id.to_string(),
            }),
            Vec::new(),
            message.created_at,
        );
        record(store, &notice, stats)?;
    }
    set_cursor_time(store, CURSOR_MESSAGES, since, next)
}

fn sync_deliveries<S>(
    store: &S,
    now: OffsetDateTime,
    deliveries: &[task_core::Delivery],
    tasks: &mut TaskCache,
    stats: &mut FeedSyncStats,
) -> Result<(), StoreError>
where
    S: TaskStore + NoticeStore + ?Sized,
{
    // delivery は状態が変わるので位置を持たない。代わりに key を先に作り、記録済みなら task も読まない。
    let mut keys = Vec::new();
    for delivery in deliveries {
        if let Some(release) = &delivery.release {
            keys.push(format!("release:created:{release}"));
        }
        if matches!(
            delivery.state,
            task_core::delivery::DeliveryState::Ready | task_core::delivery::DeliveryState::Blocked
        ) {
            keys.push(delivery_key(delivery));
        }
    }
    let known = store.notice_sources_known(&keys)?;
    for delivery in deliveries {
        if let Some(release) = &delivery.release {
            let key = format!("release:created:{release}");
            if !known.contains(&key)
                && let Some(task) = tasks.get(store, delivery.task_id)?
                && task.updated_at <= now
            {
                let notice = event(
                    key,
                    NoticeKind::Release,
                    "",
                    format!("リリース {release} を作成しました"),
                    None,
                    Some(task.id.to_string()),
                    Some(NoticeTarget {
                        kind: "release".into(),
                        id: release.clone(),
                    }),
                    Vec::new(),
                    task.updated_at,
                );
                record(store, &notice, stats)?;
            }
        }
        if !matches!(
            delivery.state,
            task_core::delivery::DeliveryState::Ready | task_core::delivery::DeliveryState::Blocked
        ) {
            continue;
        }
        let key = delivery_key(delivery);
        if known.contains(&key) {
            continue;
        }
        let Some(task) = tasks.get(store, delivery.task_id)? else {
            continue;
        };
        if task.updated_at > now {
            continue;
        }
        let notice = event(
            key,
            NoticeKind::Delivery,
            &format!("project:{}", delivery.project_id),
            format!("{} の配送結果: {:?}", task.title, delivery.state),
            Some(delivery.project_id.to_string()),
            Some(task.id.to_string()),
            Some(NoticeTarget {
                kind: "delivery".into(),
                id: task.id.to_string(),
            }),
            vec![NoticeLink {
                label: "タスク".into(),
                href: format!("/tasks/{}", task.id),
            }],
            task.updated_at,
        );
        record(store, &notice, stats)?;
    }
    Ok(())
}

fn delivery_key(delivery: &task_core::Delivery) -> String {
    format!(
        "delivery:{}:{}:{:?}",
        delivery.task_id, delivery.head, delivery.state
    )
}

fn record(
    store: &(impl NoticeStore + ?Sized),
    notice: &NoticeEvent,
    stats: &mut FeedSyncStats,
) -> Result<(), StoreError> {
    stats.record_calls += 1;
    stats.recorded += u64::from(!matches!(
        store.notice_record(notice)?,
        NoticeRecordOutcome::Duplicate(_)
    ));
    Ok(())
}

#[cfg(test)]
mod tests;
