//! ADR-0024 D5〜D7 / ADR-0025 D4〜D5: celeris 側のアカウント管理（手動確認・ログインの中継）。
//!
//! task-api からは `task_api::AdminRequest` 経由でだけ触れる（DESIGN §5.10 の境界。ワーカー・子プロセスの
//! 起動は celeris 側）。長くかかる操作（確認・ログイン）は `tick_loop` をブロックしないよう `tokio::spawn` する。
//! 結果は 2 つの経路で返る: 呼び出し元（task-api）への `oneshot` 応答と、`Dispatcher`（`AccountBook`・
//! `login_pending`）へ反映するための `AccountAdminEvent` チャネル。
//!
//! claude-code（ADR-0024 D6/D7）と codex（ADR-0025 D4/D5）はログインの流儀が違うので、`LoginSessions` を
//! アダプタごとに別のマップに分ける（claude-code はコード貼り付け式、codex は端末を跨いだデバイス認証で
//! `login/code` を使わない）。確認は共通の語彙（`AccountCheckResult` 等）に写して扱う。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use task_api::{
    AccountAdminError, AccountCheckOutcome, AccountLoginCodeOutcome, AccountLoginStartOutcome,
};
use task_core::{AccountAdapter, RateLimitObservation};
use task_worker::{
    AccountCheckResult, CodexLoginSession, LoginOutcome, LoginSession, check_account,
    check_account_codex, check_account_opencode_go, start_login, start_login_codex,
};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::config::Config;

/// D6 の確認 1 回あたりの上限（`AdapterError` 等の分類は `task_worker::claude_account`/`codex_account` 側で行う）。
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
/// D7: 認可 URL が出るまでの上限（claude-code）。
pub const LOGIN_URL_TIMEOUT: Duration = Duration::from_secs(15);
/// D7: コード送信後、終了を待つ上限（claude-code）。
pub const LOGIN_CODE_WAIT: Duration = Duration::from_secs(30);
/// D7: 進行中のログインを打ち切るまでの時間（claude-code）。
pub const LOGIN_EXPIRY: Duration = Duration::from_secs(600);
/// ADR-0025 D5: 認可 URL とコードが出るまでの上限（codex）。
pub const LOGIN_URL_TIMEOUT_CODEX: Duration = Duration::from_secs(15);
/// ADR-0025 D5: 人が別デバイスでコードを入力し終わるのを待つ上限（codex の一回限りのコードの実際の有効期限）。
pub const LOGIN_EXPIRY_CODEX: Duration = Duration::from_secs(900);

/// 進行中のログイン中継（アカウントごとに高々 1 つ）。celeris の生存期間だけ持つ（プロセス終了で Drop → kill）。
pub type LoginSessions = Arc<Mutex<HashMap<String, LoginSession>>>;
/// ADR-0025 D5: codex 版（別の型・別の流儀なので別のマップに分ける）。
pub type CodexLoginSessions = Arc<Mutex<HashMap<String, CodexLoginSession>>>;

