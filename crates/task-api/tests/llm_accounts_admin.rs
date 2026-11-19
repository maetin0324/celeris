//! ADR-0024（Phase 13）/ ADR 2026-10-11 llm-source-harness-selection: アカウントの管理 API（source 単位の路
//! `/api/v1/llm/sources/{source_id}/accounts/{account_id}…`）。`check`/`login`/`login/code`/`DELETE .../login` は
//! celeris 側の実行（`AdminRequest` 経由）が要るので、ここでは task-api だけで完結する部分と、celeris の代わりの
//! ダブルへ届く引数（adapter の内部 key）を確かめる。実際の確認・ログイン中継は `tests/e2e/tests/account_pool_scenarios.rs`
//! で実バイナリを使って検証する。

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};

use common::*;
use serde_json::json;
use task_api::{AccountAdminError, AccountLoginStartOutcome, AdminRequest};
use task_core::{Event, Status, TaskKind};
use task_ops::daemon::{AccountCooldownLive, AccountLive, AccountUsageLive};
use tokio::sync::mpsc;

fn auth() -> String {
    format!("Bearer {TOKEN}")
}

/// `[accounts] claude_dir` に使う一時ディレクトリを持つ環境（source `claude_oauth` の pool）。
fn env_with_accounts_root() -> (TestEnv, tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(&root).unwrap();
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        accounts_root: Some(root.clone()),
        max_runs_per_account: 2,
        ..Default::default()
    });
    (env, tmp, root)
}

/// celeris 側の `AdminRequest::AccountRemove` のダブル。本物の実装とその単体テストは `crates/celeris/src/accounts_admin.rs`。
/// 同じ fs 操作（`.removed/<id>-<unix>` への move）と `in_use` の判定を行う。`in_use` はディスパッチャの
/// `account_in_use` の代わりに、このダブルが見る集合。
fn spawn_account_remove_double(
    root: std::path::PathBuf,
    in_use: Arc<StdMutex<HashSet<String>>>,
) -> mpsc::Sender<AdminRequest> {
    let (tx, mut rx) = mpsc::channel::<AdminRequest>(8);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            if let AdminRequest::AccountRemove { id, reply, .. } = req {
                let result: Result<(), AccountAdminError> = (|| {
                    let dir = root.join(&id);
                    if !dir.is_dir() {
                        return Err(AccountAdminError::NotFound);
                    }
                    if in_use
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .contains(&id)
                    {
                        return Err(AccountAdminError::InUse);
                    }
                    let removed_dir = root.join(".removed");
                    std::fs::create_dir_all(&removed_dir)
                        .map_err(|e| AccountAdminError::Unavailable(e.to_string()))?;
                    let dest = removed_dir.join(format!(
                        "{id}-{}",
                        time::OffsetDateTime::now_utc().unix_timestamp()
                    ));
                    std::fs::rename(&dir, &dest)
                        .map_err(|e| AccountAdminError::Unavailable(e.to_string()))?;
                    Ok(())
                })();
                let _ = reply.send(result);
            }
        }
    });
    tx
}

/// `env_with_accounts_root` に `spawn_account_remove_double` を配線したもの。
fn env_with_accounts_root_and_remove_double() -> (
    TestEnv,
    tempfile::TempDir,
    std::path::PathBuf,
    Arc<StdMutex<HashSet<String>>>,
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(&root).unwrap();
    let in_use = Arc::new(StdMutex::new(HashSet::new()));
    let admin_tx = spawn_account_remove_double(root.clone(), in_use.clone());
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        accounts_root: Some(root.clone()),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    (env, tmp, root, in_use)
}

/// 管理系エンドポイントは `token_file` が無くても常に 401（ADR-0017 M3 / api.md §3.29）。`GET /llm/accounts` は
/// 読み取りなので、token を設定していない（loopback 限定）構成でも 200 になる。
#[tokio::test]
async fn management_routes_all_require_a_token() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(&root).unwrap();
    let env = TestEnv::with(EnvOptions {
        token: None,
        accounts_root: Some(root),
        max_runs_per_account: 2,
        ..Default::default()
    });
    let app = env.router();

    let resp = send(
        &app,
        post_json(
            "/api/v1/llm/sources/claude_oauth/accounts",
            &json!({"id": "a"}),
        ),
    )
    .await;
    assert_problem(&resp, 401, "unauthorized");

    let resp = send(
        &app,
        delete_with("/api/v1/llm/sources/claude_oauth/accounts/a", &[]),
    )
    .await;
    assert_problem(&resp, 401, "unauthorized");

    let resp = send(
        &app,
        post_json(
            "/api/v1/llm/sources/claude_oauth/accounts/a/check",
            &json!({}),
        ),
    )
    .await;
    assert_problem(&resp, 401, "unauthorized");

    let resp = send(
        &app,
        post_json(
            "/api/v1/llm/sources/claude_oauth/accounts/a/login",
            &json!({}),
        ),
    )
    .await;
    assert_problem(&resp, 401, "unauthorized");

    let resp = send(
        &app,
        delete_with("/api/v1/llm/sources/claude_oauth/accounts/a/login", &[]),
    )
    .await;
    assert_problem(&resp, 401, "unauthorized");

    let resp = send(
        &app,
        post_json(
            "/api/v1/llm/sources/claude_oauth/accounts/a/login/code",
            &json!({"code": "x"}),
        ),
    )
    .await;
    assert_problem(&resp, 401, "unauthorized");

    let resp = send(
        &app,
        patch_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts/a",
            &json!({"max_runs": 2}),
            &[],
        ),
    )
    .await;
    assert_problem(&resp, 401, "unauthorized");

    // GET は読み取りなので token 不要（他のトークン無しの読み取りエンドポイントと同じ扱い）。
    let resp = send(&app, get("/api/v1/llm/accounts")).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
}

