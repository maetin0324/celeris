//! ADR-0033 D3（Phase 25）: `GET /reports`・`GET /reports/{id}`・`GET /daemon` の未読の件数。
//!
//! 見るもの: 一覧の絞り込みと並び、`sources` の展開、管理系の 401（トークン未設定の構成でも）、
//! 404、`GET /daemon` に載る未読の件数と通知の判定。

mod common;

use common::*;
use task_core::report::{Report, ReportId, ReportKind};
use task_core::{ProjectId, SqliteStore};
use time::OffsetDateTime;

fn auth_header() -> String {
    format!("Bearer {TOKEN}")
}

fn g(path: &str) -> axum::http::Request<axum::body::Body> {
    get_with(path, &[("authorization", auth_header().as_str())])
}

#[allow(clippy::too_many_arguments)]
fn report(
    store: &SqliteStore,
    node_id: &str,
    level: u32,
    kind: ReportKind,
    project: Option<ProjectId>,
    headline: &str,
    at: OffsetDateTime,
    sources: Vec<ReportId>,
) -> Report {
    let report = Report {
        id: ReportId::new(),
        project_id: project,
        node_id: node_id.into(),
        task_id: None,
        kind,
        level,
        headline: headline.into(),
        body: format!("{headline} の本文"),
        sources,
        read_at: None,
        created_at: at,
    };
    store.report_append(&report).expect("append");
    report
}

#[tokio::test]
async fn reports_are_listed_newest_first_and_can_be_filtered() {
    let env = admin_env();
    let app = env.router();
    let now = OffsetDateTime::now_utc();
    let project = ProjectId::new();
    let other = ProjectId::new();

    let old = report(
        &env.store,
        "coding-poc",
        2,
        ReportKind::Result,
        Some(project),
        "古い結果",
        now - time::Duration::hours(2),
        vec![],
    );
    let new = report(
        &env.store,
        "coding-poc",
        2,
        ReportKind::Result,
        Some(project),
        "新しい結果",
        now - time::Duration::minutes(1),
        vec![],
    );
    let top = report(
        &env.store,
        "secretary",
        0,
        ReportKind::BadNews,
        Some(other),
        "落ちました",
        now - time::Duration::minutes(30),
        vec![old.id],
    );

    let resp = send(&app, g("/api/v1/reports")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let items = resp.json()["items"].as_array().cloned().expect("items");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["headline"], "新しい結果", "新しい順");
    assert_eq!(items[2]["headline"], "古い結果");

    // 案件で絞る。
    let resp = send(&app, g(&format!("/api/v1/reports?project={project}"))).await;
    let items = resp.json()["items"].as_array().cloned().expect("items");
    assert_eq!(items.len(), 2);

    // ノードで絞る。
    let resp = send(&app, g("/api/v1/reports?node=secretary")).await;
    let items = resp.json()["items"].as_array().cloned().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], top.id.to_string());

    // level と未読で絞る（秘書レベルの未読 = 人が見る報告）。
    let resp = send(&app, g("/api/v1/reports?level=0&unread=true")).await;
    let items = resp.json()["items"].as_array().cloned().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "bad_news");

    // limit。
    let resp = send(&app, g("/api/v1/reports?limit=1")).await;
    assert_eq!(resp.json()["items"].as_array().map(Vec::len), Some(1));

    // 知らないクエリは 400。
    let resp = send(&app, g("/api/v1/reports?nope=1")).await;
    assert_eq!(resp.status.as_u16(), 400);

    // 1 件取得は sources を展開する。
    let resp = send(&app, g(&format!("/api/v1/reports/{}", top.id))).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["report"]["headline"], "落ちました");
    let expanded = body["sources_expanded"]
        .as_array()
        .cloned()
        .expect("sources");
    assert_eq!(expanded.len(), 1);
    assert_eq!(expanded[0]["id"], old.id.to_string());
    assert_eq!(new.sources.len(), 0);

    // 無い id は 404（ULID でない文字列も）。
    let resp = send(&app, g(&format!("/api/v1/reports/{}", ReportId::new()))).await;
    assert_eq!(resp.status.as_u16(), 404);
    assert_eq!(resp.json()["code"], "report_not_found");
    let resp = send(&app, g("/api/v1/reports/not-a-ulid")).await;
    assert_eq!(resp.status.as_u16(), 404);
}
#[tokio::test]
async fn the_daemon_snapshot_carries_the_unread_counts() {
    let env = TestEnv::new();
    let app = env.router();
    env.daemon_tx.send_replace(Some(snapshot(3)));
    let now = OffsetDateTime::now_utc();

    let resp = send(&app, get("/api/v1/daemon")).await;
    let reports = resp.json()["snapshot"]["reports"].clone();
    assert_eq!(reports["unread_secretary"], 0);

    // 秘書レベルの未読と、そのうちの悪い知らせを数える。
    for (kind, headline) in [
        (ReportKind::Result, "まとめ"),
        (ReportKind::BadNews, "ノードが落ちました"),
    ] {
        report(
            &env.store,
            "secretary",
            0,
            kind,
            None,
            headline,
            now,
            vec![],
        );
    }
    let resp = send(&app, get("/api/v1/daemon")).await;
    let reports = resp.json()["snapshot"]["reports"].clone();
    assert_eq!(reports["unread_secretary"], 2);
    assert_eq!(reports["unread_bad_news"], 1);
    assert!(reports.get("notify_now").is_none(), "{reports}");
}

/// ADR 2026-10-15-verify-counts-without-page-limit: 本番規模（reports 1723 件・1 node 600 件超、
/// messages 1 node 600 件超）でも件数の口は一覧の上限（500）に当たらず、生の行数と一致する。
#[tokio::test]
async fn count_endpoints_count_every_row_beyond_the_page_limit() {
    let env = admin_env();
    let app = env.router();
    let now = OffsetDateTime::now_utc();
    let project = ProjectId::new();
    for i in 0..1723u32 {
        let node = if i < 650 {
            "software-engineering"
        } else {
            "secretary"
        };
        let p = if i % 3 == 0 { Some(project) } else { None };
        report(
            &env.store,
            node,
            i % 3,
            ReportKind::Result,
            p,
            "r",
            now,
            vec![],
        );
    }
    for i in 0..650u32 {
        env.store
            .message_append(&task_core::Message {
                id: task_core::MessageId::new(),
                node_id: "software-engineering".into(),
                project_id: if i % 2 == 0 { Some(project) } else { None },
                role: task_core::MessageRole::User,
                text: format!("m{i}"),
                run_id: None,
                task_id: None,
                metadata: None,
                created_at: now,
            })
            .expect("append message");
    }

    // 一覧は上限で打ち切られる（これが旧 verify の数え漏れ）。
    let resp = send(
        &app,
        g("/api/v1/reports?node=software-engineering&limit=500"),
    )
    .await;
    assert_eq!(resp.json()["items"].as_array().map(Vec::len), Some(500));

    let resp = send(&app, g("/api/v1/reports/count")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["count"], 1723);
    let resp = send(&app, g("/api/v1/messages/count")).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    assert_eq!(resp.json()["count"], 650);
}
