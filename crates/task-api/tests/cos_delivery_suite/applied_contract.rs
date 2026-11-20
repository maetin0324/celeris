//! アーキv4 H R5（contract）: `Option<&OperationAudit>` を受けて `Applied` を返す 6 つの共有関数
//! （`knowledge.rs` の `accept_op`・`reject_op`・`put_page_op`、`lifecycle.rs` の `task_pause_op`、
//! `skills.rs` の `put_skill_op`・`delete_skill_op`）を org_write の形へ移す前に、HTTP の handler と
//! `/api/v1/cos/operations` の 2 つの入口の契約を移す前のコードで固定する
//! （ADR 2026-11-20 applied-shape-migration。WorkUnit `adr` が書く）。
//!
//! 名前は `applied_contract_<op>_<http|cos>_<内容>`。既存の試験が見ている点は重ねない:
//! - `knowledge.rs`: accept の 404・`page_exists`・`path`/`etag`、reject の 403、PUT の etag 409・403・422・
//!   `unchanged`、GUI の編集の『人が書いた』印。
//! - `project_plan.rs`: task の pause の 200・`paused_at`・`subtree`・二重 pause の 409・知らない task の status 404。
//! - `unmapped_suite_b/skills.rs`: PUT の 200 `path`・422・401、mount 中の DELETE 409。
//! - `cos_ops_admin_config.rs`・`cos_operations_domains.rs`・`cos_ops_mutations.rs`・
//!   `cos_ops_projects_cron_docs.rs`: 6 op の CoS の applied・監査 event・card・直の呼び出しの 422・再送、
//!   accept の `result.path`、reject の `result.sha` と二度目の 404、page_put の etag 409・`_inbox` の 403 の
//!   status と commit 数、skill の `result.path`・`result.deleted`・mount 中の 409 の rejected の行の数。
//!
//! ここで足すのは、上の試験では検出できない次の点: 成功の応答の JSON の欄の集合と `sha` が KB の HEAD と同じこと、
//! 拒否の error code（task の 404 の code・etag 付きの新規 PUT・KB の未初期化など）、CoS の拒否の行の
//! `state`・`action`・`reason`・`result`（`audit.reject` の `rejected: <detail>; requested reason: …`、
//! `audit.apply` の rejected、外部効果の `{"error","detail"}`）、拒否で KB・task・org が変わらないこと、
//! task の pause の `paused_at` と applied の行が同じ transaction で書かれること、skill の frontmatter の
//! `source`（HTTP は `gui`、CoS は `cos`）。

use crate::common;

use common::cos_ops::{OPS, audit_events, cos_bearer, db, op_body, run_domain, run_external};
use common::*;
use serde_json::{Value, json};
use std::path::Path;
use task_core::{Event, Status, TaskKind, knowledge as kb};

/// CoS の依頼の理由（`op_body` が入れる）。
const REASON: &str = "人が依頼した";

const SKILL_MD: &str = "---\nname: rust-review\ndescription: Rust のコードレビューの手順\n---\n\n# rust-review\n\n手順...\n";

fn kb_env() -> TestEnv {
    let env = admin_env();
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    env
}