pub fn new_sessions() -> LoginSessions {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn new_codex_sessions() -> CodexLoginSessions {
    Arc::new(Mutex::new(HashMap::new()))
}

/// `handle_account_admin_request` の結果を `Dispatcher` に反映するための通知（`tick_loop` が受け取る）。
pub enum AccountAdminEvent {
    /// D6: 確認結果を `AccountBook` に記録する（`source = "check"`）。
    Checked {
        adapter: AccountAdapter,
        id: String,
        result: String,
        detail: Option<String>,
        observation: Option<RateLimitObservation>,
    },
    /// D7: 進行中のログインの有無（開始・コード送信・打ち切りのたびに送る）。
    LoginPending {
        adapter: AccountAdapter,
        id: String,
        pending: bool,
    },
}

/// 10 分を超えたログイン中継を打ち切り、打ち切ったアカウント id を返す（`tick_loop` が毎 tick 呼ぶ。claude-code）。
///
/// B1: `tick_loop` 自身が drain する `account_tx`（容量 16）へ、この関数の中から `await` で送ってはいけない
/// （`tick_loop` は `select!` に戻るまでチャネルを読まないので、詰まると `tick_loop` 自身がここで永遠に
/// ブロックしてデッドロックする）。そのため、ここではチャネルを一切使わず、打ち切った id を呼び出し側
/// （`tick_loop`）に返すだけにし、`Dispatcher::set_account_login_pending` は呼び出し側が直接呼ぶ。
///
/// `expiry` は N12（テストで注入できるように 10 分固定にしない）: 本番は `LOGIN_EXPIRY` を渡す。
pub async fn expire_stale_logins(sessions: &LoginSessions, expiry: Duration) -> Vec<String> {
    let mut guard = sessions.lock().await;
    let expired: Vec<String> = guard
        .iter()
        .filter(|(_, s)| s.started_at.elapsed() >= expiry)
        .map(|(id, _)| id.clone())
        .collect();
    for id in &expired {
        if let Some(session) = guard.remove(id) {
            tracing::info!(who = "admin", op = "account_login_expire", account_id = %id, adapter = "claude-code", "admin: login session expired");
            session.cancel();
        }
    }
    drop(guard);
    expired
}

/// ADR-0025 D5 / B1 と同じ理由: codex の進行中のログインを 15 分で打ち切る（`tick_loop` が毎 tick 呼ぶ）。
pub async fn expire_stale_codex_logins(
    sessions: &CodexLoginSessions,
    expiry: Duration,
) -> Vec<String> {
    let mut guard = sessions.lock().await;
    let expired: Vec<String> = guard
        .iter()
        .filter(|(_, s)| s.started_at.elapsed() >= expiry)
        .map(|(id, _)| id.clone())
        .collect();
    for id in &expired {
        if let Some(session) = guard.remove(id) {
            tracing::info!(who = "admin", op = "account_login_expire", account_id = %id, adapter = "codex", "admin: login session expired");
            session.cancel();
        }
    }
    drop(guard);
    expired
}

/// ADR-0025 D5: codex は標準入力を使わず、人が別デバイスでコードを入力し終わるのを待つだけ。子プロセスの
/// 終了を非同期にブロックせず毎 tick ポーリングし、終わっていたセッションを取り除いて `(id, ok)` を返す
/// （`tick_loop` が毎 tick 呼ぶ。`expire_stale_codex_logins` と同じ理由でチャネルを使わない）。
pub async fn poll_codex_logins(sessions: &CodexLoginSessions) -> Vec<(String, bool)> {
    let mut guard = sessions.lock().await;
    let mut finished = Vec::new();
    let ids: Vec<String> = guard.keys().cloned().collect();
    for id in ids {
        let Some(session) = guard.get_mut(&id) else {
            continue;
        };
        if let Some(result) = session.try_finished() {
            guard.remove(&id);
            let ok = result.result == LoginOutcome::Ok;
            tracing::info!(who = "admin", op = "account_login_finished", account_id = %id, adapter = "codex", ok, "admin: codex device login finished");
            finished.push((id, ok));
        }
    }
    finished
}

/// `[adapters.claude_code].command` と、そのアダプタの env（**プロバイダの env ではない**。ADR-0024 D6/D7）。
fn claude_command_and_env(config: &Config) -> (String, Vec<(String, String)>) {
    let base = &config.adapters.claude_code;
    let mut env: Vec<(String, String)> = base
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    env.sort();
    (base.command.clone(), env)
}

/// `[adapters.codex].command` と、そのアダプタの env（ADR-0025 D4/D5）。
fn codex_command_and_env(config: &Config) -> (String, Vec<(String, String)>) {
    let base = &config.adapters.codex;
    let mut env: Vec<(String, String)> = base
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    env.sort();
    (base.command.clone(), env)
}

/// アカウントディレクトリを解決する。`id` が無効、`[accounts]` にそのアダプタの根が無い、ディレクトリが
/// 無ければエラー（ADR-0025 D1）。
fn account_dir(
    config: &Config,
    adapter: AccountAdapter,
    id: &str,
) -> Result<PathBuf, AccountAdminError> {
    let accounts = config.accounts.as_ref().ok_or_else(|| {
        AccountAdminError::Unavailable("the [accounts] section is not configured".to_string())
    })?;
    if !task_dispatch::valid_account_id(id) {
        return Err(AccountAdminError::NotFound);
    }
    let root = accounts.root_for(adapter).ok_or_else(|| {
        AccountAdminError::Unavailable(format!(
            "the [accounts] section has no root configured for adapter {adapter}"
        ))
    })?;
    let dir = root.join(id);
    if !dir.is_dir() {
        return Err(AccountAdminError::NotFound);
    }
    Ok(dir)
}

/// S2+S8 (ADR-0024 D5 / ADR-0025 D5): `DELETE /accounts/{id}`。celeris 側で行う（スナップショットではなく、
/// ディスパッチャの権威ある `account_in_use`（running/reviewing を直接見る）でレースなく判定できるため）。
/// cheap な fs 操作なので `tick_loop` からは spawn せずそのまま呼ぶ。進行中のログインがあれば止め、
/// `.removed/<id>-<unix>` へ移し、`AccountBook` からもこのアカウントの記録を消す。
pub async fn remove_account(
    config: &Config,
    dispatcher: &mut task_dispatch::Dispatcher,
    sessions: &LoginSessions,
    codex_sessions: &CodexLoginSessions,
    adapter: AccountAdapter,
    id: &str,
) -> Result<(), AccountAdminError> {
    let dir = account_dir(config, adapter, id)?;
    if dispatcher.account_in_use(adapter, id) > 0 {
        return Err(AccountAdminError::InUse);
    }
    match adapter {
        AccountAdapter::ClaudeCode => {
            if let Some(session) = sessions.lock().await.remove(id) {
                session.cancel();
            }
        }
        AccountAdapter::Codex => {
            if let Some(session) = codex_sessions.lock().await.remove(id) {
                session.cancel();
            }
        }
        // opencode go にはログイン中継が無い（auth.json を置くだけ。ADR 2026-10-06 D2）。
        AccountAdapter::OpencodeGo => {}
    }
    dispatcher.set_account_login_pending(adapter, id, false);
    // `account_dir` は `[accounts]` とそのアダプタの根の存在を既に確かめている。
    let accounts = config.accounts.as_ref().ok_or_else(|| {
        AccountAdminError::Unavailable("the [accounts] section is not configured".to_string())
    })?;
    let root = accounts.root_for(adapter).ok_or_else(|| {
        AccountAdminError::Unavailable(format!("no root configured for adapter {adapter}"))
    })?;
    let removed_dir = root.join(".removed");
    // 移した先にも認証ファイルが残るので、アカウントのディレクトリと同じく本人だけが読める権限にする。
    {
        use std::os::unix::fs::DirBuilderExt;
        match std::fs::DirBuilder::new().mode(0o700).create(&removed_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(AccountAdminError::Unavailable(format!(
                    "failed to create .removed dir: {e}"
                )));
            }
        }
    }
    let dest = removed_dir.join(format!("{id}-{}", unix_now()));
    std::fs::rename(&dir, &dest)
        .map_err(|e| AccountAdminError::Unavailable(format!("failed to move account dir: {e}")))?;
    dispatcher.remove_account_book_entry(adapter, id);
    Ok(())
}