/// pool の root（`[accounts] claude_dir`）が無い構成では、一覧は `roots: {}` `items: []`。source 単位の管理系は
/// 実 account の root が無いので 404（`account_not_found`）。
#[tokio::test]
async fn pool_without_root_lists_nothing_and_management_is_not_found() {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        ..Default::default()
    });
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];

    // GET は認証ありでも普通に読める（他の読み取りエンドポイントと同じ）。
    let resp = send(&app, get_with("/api/v1/llm/accounts", &headers)).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let v = resp.json();
    assert_eq!(v["roots"], json!({}));
    assert_eq!(v["items"], json!([]));

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts",
            &json!({"id": "a"}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");

    let resp = send(
        &app,
        delete_with("/api/v1/llm/sources/claude_oauth/accounts/a", &headers),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts/a/check",
            &json!({}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");
}

/// 未知の source（pool でも暗黙でもない）は、どの管理系でも 404 `account_not_found`。
#[tokio::test]
async fn unknown_source_is_not_found() {
    let (env, _tmp, _root) = env_with_accounts_root();
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/nope/accounts",
            &json!({"id": "a"}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");

    let resp = send(
        &app,
        delete_with("/api/v1/llm/sources/nope/accounts/a", &headers),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");

    let resp = send(
        &app,
        patch_json_with(
            "/api/v1/llm/sources/nope/accounts/a",
            &json!({"max_runs": 2}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");
}

/// `POST /llm/sources/claude_oauth/accounts` はディレクトリを作る。既にあれば 409 `account_exists`。不正な id は 400。
#[tokio::test]
async fn create_account_makes_a_directory_and_rejects_duplicates_and_bad_ids() {
    let (env, _tmp, root) = env_with_accounts_root();
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts",
            &json!({"id": "../escape"}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 400, "bad_request");

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts",
            &json!({"id": "acct-b"}),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status, 201, "{}", resp.text());
    assert_eq!(
        resp.header("location"),
        Some("/api/v1/llm/sources/claude_oauth/accounts/acct-b")
    );
    let v = resp.json();
    assert_eq!(v["source_id"], json!("claude_oauth"));
    assert_eq!(v["account_id"], json!("acct-b"));
    assert!(v.get("adapter").is_none(), "{v}");
    assert!(v.get("id").is_none(), "{v}");
    assert_eq!(v["logged_in"], json!(false));
    assert_eq!(v["in_use"], json!(0));
    assert!(root.join("acct-b").is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root.join("acct-b"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts",
            &json!({"id": "acct-b"}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 409, "account_exists");
}

/// `DELETE .../accounts/{account_id}` は celeris 側（`AdminRequest::AccountRemove`）へ委譲され、ディレクトリを
/// `.removed/<id>-<unix>` へ移す（S2+S8）。無ければ 404。`admin_tx` が無い（celeris に届かない）構成は
/// 409 `accounts_unavailable`。
#[tokio::test]
async fn delete_account_moves_the_directory_to_removed_and_missing_is_404() {
    let (env, _tmp, root, _in_use) = env_with_accounts_root_and_remove_double();
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];
    std::fs::create_dir_all(root.join("acct-b")).unwrap();
    std::fs::write(root.join("acct-b").join(".credentials.json"), "{}").unwrap();

    let resp = send(
        &app,
        delete_with(
            "/api/v1/llm/sources/claude_oauth/accounts/does-not-exist",
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");

    let resp = send(
        &app,
        delete_with("/api/v1/llm/sources/claude_oauth/accounts/acct-b", &headers),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert!(!root.join("acct-b").exists());
    let removed_entries: Vec<_> = std::fs::read_dir(root.join(".removed")).unwrap().collect();
    assert_eq!(removed_entries.len(), 1);
    let moved = removed_entries.into_iter().next().unwrap().unwrap().path();
    assert!(
        moved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("acct-b-")
    );
    assert!(
        moved.join(".credentials.json").is_file(),
        "credentials are not deleted, just moved"
    );
}

/// `[accounts]` はあるが `admin_tx`（celeris への経路）が無い構成は 409 `accounts_unavailable`（celeris 側で
/// 実行するしかない操作なので、届かなければ「使えない」）。
#[tokio::test]
async fn delete_account_without_admin_tx_is_accounts_unavailable() {
    let (env, _tmp, root) = env_with_accounts_root();
    let app = env.router();
    let auth = auth();
    std::fs::create_dir_all(root.join("acct-b")).unwrap();

    let resp = send(
        &app,
        delete_with(
            "/api/v1/llm/sources/claude_oauth/accounts/acct-b",
            &[("authorization", &auth)],
        ),
    )
    .await;
    assert_problem(&resp, 409, "accounts_unavailable");
    assert!(root.join("acct-b").exists());
}

/// `in_use > 0`（celeris 側のディスパッチャの権威ある値）のアカウントは削除できない（S2+S8）。
#[tokio::test]
async fn delete_account_in_use_is_conflict() {
    let (env, _tmp, root, in_use) = env_with_accounts_root_and_remove_double();
    let app = env.router();
    let auth = auth();
    std::fs::create_dir_all(root.join("acct-b")).unwrap();
    in_use.lock().unwrap().insert("acct-b".to_string());

    let resp = send(
        &app,
        delete_with(
            "/api/v1/llm/sources/claude_oauth/accounts/acct-b",
            &[("authorization", &auth)],
        ),
    )
    .await;
    assert_problem(&resp, 409, "account_in_use");
    assert!(root.join("acct-b").exists());
}

/// `GET /llm/accounts` はディレクトリのスキャン（`logged_in`/`dir`）とスナップショットの観測値（`score`/`cooldown`/
/// `usage`/`login_pending`）、`WorkerStarted.account` からの集計を合わせる。
#[tokio::test]
async fn list_accounts_merges_filesystem_snapshot_and_stats() {
    let (env, _tmp, root) = env_with_accounts_root();
    let app = env.router();

    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("b")).unwrap();
    std::fs::write(root.join("b").join(".credentials.json"), "{}").unwrap();

    // b は WorkerStarted/WorkerFinished を通じて集計に現れる。
    let task = new_task(TaskKind::Execute, Status::Done);
    env.seed_with(
        &task,
        vec![
            Event::WorkerStarted {
                run_id: "r1".into(),
                adapter: "claude-code".into(),
                model: "m".into(),
                provider: Some("pool".into()),
                account: Some("b".into()),
                role: None,
                task_role: None,
            },
            Event::WorkerFinished {
                run_id: "r1".into(),
                outcome: "done: ok".into(),
                usage: Some(task_core::Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(4),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: None,
                    duplicate_reads: None,
                    session_resumed: None,
                    context_tokens: None,
                }),
                role: None,
                metrics: None,
                end: None,
            },
        ],
    );

    let mut snap = snapshot(1);
    snap.accounts_root = Some(root.display().to_string());
    snap.max_runs_per_account = Some(2);
    snap.accounts = vec![AccountLive {
        adapter: "claude-code".into(),
        id: "b".into(),
        logged_in: true,
        in_use: 1,
        usage: Some(AccountUsageLive {
            one_month: None,
            five_hour: Some(task_core::RateWindow {
                utilization: 0.2,
                resets_at: 2_000_000_000,
            }),
            seven_day: None,
            status: Some("allowed".into()),
            observed_at: 1_900_000_000,
            source: "run".into(),
        }),
        score: Some(0.8),
        excluded_reason: None,
        cooldown: Some(AccountCooldownLive {
            until: 1_900_000_100,
            reason: "throttled".into(),
        }),
        last_check: None,
        login_pending: true,
    }];
    env.daemon_tx.send(Some(snap)).unwrap();

    let resp = send(
        &app,
        get_with("/api/v1/llm/accounts", &[("authorization", &auth())]),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let v = resp.json();
    assert_eq!(
        v["roots"]["claude_oauth"],
        json!(root.display().to_string())
    );
    assert_eq!(v["max_runs_per_account"], json!(2));
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{items:?}");

    let a = items.iter().find(|it| it["account_id"] == "a").unwrap();
    assert_eq!(a["source_id"], json!("claude_oauth"));
    assert_eq!(a["logged_in"], json!(false));
    assert_eq!(a["in_use"], json!(0));
    assert_eq!(a["usage"], json!(null));
    assert_eq!(a["login_pending"], json!(false));

    let b = items.iter().find(|it| it["account_id"] == "b").unwrap();
    assert_eq!(b["source_id"], json!("claude_oauth"));
    assert_eq!(b["logged_in"], json!(true));
    assert_eq!(b["in_use"], json!(1));
    assert_eq!(b["score"], json!(0.8));
    assert_eq!(b["login_pending"], json!(true));
    assert_eq!(b["usage"]["five_hour"]["utilization"], json!(0.2));
    assert_eq!(b["usage"]["source"], json!("run"));
    assert_eq!(b["cooldown"]["reason"], json!("throttled"));
    assert_eq!(b["stats"]["runs"], json!(1));
    assert_eq!(b["stats"]["done"], json!(1));
    assert_eq!(b["stats"]["input_tokens"], json!(10));
    assert_eq!(b["stats"]["output_tokens"], json!(4));
}

/// S6: celeris 側が `AccountAdminError::Unavailable` を返したら 409 `accounts_unavailable`（500 `internal` では
/// ない）。`celeris` が `[accounts]` 消滅などの理由でその場で断ったケースを模す。
#[tokio::test]
async fn account_admin_unavailable_error_maps_to_409_not_500() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(root.join("a")).unwrap();
    // celeris 側のダブル: 何を要求されても Unavailable で応える。
    let (admin_tx, mut admin_rx) = mpsc::channel::<AdminRequest>(4);
    tokio::spawn(async move {
        while let Some(req) = admin_rx.recv().await {
            if let AdminRequest::AccountCheck { reply, .. } = req {
                let _ = reply.send(Err(AccountAdminError::Unavailable(
                    "celeris could not do this right now".into(),
                )));
            }
        }
    });
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        accounts_root: Some(root),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    let app = env.router();

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts/a/check",
            &json!({}),
            &[("authorization", &auth())],
        ),
    )
    .await;
    assert_problem(&resp, 409, "accounts_unavailable");
}

// ---- ADR-0025: codex accounts in the pool (the source dimension) ----

/// `POST /llm/sources/codex_oauth/accounts` は `codex_dir` に作り、`claude_oauth` の account は `claude_dir` に作る。
/// `GET /llm/accounts` は両方を `source_id → account_id` の順に並べ、`roots` は source ごとの pool の絶対パス。
#[tokio::test]
async fn create_account_under_codex_source_uses_the_codex_root_and_list_shows_both_sources() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let claude_root = tmp.path().join("claude-accounts");
    let codex_root = tmp.path().join("codex-accounts");
    std::fs::create_dir_all(&claude_root).unwrap();
    std::fs::create_dir_all(&codex_root).unwrap();
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        accounts_root: Some(claude_root.clone()),
        codex_accounts_root: Some(codex_root.clone()),
        max_runs_per_account: 2,
        ..Default::default()
    });
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/codex_oauth/accounts",
            &json!({"id": "codex-a"}),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status, 201, "{}", resp.text());
    let v = resp.json();
    assert_eq!(v["source_id"], json!("codex_oauth"));
    assert_eq!(v["account_id"], json!("codex-a"));
    assert!(codex_root.join("codex-a").is_dir());
    assert!(!claude_root.join("codex-a").exists());

    // claude_oauth の account も作って、一覧の並びを確かめる。
    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts",
            &json!({"id": "claude-a"}),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status, 201, "{}", resp.text());
    assert_eq!(resp.json()["source_id"], json!("claude_oauth"));

    let resp = send(&app, get_with("/api/v1/llm/accounts", &headers)).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let v = resp.json();
    assert_eq!(
        v["roots"]["claude_oauth"],
        json!(claude_root.display().to_string())
    );
    assert_eq!(
        v["roots"]["codex_oauth"],
        json!(codex_root.display().to_string())
    );
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    // source_id → account_id の順（claude_oauth が codex_oauth より先）。
    assert_eq!(items[0]["source_id"], json!("claude_oauth"));
    assert_eq!(items[0]["account_id"], json!("claude-a"));
    assert_eq!(items[1]["source_id"], json!("codex_oauth"));
    assert_eq!(items[1]["account_id"], json!("codex-a"));
}

/// `codex_oauth` の pool root（`[accounts] codex_dir`）が無い構成では、`claude_oauth` の root があっても 404。
#[tokio::test]
async fn codex_source_without_codex_root_is_not_found() {
    let (env, _tmp, _root) = env_with_accounts_root();
    let app = env.router();
    let auth = auth();

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/codex_oauth/accounts",
            &json!({"id": "a"}),
            &[("authorization", &auth)],
        ),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");
}