/// `_inbox/` の候補（ワーカーの `celerisctl knowledge record` と同じ関数）。宛先は `target`。
fn candidate(env: &TestEnv, target: &str) -> String {
    task_ops::knowledge::record(
        &env.knowledge_root,
        &task_ops::knowledge::RecordRequest {
            title: "fern03 の使い方".into(),
            scope: "environment".into(),
            sources: vec!["task:01J1".into()],
            body: "ssh fern03 で入る。".into(),
            path: Some(target.into()),
            ..task_ops::knowledge::RecordRequest::default()
        },
    )
    .expect("record")
    .id
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn kb_head(env: &TestEnv) -> String {
    git_out(&env.knowledge_root, &["rev-parse", "HEAD"])
}

fn kb_commits(env: &TestEnv) -> usize {
    git_out(&env.knowledge_root, &["rev-list", "--count", "HEAD"])
        .parse()
        .expect("count")
}

fn inbox_has(env: &TestEnv, id: &str) -> bool {
    env.knowledge_root
        .join("_inbox")
        .join(format!("{id}.md"))
        .exists()
}

fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .unwrap_or_else(|| panic!("object: {value}"))
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// 1 件の `cos_operations` の行（thread と idempotency key で引く）。
#[derive(Debug)]
struct OpRow {
    id: String,
    state: String,
    action: String,
    reason: String,
    target_kind: String,
    target_id: String,
    result: Value,
}

fn op_row(env: &TestEnv, thread: &str, key: &str) -> OpRow {
    db(env)
        .query_row(
            "SELECT id,state,action,reason,target_kind,target_id,result_json FROM cos_operations \
             WHERE thread_id=?1 AND idempotency_key=?2",
            [thread, key],
            |r| {
                Ok(OpRow {
                    id: r.get(0)?,
                    state: r.get(1)?,
                    action: r.get(2)?,
                    reason: r.get(3)?,
                    target_kind: r.get(4)?,
                    target_id: r.get(5)?,
                    result: serde_json::from_str(&r.get::<_, String>(6)?).expect("result json"),
                })
            },
        )
        .unwrap_or_else(|e| panic!("operation row {key}: {e}"))
}

/// `/cos/operations` に 1 件を新しい CoS の run で送る: `(応答, thread_id)`。
async fn envelope(
    env: &TestEnv,
    key: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (Resp, String) {
    let app = env.router();
    let (thread, _, bearer) = cos_bearer(env, key);
    let resp = send(
        &app,
        post_json_with(
            OPS,
            &op_body(key, method, path, body),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    (resp, thread)
}

/// `audit.reject` の形の拒否: 行は `state=rejected`・`action=rejected`、理由は
/// `rejected: <detail>; requested reason: <依頼の理由>`、監査 event は rejected の 1 件。
#[track_caller]
fn assert_pre_rejected(env: &TestEnv, row: &OpRow, kind: &str, target: &str, detail: &str) {
    assert_eq!(row.state, "rejected", "{row:?}");
    assert_eq!(row.action, "rejected", "{row:?}");
    assert_eq!(row.target_kind, kind, "{row:?}");
    assert_eq!(row.target_id, target, "{row:?}");
    assert_eq!(
        row.reason,
        format!("rejected: {detail}; requested reason: {REASON}"),
        "{row:?}"
    );
    let events = audit_events(env, &row.id);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["state"], "rejected");
    assert_eq!(events[0]["reason"], row.reason.as_str());
}

/// `audit.apply` の closure の失敗: 行は `state=rejected` で `action` はその操作、理由は依頼のまま、
/// `result.error` に失敗の文言。
#[track_caller]
fn assert_apply_rejected(env: &TestEnv, row: &OpRow, action: &str, error_contains: &str) {
    assert_eq!(row.state, "rejected", "{row:?}");
    assert_eq!(row.action, action, "{row:?}");
    assert_eq!(row.reason, REASON, "{row:?}");
    assert!(
        row.result["error"]
            .as_str()
            .is_some_and(|e| e.contains(error_contains)),
        "{row:?}"
    );
    let states: Vec<Value> = audit_events(env, &row.id)
        .into_iter()
        .map(|e| e["state"].clone())
        .collect();
    assert_eq!(states, vec![json!("rejected")], "{row:?}");
}

/// 外部効果（C）の拒否: pending の行が `rejected` に決まり、`result` は `{"error": <code>, "detail": …}`、
/// 監査 event は pending → rejected。
#[track_caller]
fn assert_external_rejected(env: &TestEnv, row: &OpRow, action: &str, code: &str) {
    assert_eq!(row.state, "rejected", "{row:?}");
    assert_eq!(row.action, action, "{row:?}");
    assert_eq!(row.reason, REASON, "{row:?}");
    assert_eq!(keys(&row.result), vec!["detail", "error"], "{row:?}");
    assert_eq!(row.result["error"], code, "{row:?}");
    let states: Vec<Value> = audit_events(env, &row.id)
        .into_iter()
        .map(|e| e["state"].clone())
        .collect();
    assert_eq!(states, vec![json!("pending"), json!("rejected")], "{row:?}");
}

fn skill_md_on_disk(env: &TestEnv) -> Option<String> {
    std::fs::read_to_string(env.knowledge_root.join("skills/rust-review/SKILL.md")).ok()
}

/// 組織に `coding` を作り `rust-review` を mount する（管理系の HTTP）。
async fn mount_rust_review(env: &TestEnv) {
    let app = env.router();
    for body in [
        json!({"id": "secretary", "name": "秘書", "kind": "secretary"}),
        json!({"id": "coding", "name": "コーディング部", "kind": "department", "parent_id": "secretary"}),
    ] {
        let resp = send(&app, post_admin("/api/v1/org", &body)).await;
        assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    }
    let resp = send(
        &app,
        post_admin(
            "/api/v1/org/coding/skills",
            &json!({"skill": "rust-review"}),
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
}

fn mounts_of_coding(env: &TestEnv) -> Vec<String> {
    env.store
        .org_get("coding")
        .expect("get")
        .expect("node")
        .profile
        .skills_mounts
}

// ---------------------------------------------------------------------------
// knowledge.accept
// ---------------------------------------------------------------------------

/// 守る点（HTTP `POST /knowledge/inbox/{id}/accept`）: 成功の JSON は `etag`・`path`・`sha`・`unchanged` の
/// 4 欄で `unchanged=false`、`sha` は KB の HEAD。id に `/` を含めば 403、取り込み先の `path` の誤りは
/// 403（根の外）・422（`.md` 以外）で、どちらも候補を残し commit しない。
/// 既存の `candidates_can_be_accepted_or_rejected` は `path`・`etag` の有無と 404 だけを見て、`sha` の値・
/// 欄の集合・accept の id の 403・取り込み先の検査の拒否で候補が残ることは見ない。
#[tokio::test]
async fn applied_contract_knowledge_accept_http_response_and_refusals() {
    let env = kb_env();
    let id = candidate(&env, "environment/servers/fern03.md");
    let app = env.router();
    let before = kb_commits(&env);

    let escape = send(
        &app,
        post_admin(
            &format!("/api/v1/knowledge/inbox/{id}/accept"),
            &json!({"path": "../x.md"}),
        ),
    )
    .await;
    assert_problem(&escape, 403, "path_forbidden");
    let not_md = send(
        &app,
        post_admin(
            &format!("/api/v1/knowledge/inbox/{id}/accept"),
            &json!({"path": "user/a.txt"}),
        ),
    )
    .await;
    assert_problem(&not_md, 422, "validation");
    let slash = send(
        &app,
        post_admin("/api/v1/knowledge/inbox/..%2F..%2Fetc/accept", &json!({})),
    )
    .await;
    assert_problem(&slash, 403, "path_forbidden");
    assert!(inbox_has(&env, &id), "拒否で候補が消えてはいけない");
    assert_eq!(kb_commits(&env), before, "拒否で commit してはいけない");

    let accepted = send(
        &app,
        post_admin(&format!("/api/v1/knowledge/inbox/{id}/accept"), &json!({})),
    )
    .await;
    assert_eq!(accepted.status.as_u16(), 200, "{}", accepted.text());
    let body = accepted.json();
    assert_eq!(
        keys(&body),
        vec!["etag", "path", "sha", "unchanged"],
        "{body}"
    );
    assert_eq!(body["path"], "environment/servers/fern03.md");
    assert_eq!(body["unchanged"], false);
    assert_eq!(body["sha"], kb_head(&env).as_str());
    assert_eq!(kb_commits(&env), before + 1);
    assert!(!inbox_has(&env, &id));
}

/// 守る点（CoS `knowledge.accept`）: applied の `result` は `id`・`path`・`sha` の 3 欄で `sha` は KB の HEAD、
/// 行の target は `knowledge`/<候補 id>。取り込み先の `path` の誤りは `audit.reject`（403、理由
/// `rejected: <detail>; requested reason: …`）。知らない候補は 404、既にあるページへの取り込みは 409
/// `page_exists` で、どちらも `audit.apply` の rejected の行（action は `knowledge.accept`）になり、KB に
/// commit せず候補と既存のページを変えない。
/// 既存の `cos_ops_mutations_knowledge_accept` は成功の `result.path` だけを見て、拒否の行・理由・rollback は見ない。
#[tokio::test]
async fn applied_contract_knowledge_accept_cos_result_and_rejections() {
    let env = kb_env();
    let id = candidate(&env, "environment/servers/fern03.md");
    let existing = env.knowledge_root.join("environment/existing.md");
    std::fs::create_dir_all(existing.parent().expect("parent")).expect("mkdir");
    std::fs::write(&existing, "# 既存\n").expect("write");
    git_out(&env.knowledge_root, &["add", "-A"]);
    git_out(
        &env.knowledge_root,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            "existing",
        ],
    );
    let before = kb_commits(&env);
    let accept = format!("/api/v1/knowledge/inbox/{id}/accept");

    let (resp, thread) = envelope(
        &env,
        "acc-escape",
        "POST",
        &accept,
        json!({"path": "../x.md"}),
    )
    .await;
    let problem = assert_problem(&resp, 403, "path_forbidden");
    let row = op_row(&env, &thread, "acc-escape");
    assert_pre_rejected(
        &env,
        &row,
        "knowledge",
        &id,
        problem["detail"].as_str().expect("detail"),
    );

    let (resp, thread) = envelope(
        &env,
        "acc-exists",
        "POST",
        &accept,
        json!({"path": "environment/existing.md"}),
    )
    .await;
    assert_problem(&resp, 409, "page_exists");
    assert_apply_rejected(
        &env,
        &op_row(&env, &thread, "acc-exists"),
        "knowledge.accept",
        "page already exists",
    );
    assert_eq!(
        std::fs::read_to_string(&existing).expect("read"),
        "# 既存\n"
    );

    let (resp, thread) = envelope(
        &env,
        "acc-missing",
        "POST",
        "/api/v1/knowledge/inbox/nope/accept",
        json!(null),
    )
    .await;
    assert_problem(&resp, 404, "candidate_not_found");
    let row = op_row(&env, &thread, "acc-missing");
    assert_apply_rejected(
        &env,
        &row,
        "knowledge.accept",
        "knowledge candidate not found: nope",
    );
    assert_eq!(
        (row.target_kind.as_str(), row.target_id.as_str()),
        ("knowledge", "nope")
    );

    assert!(inbox_has(&env, &id), "拒否で候補が消えてはいけない");
    assert_eq!(kb_commits(&env), before, "拒否で commit してはいけない");

    let (resp, thread) = envelope(&env, "acc-ok", "POST", &accept, json!(null)).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(keys(&op["result"]), vec!["id", "path", "sha"], "{op}");
    assert_eq!(op["result"]["id"], id.as_str());
    assert_eq!(op["result"]["sha"], kb_head(&env).as_str());
    let row = op_row(&env, &thread, "acc-ok");
    assert_eq!(
        (row.target_kind.as_str(), row.target_id.as_str()),
        ("knowledge", id.as_str())
    );
    assert_eq!(row.result, op["result"]);
    assert_eq!(kb_commits(&env), before + 1);
}

// ---------------------------------------------------------------------------
// knowledge.reject
// ---------------------------------------------------------------------------

/// 守る点（HTTP `POST /knowledge/inbox/{id}/reject`）: 成功の JSON は `id`・`sha` の 2 欄で、`id` は候補、
/// `sha` は KB の HEAD、候補の file は消える。知らない候補は 404 `candidate_not_found` で commit しない。
/// 既存の `candidates_can_be_accepted_or_rejected` は reject の 403 と成功の status だけを見て、応答の欄と
/// reject の 404 は見ない。
#[tokio::test]
async fn applied_contract_knowledge_reject_http_response_and_refusals() {
    let env = kb_env();
    let id = candidate(&env, "environment/servers/fern03.md");
    let app = env.router();
    let before = kb_commits(&env);

    let missing = send(
        &app,
        post_admin("/api/v1/knowledge/inbox/nope/reject", &json!({})),
    )
    .await;
    assert_problem(&missing, 404, "candidate_not_found");
    assert_eq!(kb_commits(&env), before);

    let rejected = send(
        &app,
        post_admin(&format!("/api/v1/knowledge/inbox/{id}/reject"), &json!({})),
    )
    .await;
    assert_eq!(rejected.status.as_u16(), 200, "{}", rejected.text());
    let body = rejected.json();
    assert_eq!(keys(&body), vec!["id", "sha"], "{body}");
    assert_eq!(body["id"], id.as_str());
    assert_eq!(body["sha"], kb_head(&env).as_str());
    assert_eq!(kb_commits(&env), before + 1);
    assert!(!inbox_has(&env, &id));
}

/// 守る点（CoS `knowledge.reject`）: applied の `result` は `id`・`sha` の 2 欄。空でない `request.body` は
/// 422 `validation` の `audit.reject`（理由 `rejected: request.body must be empty; requested reason: …`）、
/// 知らない候補は 404 の `audit.apply` の rejected の行（action `knowledge.reject`、理由は依頼のまま）。
/// どちらも候補を残し KB に commit しない。
/// 既存の `cos_chat_ops_domain_knowledge_reject` は `result.sha` と二度目の 404 の status だけを見て、行の
/// state・action・理由・body の検査は見ない。
#[tokio::test]
async fn applied_contract_knowledge_reject_cos_rejections_leave_kb_unchanged() {
    let env = kb_env();
    let id = candidate(&env, "environment/servers/fern03.md");
    let before = kb_commits(&env);
    let reject = format!("/api/v1/knowledge/inbox/{id}/reject");

    let (resp, thread) = envelope(&env, "rej-body", "POST", &reject, json!({"x": 1})).await;
    assert_problem(&resp, 422, "validation");
    assert_pre_rejected(
        &env,
        &op_row(&env, &thread, "rej-body"),
        "knowledge",
        &id,
        "request.body must be empty",
    );

    let (resp, thread) = envelope(
        &env,
        "rej-missing",
        "POST",
        "/api/v1/knowledge/inbox/nope/reject",
        json!({}),
    )
    .await;
    assert_problem(&resp, 404, "candidate_not_found");
    assert_apply_rejected(
        &env,
        &op_row(&env, &thread, "rej-missing"),
        "knowledge.reject",
        "nope",
    );

    assert!(inbox_has(&env, &id));
    assert_eq!(kb_commits(&env), before);

    let (resp, thread) = envelope(&env, "rej-ok", "POST", &reject, json!({})).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(keys(&op["result"]), vec!["id", "sha"], "{op}");
    assert_eq!(op["result"]["id"], id.as_str());
    assert_eq!(op["result"]["sha"], kb_head(&env).as_str());
    assert_eq!(op_row(&env, &thread, "rej-ok").state, "applied");
    assert!(!inbox_has(&env, &id));
}

// ---------------------------------------------------------------------------
// knowledge.page_put
// ---------------------------------------------------------------------------

/// 守る点（HTTP `PUT /knowledge/page`）: 成功の JSON は `etag`・`path`・`sha`・`unchanged` の 4 欄で、`sha` は
/// KB の HEAD、`etag` は `GET /knowledge/page` の etag と同じ。まだ無いページに etag を付けた PUT は 409
/// `etag_mismatch`（いまの etag が無いので `etag` 欄を持たない）で、file も commit も作らない。
/// 既存の `writing_a_page_checks_the_etag_and_the_path` は既にあるページの etag 409 と 403・422・`unchanged`
/// を見て、成功の欄の集合・`sha`・新規ページの etag 409 は見ない。
#[tokio::test]
async fn applied_contract_knowledge_page_put_http_response_and_refusals() {
    let env = kb_env();
    let app = env.router();
    let before = kb_commits(&env);

    let stale = send(
        &app,
        put_admin(
            "/api/v1/knowledge/page",
            &json!({"path": "user/new.md", "body": "# 新\n", "etag": "0000"}),
        ),
    )
    .await;
    let problem = assert_problem(&stale, 409, "etag_mismatch");
    assert!(problem.get("etag").is_none(), "{problem}");
    assert!(!env.knowledge_root.join("user/new.md").exists());
    assert_eq!(kb_commits(&env), before);

    let put = send(
        &app,
        put_admin(
            "/api/v1/knowledge/page",
            &json!({"path": "user/new.md", "body": "# 新\n"}),
        ),
    )
    .await;
    assert_eq!(put.status.as_u16(), 200, "{}", put.text());
    let body = put.json();
    assert_eq!(
        keys(&body),
        vec!["etag", "path", "sha", "unchanged"],
        "{body}"
    );
    assert_eq!(body["path"], "user/new.md");
    assert_eq!(body["unchanged"], false);
    assert_eq!(body["sha"], kb_head(&env).as_str());
    assert_eq!(kb_commits(&env), before + 1);
    let page = send(&app, get_admin("/api/v1/knowledge/page?path=user/new.md")).await;
    assert_eq!(page.json()["etag"], body["etag"]);
}

/// 守る点（CoS `knowledge.page_put`）: applied の `result` は HTTP と同じ 4 欄、行の target は
/// `knowledge_page`/<path>、CoS の編集も GUI と同じ『人が書いた』印（`sources: ["human:authored"]`）で保存する。
/// `_inbox/` は 403 の `audit.reject`（理由 `rejected: <detail>; requested reason: …`）。古い etag は外部効果の
/// 拒否（pending → rejected、`result` は `{"error":"etag_mismatch","detail"}`）で、ページを変えず commit しない。
/// 既存の `cos_ops_projects_cron_knowledge_page_put_is_external_once` は applied・再送・etag の 409 と commit
/// 数・`_inbox` の status だけを見て、行の state・action・理由・`result`・人の印は見ない。
#[tokio::test]
async fn applied_contract_knowledge_page_put_cos_rejections_and_human_mark() {
    let env = kb_env();
    let page = "/api/v1/knowledge/page";

    let (resp, thread) = envelope(
        &env,
        "put-ok",
        "PUT",
        page,
        json!({"path": "environment/notes.md", "body": "# メモ\n"}),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(
        keys(&op["result"]),
        vec!["etag", "path", "sha", "unchanged"],
        "{op}"
    );
    assert_eq!(op["result"]["sha"], kb_head(&env).as_str());
    let row = op_row(&env, &thread, "put-ok");
    assert_eq!(
        (
            row.state.as_str(),
            row.target_kind.as_str(),
            row.target_id.as_str()
        ),
        ("applied", "knowledge_page", "environment/notes.md")
    );
    let raw =
        std::fs::read_to_string(env.knowledge_root.join("environment/notes.md")).expect("read");
    assert_eq!(raw, "---\nsources: [\"human:authored\"]\n---\n\n# メモ\n");
    assert!(kb::protected_page("environment/notes.md", &raw));
    let before = kb_commits(&env);

    let (resp, thread) = envelope(
        &env,
        "put-inbox",
        "PUT",
        page,
        json!({"path": "_inbox/x.md", "body": "# x\n"}),
    )
    .await;
    let problem = assert_problem(&resp, 403, "path_forbidden");
    assert_pre_rejected(
        &env,
        &op_row(&env, &thread, "put-inbox"),
        "knowledge_page",
        "_inbox/x.md",
        problem["detail"].as_str().expect("detail"),
    );

    let (resp, thread) = envelope(
        &env,
        "put-stale",
        "PUT",
        page,
        json!({"path": "environment/notes.md", "body": "# 上書き\n", "etag": "0000"}),
    )
    .await;
    assert_problem(&resp, 409, "etag_mismatch");
    assert_external_rejected(
        &env,
        &op_row(&env, &thread, "put-stale"),
        "knowledge.page_put",
        "etag_mismatch",
    );

    assert_eq!(kb_commits(&env), before, "拒否で commit してはいけない");
    assert_eq!(
        std::fs::read_to_string(env.knowledge_root.join("environment/notes.md")).expect("read"),
        raw
    );
    assert!(!env.knowledge_root.join("_inbox/x.md").exists());
}

// ---------------------------------------------------------------------------
// task.pause
// ---------------------------------------------------------------------------

/// 守る点（HTTP `POST /tasks/{id}/pause`）: 成功の JSON は `paused_at`・`subtree`・`task` の 3 欄で `task.id` は
/// 対象、`Edited { fields: ["paused_at"], by: <認証した principal> }` を 1 件残す。知らない task は 404
/// `task_not_found`、ULID でない id は 400 `bad_request`、終わった task は 409 `invalid_transition` で
/// `paused_at` も event も書かない。
/// 既存の `project_plan.rs` の pause の試験は 200・`paused_at`・`subtree`・二重 pause の 409・404 の status
/// だけを見て、欄の集合・`Edited.by`・404 の code・400・終わった task の拒否は見ない。
#[tokio::test]
async fn applied_contract_task_pause_http_response_and_refusals() {
    let env = admin_env();
    let app = env.router();
    let ready = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&ready);
    let done = new_task(TaskKind::Execute, Status::Done);
    env.seed(&done);

    let missing = send(
        &app,
        post_admin(
            &format!("/api/v1/tasks/{}/pause", task_core::TaskId::new()),
            &json!({}),
        ),
    )
    .await;
    assert_problem(&missing, 404, "task_not_found");
    let malformed = send(
        &app,
        post_admin("/api/v1/tasks/not-a-ulid/pause", &json!({})),
    )
    .await;
    assert_problem(&malformed, 400, "bad_request");
    let events_before = env.store.events_for(done.id).expect("events").len();
    let terminal = send(
        &app,
        post_admin(&format!("/api/v1/tasks/{}/pause", done.id), &json!({})),
    )
    .await;
    assert_problem(&terminal, 409, "invalid_transition");
    let after = env.store.get(done.id).expect("get").expect("task");
    assert!(after.paused_at.is_none());
    assert_eq!(
        env.store.events_for(done.id).expect("events").len(),
        events_before
    );

    let paused = send(
        &app,
        post_admin(&format!("/api/v1/tasks/{}/pause", ready.id), &json!({})),
    )
    .await;
    assert_eq!(paused.status.as_u16(), 200, "{}", paused.text());
    let body = paused.json();
    assert_eq!(keys(&body), vec!["paused_at", "subtree", "task"], "{body}");
    assert_eq!(body["task"]["id"], ready.id.to_string());
    let edits: Vec<(Vec<String>, String)> = env
        .store
        .events_for(ready.id)
        .expect("events")
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::Edited { fields, by } => Some((fields, by)),
            _ => None,
        })
        .collect();
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert_eq!(edits[0].0, vec!["paused_at".to_string()]);
    assert_eq!(edits[0].1, TEST_ACTOR, "{edits:?}");
}

/// 守る点（CoS `task.pause`）: task の `paused_at`・`Edited` event と applied の行・監査 event は同じ
/// transaction で書く。applied の行の挿入を落とすと task は止まらず `Edited` も残らない（片方だけ残らない）。
/// 成功の `result` は `{"task_id", "paused": true}`、`Edited.by` は `cos:<thread>`。知らない task（404
/// `task_not_found`）と既に止めた task（409 `invalid_transition`）は `audit.reject` の行（理由
/// `rejected: <detail>; requested reason: …`）で、task を変えない。
/// 既存の `cos_ops_mutations_task_edit_pause_resume_reopen_retry` は applied と `paused_at` の有無だけを見て、
/// 同じ transaction・`result`・拒否の行は見ない。`tx_boundary_contract.rs` の同じ trigger の試験は案件の
/// cancel だけを見る。
#[tokio::test]
async fn applied_contract_task_pause_cos_same_transaction_and_rejections() {
    let env = admin_env();
    let ready = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&ready);
    let pause = format!("/api/v1/tasks/{}/pause", ready.id);
    let edited = |env: &TestEnv| -> Vec<String> {
        env.store
            .events_for(ready.id)
            .expect("events")
            .into_iter()
            .filter_map(|(_, e)| match e {
                Event::Edited { by, .. } => Some(by),
                _ => None,
            })
            .collect()
    };

    db(&env)
        .execute_batch(
            "CREATE TRIGGER applied_contract_fault BEFORE INSERT ON cos_operations \
             WHEN NEW.state='applied' BEGIN SELECT RAISE(ABORT, 'applied_contract_fault'); END;",
        )
        .expect("trigger");
    let (resp, thread) = envelope(&env, "pause-fault", "POST", &pause, json!({})).await;
    assert!(
        resp.status.is_server_error(),
        "{}: {}",
        resp.status,
        resp.text()
    );
    assert!(
        env.store
            .get(ready.id)
            .expect("get")
            .expect("task")
            .paused_at
            .is_none(),
        "監査の行が書けなければ task も止めない"
    );
    assert!(
        edited(&env).is_empty(),
        "監査の行が書けなければ Edited も残さない"
    );
    let applied: i64 = db(&env)
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE thread_id=?1 AND state='applied'",
            [&thread],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(applied, 0);
    db(&env)
        .execute_batch("DROP TRIGGER applied_contract_fault")
        .expect("drop trigger");

    let (resp, thread) = envelope(&env, "pause-ok", "POST", &pause, json!({})).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(
        op["result"],
        json!({"task_id": ready.id.to_string(), "paused": true}),
        "{op}"
    );
    let row = op_row(&env, &thread, "pause-ok");
    assert_eq!(
        (
            row.state.as_str(),
            row.action.as_str(),
            row.target_kind.as_str()
        ),
        ("applied", "task.pause", "task")
    );
    let paused_at = env
        .store
        .get(ready.id)
        .expect("get")
        .expect("task")
        .paused_at;
    assert!(paused_at.is_some());
    assert_eq!(edited(&env), vec![format!("cos:{thread}")]);

    let (resp, thread) = envelope(&env, "pause-again", "POST", &pause, json!({})).await;
    let problem = assert_problem(&resp, 409, "invalid_transition");
    assert_pre_rejected(
        &env,
        &op_row(&env, &thread, "pause-again"),
        "task",
        &ready.id.to_string(),
        problem["detail"].as_str().expect("detail"),
    );
    let missing = task_core::TaskId::new();
    let (resp, thread) = envelope(
        &env,
        "pause-missing",
        "POST",
        &format!("/api/v1/tasks/{missing}/pause"),
        json!({}),
    )
    .await;
    let problem = assert_problem(&resp, 404, "task_not_found");
    assert_pre_rejected(
        &env,
        &op_row(&env, &thread, "pause-missing"),
        "task",
        &missing.to_string(),
        problem["detail"].as_str().expect("detail"),
    );
    assert_eq!(
        env.store
            .get(ready.id)
            .expect("get")
            .expect("task")
            .paused_at,
        paused_at
    );
    assert_eq!(edited(&env).len(), 1, "拒否で Edited を足さない");
}

// ---------------------------------------------------------------------------
// skill.put
// ---------------------------------------------------------------------------

/// 守る点（HTTP `PUT /skills/{name}`）: GUI からの書き込みは frontmatter に `source: gui` を残す。KB が無ければ
/// 409 `knowledge_unavailable` で何も作らない。
/// 既存の `put_skill_creates_and_updates_and_requires_admin` は `path`・422・401 だけを見て、`source` と未初期化の
/// 拒否は見ない。
#[tokio::test]
async fn applied_contract_skill_put_http_source_and_refusals() {
    let env = admin_env();
    let app = env.router();
    let refused = send(
        &app,
        put_admin("/api/v1/skills/rust-review", &json!({"skill_md": SKILL_MD})),
    )
    .await;
    assert_problem(&refused, 409, "knowledge_unavailable");
    assert!(!env.knowledge_root.exists(), "拒否で KB を作ってはいけない");

    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    let put = send(
        &app,
        put_admin("/api/v1/skills/rust-review", &json!({"skill_md": SKILL_MD})),
    )
    .await;
    assert_eq!(put.status.as_u16(), 200, "{}", put.text());
    assert_eq!(put.json(), json!({"path": "skills/rust-review/SKILL.md"}));
    let raw = skill_md_on_disk(&env).expect("SKILL.md");
    assert!(raw.contains("\nsource: gui\n"), "{raw}");
}

/// 守る点（CoS `skill.put`）: CoS の書き込みは frontmatter に `source: cos` を残し、`result` は `{"path"}`。
/// 名前の不一致は外部効果の拒否（422、pending → rejected、`result` は `{"error":"validation","detail"}`）で
/// SKILL.md を作らない。KB が無ければ `audit.reject` の行（409、理由 `rejected: <detail>; requested reason: …`）。
/// 既存の `cos_ops_admin_config_org_and_skills_are_audited` は applied の `result.path` と file の有無だけを
/// 見て、`source` と拒否の行は見ない。
#[tokio::test]
async fn applied_contract_skill_put_cos_source_and_rejections() {
    let env = admin_env();
    let skill = "/api/v1/skills/rust-review";
    let (resp, thread) = envelope(
        &env,
        "skill-nokb",
        "PUT",
        skill,
        json!({"skill_md": SKILL_MD}),
    )
    .await;
    let problem = assert_problem(&resp, 409, "knowledge_unavailable");
    assert_pre_rejected(
        &env,
        &op_row(&env, &thread, "skill-nokb"),
        "skill",
        "rust-review",
        problem["detail"].as_str().expect("detail"),
    );
    assert!(!env.knowledge_root.exists());

    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    let before = kb_commits(&env);
    let (resp, thread) = envelope(
        &env,
        "skill-mismatch",
        "PUT",
        skill,
        json!({"skill_md": "---\nname: other\ndescription: x\n---\n"}),
    )
    .await;
    assert_problem(&resp, 422, "validation");
    assert_external_rejected(
        &env,
        &op_row(&env, &thread, "skill-mismatch"),
        "skill.put",
        "validation",
    );
    assert!(skill_md_on_disk(&env).is_none());
    assert_eq!(kb_commits(&env), before);

    let op = run_external(
        &env,
        "skill-ok",
        "PUT",
        skill,
        json!({"skill_md": SKILL_MD}),
        "skill.put",
    )
    .await;
    assert_eq!(op["result"], json!({"path": "skills/rust-review/SKILL.md"}));
    assert_eq!(
        (op["target_kind"].as_str(), op["target_id"].as_str()),
        (Some("skill"), Some("rust-review"))
    );
    let raw = skill_md_on_disk(&env).expect("SKILL.md");
    assert!(raw.contains("\nsource: cos\n"), "{raw}");
}

// ---------------------------------------------------------------------------
// skill.delete
// ---------------------------------------------------------------------------

/// 守る点（HTTP `DELETE /skills/{name}`）: 成功は 204 で本文が空、SKILL.md を消して KB に 1 commit。
/// 知らない skill は 404 `skill_not_found`、mount 中は 409 `skill_mounted` で SKILL.md と mount を残す。
/// 既存の `mounting_a_skill_on_a_node_shows_up_in_mounted_by_including_children` は mount 中の 409 の code
/// だけを見て、204 の本文・commit・404・拒否で file と mount が残ることは見ない。
#[tokio::test]
async fn applied_contract_skill_delete_http_status_and_refusals() {
    let env = kb_env();
    let app = env.router();
    let put = send(
        &app,
        put_admin("/api/v1/skills/rust-review", &json!({"skill_md": SKILL_MD})),
    )
    .await;
    assert_eq!(put.status.as_u16(), 200, "{}", put.text());

    let missing = send(&app, delete_admin("/api/v1/skills/nope")).await;
    assert_problem(&missing, 404, "skill_not_found");

    mount_rust_review(&env).await;
    let before = kb_commits(&env);
    let mounted = send(&app, delete_admin("/api/v1/skills/rust-review")).await;
    assert_problem(&mounted, 409, "skill_mounted");
    assert!(skill_md_on_disk(&env).is_some());
    assert_eq!(mounts_of_coding(&env), vec!["rust-review".to_string()]);
    assert_eq!(kb_commits(&env), before);

    let unmount = send(&app, delete_admin("/api/v1/org/coding/skills/rust-review")).await;
    assert_eq!(unmount.status.as_u16(), 200, "{}", unmount.text());
    let deleted = send(&app, delete_admin("/api/v1/skills/rust-review")).await;
    assert_eq!(deleted.status.as_u16(), 204, "{}", deleted.text());
    assert!(deleted.text().is_empty(), "{}", deleted.text());
    assert!(skill_md_on_disk(&env).is_none());
    assert_eq!(kb_commits(&env), before + 1);
}

/// 守る点（CoS `skill.delete`）: applied の `result` は `{"name", "deleted": true}`。知らない skill（404
/// `skill_not_found`）・mount 中（409 `skill_mounted`）・空でない `request.body`（422 `validation`）は
/// `audit.reject` の行（理由 `rejected: <detail>; requested reason: …`）で、SKILL.md・組織の mount・KB の commit
/// を変えない。
/// 既存の `cos_ops_admin_config_org_and_skills_are_audited` は mount 中の拒否の行の数と `result.deleted` だけを
/// 見て、理由の文言・404・body の検査・拒否で org と KB が変わらないことは見ない。
#[tokio::test]
async fn applied_contract_skill_delete_cos_result_and_rejections() {
    let env = kb_env();
    let app = env.router();
    let put = send(
        &app,
        put_admin("/api/v1/skills/rust-review", &json!({"skill_md": SKILL_MD})),
    )
    .await;
    assert_eq!(put.status.as_u16(), 200, "{}", put.text());
    mount_rust_review(&env).await;
    let before = kb_commits(&env);
    let skill = "/api/v1/skills/rust-review";

    let (resp, thread) = envelope(
        &env,
        "del-missing",
        "DELETE",
        "/api/v1/skills/nope",
        json!(null),
    )
    .await;
    assert_problem(&resp, 404, "skill_not_found");
    assert_pre_rejected(
        &env,
        &op_row(&env, &thread, "del-missing"),
        "skill",
        "nope",
        "skill not found: nope",
    );

    let (resp, thread) = envelope(&env, "del-mounted", "DELETE", skill, json!(null)).await;
    let problem = assert_problem(&resp, 409, "skill_mounted");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("coding")),
        "{problem}"
    );
    assert_pre_rejected(
        &env,
        &op_row(&env, &thread, "del-mounted"),
        "skill",
        "rust-review",
        problem["detail"].as_str().expect("detail"),
    );

    let (resp, thread) = envelope(&env, "del-body", "DELETE", skill, json!({"x": 1})).await;
    let problem = assert_problem(&resp, 422, "validation");
    let row = op_row(&env, &thread, "del-body");
    assert_eq!(
        (row.state.as_str(), row.action.as_str()),
        ("rejected", "rejected"),
        "{row:?}"
    );
    assert_eq!(
        row.reason,
        format!(
            "rejected: {}; requested reason: {REASON}",
            problem["detail"].as_str().expect("detail")
        )
    );

    assert!(
        skill_md_on_disk(&env).is_some(),
        "拒否で SKILL.md を消さない"
    );
    assert_eq!(mounts_of_coding(&env), vec!["rust-review".to_string()]);
    assert_eq!(kb_commits(&env), before);

    run_domain(
        &env,
        "del-unmount",
        "DELETE",
        "/api/v1/org/coding/skills/rust-review",
        json!(null),
        "org.skill_unmount",
    )
    .await;
    let op = run_external(&env, "del-ok", "DELETE", skill, json!(null), "skill.delete").await;
    assert_eq!(
        op["result"],
        json!({"name": "rust-review", "deleted": true})
    );
    assert!(skill_md_on_disk(&env).is_none());
}