/// Codex の確認は推論を消費しないので、active の tick から定期的に更新する。
#[derive(Default)]
pub struct UsageChecks {
    last: std::collections::HashMap<String, std::time::Instant>,
}

impl UsageChecks {
    /// 推論を使わない確認（codex の app-server、opencode go の usage endpoint）を idle の logged-in
    /// account に 300 秒おきに行う（ADR-0049、ADR 2026-10-06 D2）。
    pub fn poll(
        &mut self,
        config: &Config,
        dispatcher: &task_dispatch::Dispatcher,
        events: mpsc::Sender<AccountAdminEvent>,
    ) {
        let Some(accounts) = config.accounts.as_ref() else {
            return;
        };
        let now = std::time::Instant::now();
        for adapter in [AccountAdapter::Codex, AccountAdapter::OpencodeGo] {
            let Some(root) = accounts.root_for(adapter) else {
                continue;
            };
            for account in task_dispatch::accounts::scan_accounts(root, adapter) {
                // 同じ id でもアダプタが違えば別のアカウント（codex は従来どおり id そのまま、他は `<adapter>:<id>`）。
                let key = if adapter == AccountAdapter::Codex {
                    account.id.clone()
                } else {
                    format!("{adapter}:{}", account.id)
                };
                if !account.logged_in
                    || dispatcher.account_in_use(adapter, &account.id) > 0
                    || dispatcher.account_login_pending(adapter, &account.id)
                    || self
                        .last
                        .get(&key)
                        .is_some_and(|last| now.duration_since(*last) < Duration::from_secs(300))
                {
                    continue;
                }
                self.last.insert(key, now);
                let (reply, _rx) = oneshot::channel();
                spawn_check(config, adapter, account.id, events.clone(), reply);
            }
        }
    }
}