/// source 単位の管理系は、対応する内部 adapter（`codex`）を celeris のダブルへ渡す（ADR-0025 D6）。`login/code` は
/// codex では 409 で、celeris へは届かない。
#[tokio::test]
async fn management_endpoints_forward_the_source_as_its_adapter() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let codex_root = tmp.path().join("codex-accounts");
    std::fs::create_dir_all(codex_root.join("a")).unwrap();
    let seen_adapters: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let (admin_tx, mut admin_rx) = mpsc::channel::<AdminRequest>(8);
    let seen = seen_adapters.clone();
    tokio::spawn(async move {
        while let Some(req) = admin_rx.recv().await {
            match req {
                AdminRequest::AccountCheck { adapter, reply, .. } => {
                    seen.lock().unwrap().push(adapter.to_string());
                    let _ = reply.send(Err(AccountAdminError::NotFound));
                }
                AdminRequest::AccountLoginStart { adapter, reply, .. } => {
                    seen.lock().unwrap().push(adapter.to_string());
                    let _ = reply.send(Ok(AccountLoginStartOutcome {
                        url: "https://auth.openai.com/codex/device".into(),
                        expires_at_unix: 2_000_000_000,
                        user_code: Some("ABCD-EFGHI".into()),
                    }));
                }
                AdminRequest::AccountLoginCancel { adapter, reply, .. } => {
                    seen.lock().unwrap().push(adapter.to_string());
                    let _ = reply.send(Ok(()));
                }
                AdminRequest::AccountRemove { adapter, reply, .. } => {
                    seen.lock().unwrap().push(adapter.to_string());
                    let _ = reply.send(Err(AccountAdminError::NotFound));
                }
                _ => {}
            }
        }
    });
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        codex_accounts_root: Some(codex_root),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/codex_oauth/accounts/a/check",
            &json!({}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/codex_oauth/accounts/a/login",
            &json!({}),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let v = resp.json();
    assert_eq!(v["kind"], json!("device_code"));
    assert_eq!(v["user_code"], json!("ABCD-EFGHI"));
    assert_eq!(v["url"], json!("https://auth.openai.com/codex/device"));

    let resp = send(
        &app,
        delete_with("/api/v1/llm/sources/codex_oauth/accounts/a/login", &headers),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());

    let resp = send(
        &app,
        delete_with("/api/v1/llm/sources/codex_oauth/accounts/a", &headers),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");

    let seen = seen_adapters.lock().unwrap().clone();
    assert_eq!(seen, vec!["codex", "codex", "codex", "codex"]);
}

/// ADR-0025 D5: `POST .../codex_oauth/accounts/a/login/code` は 409 `login_code_not_supported`（codex は device
/// flow でコードの提出が無い。celeris へ `AdminRequest` は送らない）。
#[tokio::test]
async fn login_code_is_not_supported_for_codex() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let codex_root = tmp.path().join("codex-accounts");
    std::fs::create_dir_all(&codex_root).unwrap();
    let (admin_tx, mut admin_rx) = mpsc::channel::<AdminRequest>(4);
    tokio::spawn(async move {
        if admin_rx.recv().await.is_some() {
            panic!("login/code for codex must not reach the admin channel");
        }
    });
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        codex_accounts_root: Some(codex_root),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    let app = env.router();
    let auth = auth();

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/codex_oauth/accounts/a/login/code",
            &json!({"code": "x"}),
            &[("authorization", &auth)],
        ),
    )
    .await;
    assert_problem(&resp, 409, "login_code_not_supported");
}

