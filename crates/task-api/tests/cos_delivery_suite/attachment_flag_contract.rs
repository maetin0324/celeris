//! アーキv4 H R4（ADR 2026-11-19-chat-attachment-store-connection D4・§5 の `attachment_flag_`）: 知識の候補の
//! `provenance[]` と `POST /knowledge/inbox` の `attachment_ids` は、添付の保存の設定（`with_chat_attachments` が入れる
//! `attachment_data_dir`）の有無だけで決まる。同じ DB・同じ KB を指す 2 つの `ApiState`（設定なしの `admin_env` の
//! state と、それに `with_chat_attachments` を足した state）の router を並べ、同じ候補を両方から読む。
//!
//! 試験ごとに守る不変条件と、既存の試験で検出できない理由:
//!
//! (F1) `attachment_flag_no_data_dir_hides_provenance_and_refuses_ids`
//!      添付の保存が無効なら、pin の行（`chat_attachment_refs` の owner_kind `knowledge_inbox`）を持つ候補でも
//!      一覧と詳細の `provenance` が空で、`attachment_ids` を渡す record は 503 `attachments_unavailable` になり、
//!      `_inbox/` に候補を残さず、参照行も添付の `expires_at` も変えない（api.md §3.104 と `POST /knowledge/inbox` の
//!      2 つの文）。`attachment_ids` の無い record は添付の保存に依らず 201 になる。
//!      既存で検出できない理由: 旗は `ChatState.attachment_db_path` から読み、`ApiState::new` が常に `Some` を入れて
//!      いたので、どの試験も無効の側を通らなかった。旗を常に真に戻す変更や、旗を DB の path から導き直す変更で、
//!      provenance が出る・pin を試みる（201）ようになっても落ちる試験が無かった。
//!
//! (F2) `attachment_flag_data_dir_shows_provenance`
//!      添付の保存が有効なら、`attachment_ids` つきの record が 201 で pin の行を 1 行作り、一覧と詳細の `provenance` に
//!      その添付が 1 件出る。
//!      既存で検出できない理由: tx_boundary_contract.rs の K1 などは有効の側を見るが、同じ候補を無効の state からも
//!      読む F1 と対にしていない。旗を偽に倒しすぎる変更（例: 判定を逆にする）は F1 だけでは検出できないので、
//!      同じ env の作り方で有効の側を固定する。
use crate::common;
use crate::tx_boundary_contract::{
    INBOX, expires_at, inbox_files, inbox_refs, record_body, upload,
};

use axum::Router;
use common::*;
use serde_json::Value;
use task_core::chat::attachments::ChatAttachmentLimits;

/// 設定なしの router（`plain`）と、同じ state に添付の保存を足した router（`attached`）を持つ env。
struct FlagEnv {
    env: TestEnv,
    plain: Router,
    attached: Router,
}

fn flag_env() -> FlagEnv {
    let env = admin_env();
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    let plain = task_api::router(env.state.clone());
    let attached = task_api::router(env.state.clone().with_chat_attachments(
        env.dir.path().to_path_buf(),
        ChatAttachmentLimits::default(),
    ));
    FlagEnv {
        env,
        plain,
        attached,
    }
}

/// 添付の保存が有効な router で添付 1 件を pin した候補を作り、`(候補の id, 添付の id)` を返す。
async fn pinned_candidate(flag: &FlagEnv, thread: &str) -> (String, String) {
    let attachment = upload(&flag.attached, thread, "flag-pinned").await;
    let created = send(
        &flag.attached,
        post_admin(INBOX, &record_body(&[&attachment])),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let id = created.json()["id"].as_str().expect("id").to_owned();
    assert_eq!(
        inbox_refs(&flag.env)
            .iter()
            .map(|r| (r.0.as_str(), r.1.as_str()))
            .collect::<Vec<_>>(),
        vec![(attachment.as_str(), id.as_str())],
        "the candidate owns one pin row"
    );
    (id, attachment)
}

/// 一覧（`GET /knowledge/inbox`）の `id` の候補の `provenance` と、詳細（`GET /knowledge/inbox/{id}`）の `provenance`。
async fn provenance(app: &Router, id: &str) -> (Vec<Value>, Vec<Value>) {
    let list = send(app, get_admin(INBOX)).await;
    assert_eq!(list.status.as_u16(), 200, "{}", list.text());
    let list = list.json();
    let item = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["id"] == id)
        .unwrap_or_else(|| panic!("candidate {id} in {list}"))
        .clone();
    let detail = send(app, get_admin(&format!("{INBOX}/{id}"))).await;
    assert_eq!(detail.status.as_u16(), 200, "{}", detail.text());
    let detail = detail.json();
    (
        item["provenance"]
            .as_array()
            .expect("list provenance")
            .clone(),
        detail["provenance"]
            .as_array()
            .expect("detail provenance")
            .clone(),
    )
}

#[tokio::test]
async fn attachment_flag_no_data_dir_hides_provenance_and_refuses_ids() {
    let flag = flag_env();
    let thread = project_cos_thread(&flag.env.store, "flag-off").id;
    let (id, _) = pinned_candidate(&flag, &thread).await;

    let (list, detail) = provenance(&flag.plain, &id).await;
    assert!(list.is_empty(), "list provenance without storage: {list:?}");
    assert!(
        detail.is_empty(),
        "detail provenance without storage: {detail:?}"
    );

    let loose = upload(&flag.attached, &thread, "flag-loose").await;
    let files = inbox_files(&flag.env);
    let refs = inbox_refs(&flag.env);
    let loose_expiry = expires_at(&flag.env, &loose);
    assert!(loose_expiry.is_some(), "an unpinned upload expires");
    let refused = send(&flag.plain, post_admin(INBOX, &record_body(&[&loose]))).await;
    assert_problem(&refused, 503, "attachments_unavailable");
    assert_eq!(inbox_files(&flag.env), files, "no candidate file is left");
    assert_eq!(inbox_refs(&flag.env), refs, "no pin row is written");
    assert_eq!(expires_at(&flag.env, &loose), loose_expiry);

    let plain_record = send(&flag.plain, post_admin(INBOX, &record_body(&[]))).await;
    assert_eq!(plain_record.status.as_u16(), 201, "{}", plain_record.text());
    assert_eq!(inbox_files(&flag.env).len(), files.len() + 1);
    assert_eq!(inbox_refs(&flag.env), refs);
}

#[tokio::test]
async fn attachment_flag_data_dir_shows_provenance() {
    let flag = flag_env();
    let thread = project_cos_thread(&flag.env.store, "flag-on").id;
    let (id, attachment) = pinned_candidate(&flag, &thread).await;

    let (list, detail) = provenance(&flag.attached, &id).await;
    for (label, rows) in [("list", &list), ("detail", &detail)] {
        assert_eq!(rows.len(), 1, "{label} provenance: {rows:?}");
        assert_eq!(rows[0]["attachment_id"], attachment.as_str(), "{label}");
        assert_eq!(rows[0]["thread_id"], thread.as_str(), "{label}");
    }
    assert_eq!(expires_at(&flag.env, &attachment), None, "pinned is kept");
}
