//! ADR-0133 D1/D3: 判断を要しない出来事だけをアプリ内通知へ同期する。
//! 冪等性と束ねは `NoticeStore::notice_record` が一つの transaction で保証する。
use task_core::{
    Event, NoticeEvent, NoticeKind, NoticeLink, NoticeRecordOutcome, NoticeStore, NoticeTarget,
    ReportFilter, ReportKind, ReportStore, Status, StoreError, TaskStore,
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
            added += record(store, &notice)?;
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

/// store の events・reports・対話・delivery 状態を走査して通知を作る。
/// `now` より後の出来事は次の走査に残す。戻り値は新規記録した出来事の件数。
/// 受信箱へ分類された Event はここでは無視する。
pub fn sync_notifications<S>(store: &S, now: OffsetDateTime) -> Result<u64, StoreError>
where
    S: TaskStore + ReportStore + NoticeStore + ?Sized,
{
    let mut added = 0;
    let tasks = store.list(None)?;
    let by_id: std::collections::HashMap<_, _> = tasks.iter().map(|t| (t.id, t)).collect();
    let mut cursor = store
        .feed_cursor_get("events")?
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    'events: loop {
        let rows = store.events_since(cursor, EVENT_BATCH)?;
        if rows.is_empty() {
            break;
        }
        let len = rows.len();
        for row in rows {
            let at = OffsetDateTime::parse(&row.ts, &Rfc3339)
                .map_err(|e| StoreError::Invalid(format!("invalid event timestamp: {e}")))?;
            if at > now {
                // `events` は追記順であり時刻順ではない。未来の行だけ飛ばすと
                // 二度と拾えないので、この行で走査を止める。
                break 'events;
            }
            if let (
                Some(task),
                Event::Transitioned {
                    to: Status::Done, ..
                },
            ) = (by_id.get(&row.task_id), &row.event)
                && task.parent_id.is_none()
                && !task_core::tree::is_tree_child(task)
                && task_core::support_kind(task).is_none()
            {
                let project_id = task.project_id.map(|p| p.to_string());
                let notice = event(
                    format!("event:{}", row.id),
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
                added += record(store, &notice)?;
            }
            cursor = row.id;
            store.feed_cursor_set("events", &cursor.to_string())?;
        }
        if len < EVENT_BATCH {
            break;
        }
    }

    // source_key が既読後も残るため、報告を再走査しても再通知しない。
    // 報告一覧は新しい順の上限つきなので、過去の大量データを走査しない。
    for report in store.report_list(&ReportFilter {
        level: Some(0),
        limit: 10_000,
        ..Default::default()
    })? {
        if report.created_at > now {
            continue;
        }
        let kind = match report.kind {
            ReportKind::Progress | ReportKind::Result => NoticeKind::Report,
            ReportKind::BadNews => NoticeKind::BadNews,
            ReportKind::Question | ReportKind::Proposal => continue,
        };
        let project_id = report.project_id.map(|p| p.to_string());
        let notice = event(
            format!("report:{}", report.id),
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
        added += record(store, &notice)?;
    }

    // 既存の secretary_reply と同じく、対話の Node 発言のみを対象にする。
    // delivery の機械的な引き渡しメッセージは除外する。
    for message in store.message_page(None, None, None, 10_000)? {
        if message.role != task_core::MessageRole::Node || message.created_at > now {
            continue;
        }
        if let Some(task_id) = message.task_id
            && let Some(delivery) = store.delivery_get(task_id)?
            && (delivery.notification == Some(message.id)
                || message.run_id.as_deref() == Some(delivery.review_run.as_str()))
        {
            continue;
        }
        let project_id = message.project_id.map(|p| p.to_string());
        let notice = event(
            format!("message:{}", message.id),
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
        added += record(store, &notice)?;
    }

    for delivery in store.delivery_list()? {
        if let Some(release) = &delivery.release
            && let Some(task) = by_id.get(&delivery.task_id)
            && task.updated_at <= now
        {
            let notice = event(
                format!("release:created:{release}"),
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
            added += record(store, &notice)?;
        }
        if !matches!(
            delivery.state,
            task_core::delivery::DeliveryState::Ready | task_core::delivery::DeliveryState::Blocked
        ) {
            continue;
        }
        let Some(task) = by_id.get(&delivery.task_id) else {
            continue;
        };
        if task.updated_at > now {
            continue;
        }
        let notice = event(
            format!(
                "delivery:{}:{}:{:?}",
                delivery.task_id, delivery.head, delivery.state
            ),
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
        added += record(store, &notice)?;
    }
    Ok(added)
}

fn record(store: &(impl NoticeStore + ?Sized), notice: &NoticeEvent) -> Result<u64, StoreError> {
    Ok(u64::from(!matches!(
        store.notice_record(notice)?,
        NoticeRecordOutcome::Duplicate(_)
    )))
}

#[cfg(test)]
mod tests;