/// ADR 2026-10-08 account-concurrency-limits §(d): `AdminRequest::AccountLimitSet` を celeris の代わりに受ける
/// ダブル（存在の確認と DB への保存。本物は `celeris::accounts_admin::set_account_limit`。本物を通す試験は
/// `crates/celeris/src/daemon/api_account_limits_tests.rs`）。
fn spawn_account_limit_double(
    mut rx: mpsc::Receiver<AdminRequest>,
    db_path: std::path::PathBuf,
    implicit: Vec<String>,
) {
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            if let AdminRequest::AccountLimitSet {
                adapter,
                id,
                max_runs,
                reply,
            } = req
            {
                let result = if adapter == "implicit" && !implicit.contains(&id) {
                    Err(AccountAdminError::NotFound)
                } else {
                    task_core::SqliteStore::open(&db_path)
                        .and_then(|store| {
                            task_core::SqliteStore::set_account_limit(
                                &store, &adapter, &id, max_runs,
                            )
                        })
                        .map_err(|e| AccountAdminError::Unavailable(e.to_string()))
                };
                let _ = reply.send(result);
            }
        }
    });
}

/// 実 account の上限は `PATCH .../claude_oauth/accounts/{account_id}` で保存され、一覧にすぐ出る（DB が正本）。
#[tokio::test]
async fn pool_account_limit_is_patchable_by_source_and_account() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(root.join("a")).unwrap();
    let (admin_tx, admin_rx) = mpsc::channel::<AdminRequest>(8);
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        accounts_root: Some(root),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    spawn_account_limit_double(admin_rx, env.db_path.clone(), Vec::new());
    env.daemon_tx.send(Some(snapshot(1))).unwrap();
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];

    let resp = send(
        &app,
        patch_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts/a",
            &json!({"max_runs": 3}),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["source_id"], json!("claude_oauth"));
    assert_eq!(body["account_id"], json!("a"));
    assert_eq!(body["max_runs"], json!(3));
    assert_eq!(body["effective_max_runs"], json!(3));
    assert_eq!(body["max_runs_source"], json!("account"));

    let resp = send(&app, get_with("/api/v1/llm/accounts", &headers)).await;
    let items = resp.json()["items"].clone();
    let a = items
        .as_array()
        .unwrap()
        .iter()
        .find(|it| it["account_id"] == "a")
        .unwrap();
    assert_eq!(a["max_runs"], json!(3));
}