/// D6 / ADR-0025 D4: `POST /accounts/{id}/check`。設定は呼び出しごとに読み直さない（`config` は tick_loop が
/// 持つ最新の写し）。claude-code は `[accounts].check_model` を使い、codex はモデルを指定しない。
pub fn spawn_check(
    config: &Config,
    adapter: AccountAdapter,
    id: String,
    events: mpsc::Sender<AccountAdminEvent>,
    reply: oneshot::Sender<Result<AccountCheckOutcome, AccountAdminError>>,
) {
    let dir = match account_dir(config, adapter, &id) {
        Ok(dir) => dir,
        Err(e) => {
            let _ = reply.send(Err(e));
            return;
        }
    };
    match adapter {
        AccountAdapter::ClaudeCode => {
            let check_model = config
                .accounts
                .as_ref()
                .map(|a| a.check_model.clone())
                .unwrap_or_default();
            let (command, env) = claude_command_and_env(config);
            tokio::spawn(async move {
                let check = check_account(&command, &dir, &check_model, CHECK_TIMEOUT, &env).await;
                finish_check(
                    adapter,
                    id,
                    check.result,
                    check.detail,
                    check.observation,
                    events,
                    reply,
                )
                .await;
            });
        }
        AccountAdapter::OpencodeGo => {
            let usage_url = config
                .accounts
                .as_ref()
                .map(|a| a.opencode_go_usage_url.clone())
                .unwrap_or_default();
            tokio::spawn(async move {
                let check = check_account_opencode_go(&dir, &usage_url, CHECK_TIMEOUT).await;
                finish_check(
                    adapter,
                    id,
                    check.result,
                    check.detail,
                    check.observation,
                    events,
                    reply,
                )
                .await;
            });
        }
        AccountAdapter::Codex => {
            let (command, env) = codex_command_and_env(config);
            tokio::spawn(async move {
                let check = check_account_codex(&command, &dir, CHECK_TIMEOUT, &env).await;
                finish_check(
                    adapter,
                    id,
                    check.result,
                    check.detail,
                    check.observation,
                    events,
                    reply,
                )
                .await;
            });
        }
    }
}

async fn finish_check(
    adapter: AccountAdapter,
    id: String,
    result: AccountCheckResult,
    detail: Option<String>,
    observation: Option<RateLimitObservation>,
    events: mpsc::Sender<AccountAdminEvent>,
    reply: oneshot::Sender<Result<AccountCheckOutcome, AccountAdminError>>,
) {
    let result_name = account_check_result_name(&result).to_string();
    let outcome = AccountCheckOutcome {
        result: map_check_result(result),
        detail: detail.clone(),
        observation: observation.clone(),
    };
    let _ = events
        .send(AccountAdminEvent::Checked {
            adapter,
            id,
            result: result_name,
            detail,
            observation,
        })
        .await;
    let _ = reply.send(Ok(outcome));
}