/// §(e): `[accounts]` が無い構成でも、スナップショットの暗黙アカウントが `GET /llm/accounts` に出て、
/// `PATCH /llm/sources/{s}/accounts/{s}` で編集できる。暗黙アカウントの作成・削除・check・login は 409
/// `implicit_account`。暗黙 source の別の account_id は 404。pool の source（root 無し）は 404。
#[tokio::test]
async fn implicit_accounts_are_listed_and_patchable_by_source_and_refused_otherwise() {
    let (admin_tx, admin_rx) = mpsc::channel::<AdminRequest>(8);
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    // ダブルは env の DB を使うので、env を作ってから受け口を繋ぐ。
    spawn_account_limit_double(
        admin_rx,
        env.db_path.clone(),
        vec!["openai_compatible:qwen".into(), "celeris".into()],
    );
    let mut snap = snapshot(1);
    // §(e): 暗黙アカウントは LLM source 単位。同じ source を使う行は 1 件を共有する。
    snap.implicit_accounts = vec![
        task_ops::daemon::ImplicitAccountLive {
            source: "openai_compatible:qwen".into(),
            models: vec!["qwen3.8-27b".into(), "qwen3.8-27b-instruct".into()],
            in_use: 2,
        },
        task_ops::daemon::ImplicitAccountLive {
            source: "celeris".into(),
            models: vec!["m1".into()],
            in_use: 0,
        },
    ];
    env.daemon_tx.send(Some(snap)).unwrap();
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];
    let qwen = "/api/v1/llm/sources/openai_compatible:qwen/accounts/openai_compatible:qwen";

    let resp = send(&app, get_with("/api/v1/llm/accounts", &headers)).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let v = resp.json();
    assert_eq!(v["roots"], json!({}));
    assert_eq!(v["max_runs_per_account"], json!(2));
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0]["source_id"], json!("openai_compatible:qwen"));
    assert_eq!(items[0]["account_id"], json!("openai_compatible:qwen"));
    assert!(items[0].get("adapter").is_none(), "{items:?}");
    assert_eq!(
        items[0]["models"],
        json!(["qwen3.8-27b", "qwen3.8-27b-instruct"])
    );
    assert_eq!(items[0]["implicit"], json!(true));
    assert_eq!(items[0]["dir"], json!(null));
    assert_eq!(items[0]["logged_in"], json!(true));
    assert_eq!(items[0]["in_use"], json!(2));
    assert_eq!(items[0]["max_runs"], json!(null));
    assert_eq!(items[0]["effective_max_runs"], json!(2));
    assert_eq!(items[0]["max_runs_source"], json!("default"));
    assert_eq!(items[0]["excluded_reason"], json!("at_capacity"));
    assert_eq!(items[1]["account_id"], json!("celeris"));
    assert_eq!(items[1]["models"], json!(["m1"]));
    assert_eq!(items[1]["effective_max_runs"], json!(2));
    assert_eq!(items[1]["max_runs_source"], json!("default"));

    let resp = send(
        &app,
        patch_json_with(qwen, &json!({"max_runs": 2}), &headers),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(body["max_runs"], json!(2));
    assert_eq!(body["effective_max_runs"], json!(2));
    assert_eq!(body["max_runs_source"], json!("account"));
    // 走っている 2 本で上限 2 に達している。
    assert_eq!(body["excluded_reason"], json!("at_capacity"));
    // GET にもすぐ出る（DB が正本。スナップショットの更新を待たない）。
    let resp = send(&app, get_with("/api/v1/llm/accounts", &headers)).await;
    assert_eq!(resp.json()["items"][0]["max_runs"], json!(2));

    // 暗黙 source の別の account_id は 404。
    let resp = send(
        &app,
        patch_json_with(
            "/api/v1/llm/sources/openai_compatible:qwen/accounts/nope",
            &json!({"max_runs": 2}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");
    // 範囲外の上限は 400。
    let resp = send(
        &app,
        patch_json_with(qwen, &json!({"max_runs": 0}), &headers),
    )
    .await;
    assert_problem(&resp, 400, "bad_request");
    // 暗黙アカウントの作成・削除・check・login は 409 implicit_account。
    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/openai_compatible:qwen/accounts",
            &json!({"id": "x"}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 409, "implicit_account");
    let resp = send(&app, delete_with(qwen, &headers)).await;
    assert_problem(&resp, 409, "implicit_account");
    let resp = send(
        &app,
        post_json_with(&format!("{qwen}/check"), &json!({}), &headers),
    )
    .await;
    assert_problem(&resp, 409, "implicit_account");
    let resp = send(
        &app,
        post_json_with(&format!("{qwen}/login"), &json!({}), &headers),
    )
    .await;
    assert_problem(&resp, 409, "implicit_account");
    // pool の root が無い claude_oauth は（暗黙でもないので）404。
    let resp = send(
        &app,
        patch_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts/a",
            &json!({"max_runs": 2}),
            &headers,
        ),
    )
    .await;
    assert_problem(&resp, 404, "account_not_found");

    let resp = send(
        &app,
        patch_json_with(qwen, &json!({"max_runs": null}), &headers),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.json()["max_runs_source"], json!("default"));
}

// ---- アーキv4 H R2 contract: handlers::accounts の処理を SCC の外へ移す前に、HTTP の契約を今の値で固定する ----

const MCP_TOKEN: &str = "celeris-mcp.0123456789abcdef";

/// 守る不変条件: token を設定した構成で、account の管理系 4 路（作成・削除・確認・上限）は bearer の無い
/// 呼び出しと未知の bearer を 401 `unauthorized`、MCP 専用 token（読み取り専用）と `Origin` 付きの呼び出しを
/// 403（`forbidden`・`origin_forbidden`）で拒み、ディレクトリも daemon も触らない。
/// 既存の試験で検出できない理由: 既存の `management_routes_all_require_a_token` は token を設定しない構成
/// （`require_admin` が Unauthenticated を 401 にする経路）だけを見る。token を設定した構成の未提示・未知の
/// bearer・MCP token・Origin の拒否は account の路では試していないので、handler の入口（認可の呼び出し）を
/// 移したときに抜けても既存の試験は通る。
#[tokio::test]
async fn management_routes_refuse_missing_unknown_mcp_and_origin_callers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(root.join("a")).unwrap();
    let (admin_tx, mut admin_rx) = mpsc::channel::<AdminRequest>(4);
    tokio::spawn(async move {
        if admin_rx.recv().await.is_some() {
            panic!("a refused caller must not reach the admin channel");
        }
    });
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        mcp_token: Some(MCP_TOKEN.into()),
        accounts_root: Some(root.clone()),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    let app = env.router();
    let unknown = "Bearer not-the-token".to_string();
    let mcp = format!("Bearer {MCP_TOKEN}");
    let admin = auth();
    let callers: Vec<(Vec<(&str, &str)>, u16, &str)> = vec![
        (vec![], 401, "unauthorized"),
        (
            vec![("authorization", unknown.as_str())],
            401,
            "unauthorized",
        ),
        (
            vec![
                ("authorization", mcp.as_str()),
                ("x-celeris-mcp-client", "01MCPCLIENTA"),
            ],
            403,
            "forbidden",
        ),
        (
            vec![
                ("authorization", admin.as_str()),
                ("origin", "https://evil.example"),
            ],
            403,
            "origin_forbidden",
        ),
    ];
    for (headers, status, code) in &callers {
        let requests = [
            post_json_with(
                "/api/v1/llm/sources/claude_oauth/accounts",
                &json!({"id": "b"}),
                headers,
            ),
            delete_with("/api/v1/llm/sources/claude_oauth/accounts/a", headers),
            post_json_with(
                "/api/v1/llm/sources/claude_oauth/accounts/a/check",
                &json!({}),
                headers,
            ),
            patch_json_with(
                "/api/v1/llm/sources/claude_oauth/accounts/a",
                &json!({"max_runs": 2}),
                headers,
            ),
        ];
        for request in requests {
            let path = request.uri().path().to_string();
            let method = request.method().clone();
            let resp = send(&app, request).await;
            assert_eq!(
                resp.status.as_u16(),
                *status,
                "{method} {path}: {}",
                resp.text()
            );
            assert_problem(&resp, *status, code);
        }
    }
    assert!(!root.join("b").exists());
    assert!(root.join("a").is_dir());
}

/// 守る不変条件: HTTP の成功の応答の形。作成は 201 と `Location` と `AccountView` の全欄（欄の集合と値）、
/// 削除は 200 と空の object `{}`（CoS の `{id, adapter, removed}` とは違う）、確認は 200 と
/// `{result, checked_at, detail}`（観測が無ければ `usage` の欄は出ない）。
/// 既存の試験で検出できない理由: 既存の作成の試験は `source_id`・`account_id`・`logged_in`・`in_use` と
/// `adapter`・`id` が無いことだけを見て、残りの欄（`dir`・`stats`・上限の 3 欄・`implicit`・`models` など）を
/// 見ない。削除の試験は status とファイルの移動だけで本文を見ず、確認の成功の応答はどの試験も見ていない。
/// 応答を組み立てる処理を CoS と共有の module へ移すと、HTTP の本文が CoS の形に変わっても既存の試験は通る。
#[tokio::test]
async fn management_success_responses_keep_their_shape() {
    use task_api::AccountCheckOutcome;
    use task_core::AccountCheckResult;
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(&root).unwrap();
    let (admin_tx, mut admin_rx) = mpsc::channel::<AdminRequest>(8);
    tokio::spawn(async move {
        while let Some(req) = admin_rx.recv().await {
            match req {
                AdminRequest::AccountCheck { reply, .. } => {
                    let _ = reply.send(Ok(AccountCheckOutcome {
                        result: AccountCheckResult::Ok,
                        detail: Some("fine".into()),
                        observation: None,
                    }));
                }
                AdminRequest::AccountRemove { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
                AdminRequest::AccountLimitSet { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
                _ => {}
            }
        }
    });
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        accounts_root: Some(root.clone()),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts",
            &json!({"id": "acct-c"}),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status, 201, "{}", resp.text());
    assert_eq!(
        resp.header("location"),
        Some("/api/v1/llm/sources/claude_oauth/accounts/acct-c")
    );
    assert_eq!(
        resp.json(),
        json!({
            "source_id": "claude_oauth",
            "account_id": "acct-c",
            "dir": root.join("acct-c").display().to_string(),
            "logged_in": false,
            "in_use": 0,
            "usage": null,
            "score": null,
            "excluded_reason": null,
            "cooldown": null,
            "last_check": null,
            "login_pending": false,
            "stats": {"runs": 0, "done": 0, "error": 0, "input_tokens": 0, "output_tokens": 0},
            "max_runs": null,
            "effective_max_runs": 2,
            "max_runs_source": "default",
            "implicit": false,
            "models": [],
        })
    );

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts/acct-c/check",
            &json!({}),
            &headers,
        ),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let v = resp.json();
    let mut keys: Vec<&str> = v
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["checked_at", "detail", "result"], "{v}");
    assert_eq!(v["result"], json!("ok"));
    assert_eq!(v["detail"], json!("fine"));
    assert!(
        time::OffsetDateTime::parse(
            v["checked_at"].as_str().expect("checked_at"),
            &time::format_description::well_known::Rfc3339
        )
        .is_ok(),
        "{v}"
    );

    let resp = send(
        &app,
        delete_with("/api/v1/llm/sources/claude_oauth/accounts/acct-c", &headers),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.json(), json!({}));
}

/// 守る不変条件: 暗黙 source に source と違う account 名を付けた削除・確認は 404 `account_not_found`（detail は
/// account 名）で、409 `implicit_account` にはならない（`resolve_pool_account` の分岐の順）。daemon へは届かない。
/// 既存の試験で検出できない理由: 既存の `implicit_accounts_are_listed_and_patchable_by_source_and_refused_otherwise`
/// は名前の不一致を PATCH（上限）でしか試さず、削除・確認は source と同じ名前（409）だけを見る。
/// `resolve_pool_account` を移すときに分岐の順が変わると、削除・確認の不一致が 409 になっても既存の試験は通る。
#[tokio::test]
async fn implicit_source_with_another_account_name_is_not_found_for_delete_and_check() {
    let (admin_tx, mut admin_rx) = mpsc::channel::<AdminRequest>(4);
    tokio::spawn(async move {
        if admin_rx.recv().await.is_some() {
            panic!("an unknown implicit account must not reach the admin channel");
        }
    });
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        max_runs_per_account: 2,
        admin_tx: Some(admin_tx),
        ..Default::default()
    });
    let mut snap = snapshot(1);
    snap.implicit_accounts = vec![task_ops::daemon::ImplicitAccountLive {
        source: "openai_compatible:qwen".into(),
        models: vec!["qwen3.8-27b".into()],
        in_use: 0,
    }];
    env.daemon_tx.send(Some(snap)).unwrap();
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];
    let other = "/api/v1/llm/sources/openai_compatible:qwen/accounts/other";

    let resp = send(&app, delete_with(other, &headers)).await;
    let problem = assert_problem(&resp, 404, "account_not_found");
    assert_eq!(problem["detail"], json!("account not found: other"));
    let resp = send(
        &app,
        post_json_with(&format!("{other}/check"), &json!({}), &headers),
    )
    .await;
    let problem = assert_problem(&resp, 404, "account_not_found");
    assert_eq!(problem["detail"], json!("account not found: other"));
}