/// D7 / ADR-0025 D5: `POST /accounts/{id}/login`。既に進行中のセッションがあれば止めてから新しく始める。
/// claude-code は `claude auth login`（コード貼り付け式）、codex は `codex login --device-auth`
/// （デバイス認証。`user_code` を返す）。
#[allow(clippy::too_many_arguments)]
pub fn spawn_login_start(
    config: &Config,
    sessions: LoginSessions,
    codex_sessions: CodexLoginSessions,
    adapter: AccountAdapter,
    id: String,
    events: mpsc::Sender<AccountAdminEvent>,
    reply: oneshot::Sender<Result<AccountLoginStartOutcome, AccountAdminError>>,
) {
    let dir = match account_dir(config, adapter, &id) {
        Ok(dir) => dir,
        Err(e) => {
            let _ = reply.send(Err(e));
            return;
        }
    };
    match adapter {
        AccountAdapter::ClaudeCode => {
            let (command, env) = claude_command_and_env(config);
            tokio::spawn(async move {
                let had_old = {
                    let mut guard = sessions.lock().await;
                    match guard.remove(&id) {
                        Some(old) => {
                            old.cancel();
                            true
                        }
                        None => false,
                    }
                };
                match start_login(&command, &dir, &env, LOGIN_URL_TIMEOUT).await {
                    Ok(session) => {
                        let url = session.url.clone();
                        let expires_at_unix = unix_now() + LOGIN_EXPIRY.as_secs() as i64;
                        sessions.lock().await.insert(id.clone(), session);
                        let _ = events
                            .send(AccountAdminEvent::LoginPending {
                                adapter,
                                id: id.clone(),
                                pending: true,
                            })
                            .await;
                        let _ = reply.send(Ok(AccountLoginStartOutcome {
                            url,
                            expires_at_unix,
                            user_code: None,
                        }));
                    }
                    Err(e) => {
                        // B2: 古いセッションを止めた後に新しい start_login 自体が失敗したら、login_pending を false に
                        // 戻す（そのままだと `sessions` には無いのに GUI には「進行中」が残り続ける）。
                        if had_old {
                            let _ = events
                                .send(AccountAdminEvent::LoginPending {
                                    adapter,
                                    id: id.clone(),
                                    pending: false,
                                })
                                .await;
                        }
                        // D5: URL・認可コードは失敗時もログには出さない（アカウント id と操作名だけ）。
                        let _ = reply.send(Err(AccountAdminError::LoginFailed(e.to_string())));
                    }
                }
            });
        }
        AccountAdapter::OpencodeGo => {
            // opencode go は中継するログインが無い。人が `opencode auth login`（`XDG_DATA_HOME=<account dir>`）で
            // `opencode/auth.json` を作る（ADR 2026-10-06 D2）。
            let _ = reply.send(Err(AccountAdminError::LoginFailed(
                "opencode-go accounts have no login relay; create opencode/auth.json in the account dir (opencode auth login with XDG_DATA_HOME set)"
                    .to_string(),
            )));
        }
        AccountAdapter::Codex => {
            let (command, env) = codex_command_and_env(config);
            tokio::spawn(async move {
                let had_old = {
                    let mut guard = codex_sessions.lock().await;
                    match guard.remove(&id) {
                        Some(old) => {
                            old.cancel();
                            true
                        }
                        None => false,
                    }
                };
                match start_login_codex(&command, &dir, &env, LOGIN_URL_TIMEOUT_CODEX).await {
                    Ok(session) => {
                        let url = session.url.clone();
                        let user_code = session.user_code.clone();
                        let expires_at_unix = unix_now() + LOGIN_EXPIRY_CODEX.as_secs() as i64;
                        codex_sessions.lock().await.insert(id.clone(), session);
                        let _ = events
                            .send(AccountAdminEvent::LoginPending {
                                adapter,
                                id: id.clone(),
                                pending: true,
                            })
                            .await;
                        let _ = reply.send(Ok(AccountLoginStartOutcome {
                            url,
                            expires_at_unix,
                            user_code: Some(user_code),
                        }));
                    }
                    Err(e) => {
                        if had_old {
                            let _ = events
                                .send(AccountAdminEvent::LoginPending {
                                    adapter,
                                    id: id.clone(),
                                    pending: false,
                                })
                                .await;
                        }
                        let _ = reply.send(Err(AccountAdminError::LoginFailed(e.to_string())));
                    }
                }
            });
        }
    }
}