/// 守る不変条件: HTTP の作成の本文に未知の欄があれば 400 `bad_request`（`invalid JSON body`。CoS の
/// `account.create` は同じ本文を 422 `validation` で拒む）で、ディレクトリを作らない。上限の本文の未知の欄も
/// 400 `bad_request`（`only max_runs can be patched`）。
/// 既存の試験で検出できない理由: 既存の作成の試験は不正な id と重複だけを見て、本文の読み取りの失敗を
/// 試していない。上限の試験は範囲外の値だけを見る。本文を読む道具（`read_json`）を別の module へ移したときに
/// 拒否の status や code が変わっても、既存の試験は通る。
#[tokio::test]
async fn unknown_body_fields_are_bad_requests() {
    let (env, _tmp, root) = env_with_accounts_root();
    std::fs::create_dir_all(root.join("a")).unwrap();
    let app = env.router();
    let auth = auth();
    let headers = [("authorization", auth.as_str())];

    let resp = send(
        &app,
        post_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts",
            &json!({"id": "y", "adapter": "codex"}),
            &headers,
        ),
    )
    .await;
    let problem = assert_problem(&resp, 400, "bad_request");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.starts_with("invalid JSON body:")),
        "{problem}"
    );
    assert!(!root.join("y").exists());

    let resp = send(
        &app,
        patch_json_with(
            "/api/v1/llm/sources/claude_oauth/accounts/a",
            &json!({"max_runs": 2, "note": "x"}),
            &headers,
        ),
    )
    .await;
    let problem = assert_problem(&resp, 400, "bad_request");
    assert_eq!(
        problem["detail"],
        json!("unknown field `note`; only max_runs can be patched")
    );
}