/// D7: `POST /accounts/{id}/login/code`。claude-code のみ（task-api が codex を 409 で弾く）。進行中の
/// セッションが無ければ `LoginNotStarted`。
pub fn spawn_login_code(
    sessions: LoginSessions,
    id: String,
    code: String,
    events: mpsc::Sender<AccountAdminEvent>,
    reply: oneshot::Sender<Result<AccountLoginCodeOutcome, AccountAdminError>>,
) {
    tokio::spawn(async move {
        let session = sessions.lock().await.remove(&id);
        let Some(session) = session else {
            let _ = reply.send(Err(AccountAdminError::LoginNotStarted));
            return;
        };
        let result = session.submit_code(&code, LOGIN_CODE_WAIT).await;
        let _ = events
            .send(AccountAdminEvent::LoginPending {
                adapter: AccountAdapter::ClaudeCode,
                id,
                pending: false,
            })
            .await;
        let _ = reply.send(Ok(AccountLoginCodeOutcome {
            ok: result.result == LoginOutcome::Ok,
            detail: result.detail,
        }));
    });
}

/// D7 / 3.35 / ADR-0025 D5: `DELETE /accounts/{id}/login`。進行中でなければ何もしない（エラーにしない）。
pub fn spawn_login_cancel(
    sessions: LoginSessions,
    codex_sessions: CodexLoginSessions,
    adapter: AccountAdapter,
    id: String,
    events: mpsc::Sender<AccountAdminEvent>,
    reply: oneshot::Sender<Result<(), AccountAdminError>>,
) {
    tokio::spawn(async move {
        let cancelled = match adapter {
            AccountAdapter::ClaudeCode => sessions.lock().await.remove(&id).is_some_and(|s| {
                s.cancel();
                true
            }),
            AccountAdapter::Codex => codex_sessions.lock().await.remove(&id).is_some_and(|s| {
                s.cancel();
                true
            }),
            AccountAdapter::OpencodeGo => false,
        };
        if cancelled {
            let _ = events
                .send(AccountAdminEvent::LoginPending {
                    adapter,
                    id,
                    pending: false,
                })
                .await;
        }
        let _ = reply.send(Ok(()));
    });
}

fn unix_now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn account_check_result_name(result: &AccountCheckResult) -> &'static str {
    match result {
        AccountCheckResult::Ok => "ok",
        AccountCheckResult::AuthFailed => "auth_failed",
        AccountCheckResult::Throttled => "throttled",
        AccountCheckResult::SpawnFailed => "spawn_failed",
    }
}

fn map_check_result(result: AccountCheckResult) -> task_api::ProviderCheckResult {
    match result {
        AccountCheckResult::Ok => task_api::ProviderCheckResult::Ok,
        AccountCheckResult::AuthFailed => task_api::ProviderCheckResult::AuthFailed,
        AccountCheckResult::Throttled => task_api::ProviderCheckResult::Throttled,
        AccountCheckResult::SpawnFailed => task_api::ProviderCheckResult::SpawnFailed,
    }
}

#[cfg(test)]
#[path = "accounts_admin/tests.rs"]
mod tests;
