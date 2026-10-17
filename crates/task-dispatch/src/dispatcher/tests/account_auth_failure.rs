//! ADR 2026-10-17 account-auth-failure: OAuth 切れ（認証失敗）のアカウントは routing から外れ、再ログインか
//! check の成功まで戻らない。認証失敗は task の attempts / requeue 上限 / reviewer の失敗回数を消費せず、
//! 人へ通知する。偽のアダプタと一時ディレクトリだけ（実 OAuth には触れない）。

use super::*;

const AUTH_MESSAGE: &str =
    "Failed to authenticate: OAuth session expired and could not be refreshed";

/// `broken` に入っているアカウントの run は認証失敗、それ以外は done。走ったアカウントを `ran` に残す。
#[derive(Clone)]
struct OauthAdapter {
    broken: Arc<StdMutex<Vec<String>>>,
    ran: Arc<StdMutex<Vec<String>>>,
    account: Option<String>,
}

impl OauthAdapter {
    fn new(broken: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            broken: Arc::new(StdMutex::new(
                broken.iter().map(|s| s.to_string()).collect(),
            )),
            ran: Arc::new(StdMutex::new(Vec::new())),
            account: None,
        })
    }
}

#[async_trait]
impl WorkerAdapter for OauthAdapter {
    fn id(&self) -> &str {
        "claude-code"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::write(req.workspace.join("touched"), "1").unwrap();
        let account = self.account.clone().unwrap_or_default();
        self.ran.lock().unwrap().push(account.clone());
        if self.broken.lock().unwrap().contains(&account) {
            return Err(AdapterError::AuthFailed(AUTH_MESSAGE.into()));
        }
        Ok(done_outcome())
    }
    fn with_model(&self, _model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(self.clone()))
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let account = extra
            .iter()
            .find(|(k, _)| k == AccountAdapter::ClaudeCode.env_var())
            .and_then(|(_, v)| std::path::Path::new(v).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .or_else(|| self.account.clone());
        Some(Arc::new(Self {
            account,
            ..self.clone()
        }))
    }
}

fn account_live(d: &mut Dispatcher, id: &str) -> task_ops::daemon::AccountLive {
    let (_, _, _, items) = d.accounts_snapshot();
    items.into_iter().find(|a| a.id == id).expect("account")
}

fn auth_notices(store: &Arc<dyn TaskStore>) -> Vec<task_core::Notice> {
    store
        .notice_list(&NoticeQuery {
            kinds: vec![NoticeKind::BadNews],
            ..NoticeQuery::default()
        })
        .unwrap()
        .items
}

fn task(dir: &std::path::Path) -> Task {
    new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    )
}

/// 受け入れ 0/1/2: 認証失敗の run は task を失敗にせず、そのアカウントを外して別アカウントで終わらせ、
/// 一覧（cooldown・logged_in）に反映し、通知を 1 回出す。check の成功（または再ログイン）で戻る。
#[tokio::test]
async fn auth_failure_excludes_the_account_reroutes_notifies_and_recovers() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = OauthAdapter::new(&["a"]);
    // max_requeues = 0: 従来なら 1 回目の失敗で通常の失敗（attempts 消費）に落ちる設定。
    let mut d = pool_dispatcher_with_requeues(
        store.clone(),
        adapter.clone(),
        None,
        accounts.path().to_path_buf(),
        2,
        4,
        0,
    );
    let t = task(ws.path());
    store.insert(&t).unwrap();
    run_until_task_terminal(&mut d, &store, t.id).await;

    let task_row = store.get(t.id).unwrap().unwrap();
    assert_eq!((task_row.status, task_row.attempts), (Status::Done, 0));
    assert_eq!(*adapter.ran.lock().unwrap(), vec!["a", "b"]);

    let a = account_live(&mut d, "a");
    assert!(!a.logged_in, "{a:?}");
    assert_eq!(a.excluded_reason.as_deref(), Some("cooldown"));
    assert_eq!(
        a.cooldown.as_ref().map(|c| c.reason.as_str()),
        Some("auth_failed")
    );
    assert!(account_live(&mut d, "b").logged_in);

    let notices = auth_notices(&store);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].title.contains("a (claude-code)"), "{notices:?}");

    // 同じ失敗の再発では通知を重ねない（保持中）。cooldown は 5 分の既定で失効しない。
    d.record_account_check(
        AccountAdapter::ClaudeCode,
        "a",
        "auth_failed",
        Some(AUTH_MESSAGE.into()),
        None,
    );
    assert_eq!(auth_notices(&store).len(), 1);
    let a = account_live(&mut d, "a");
    assert_eq!(
        a.last_check.as_ref().map(|c| c.result.as_str()),
        Some("auth_failed")
    );
    assert!(!a.logged_in);

    // check の成功で戻る。
    d.record_account_check(
        AccountAdapter::ClaudeCode,
        "a",
        "ok",
        Some("ok".into()),
        None,
    );
    let a = account_live(&mut d, "a");
    assert!(
        a.logged_in && a.excluded_reason.is_none() && a.cooldown.is_none(),
        "{a:?}"
    );

    // 再ログインでも戻る。
    d.record_account_check(AccountAdapter::ClaudeCode, "a", "auth_failed", None, None);
    assert!(!account_live(&mut d, "a").logged_in);
    d.record_account_login(AccountAdapter::ClaudeCode, "a");
    let a = account_live(&mut d, "a");
    assert!(a.logged_in && a.excluded_reason.is_none(), "{a:?}");
}

/// 受け入れ 2: 外れたアカウントしか無くても task は failed にならず（requeue 上限 0 でも）待機し、
/// 再ログインの後に完了する。
#[tokio::test]
async fn auth_failure_with_no_other_account_waits_for_relogin_instead_of_failing() {
    let accounts = tempfile::tempdir().unwrap();
    let only = accounts.path().join("a");
    std::fs::create_dir_all(&only).unwrap();
    std::fs::write(only.join(".credentials.json"), "{}").unwrap();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = OauthAdapter::new(&["a"]);
    let mut d = pool_dispatcher_with_requeues(
        store.clone(),
        adapter.clone(),
        None,
        accounts.path().to_path_buf(),
        2,
        4,
        0,
    );
    let t = task(ws.path());
    store.insert(&t).unwrap();
    for _ in 0..40 {
        d.tick().unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let row = store.get(t.id).unwrap().unwrap();
    assert!(!row.status.is_terminal(), "{row:?}");
    assert_eq!(row.attempts, 0);
    assert_eq!(
        adapter.ran.lock().unwrap().len(),
        1,
        "no re-pick while held"
    );

    adapter.broken.lock().unwrap().clear();
    d.record_account_login(AccountAdapter::ClaudeCode, "a");
    run_until_task_terminal(&mut d, &store, t.id).await;
    assert_eq!(store.get(t.id).unwrap().unwrap().attempts, 0);
}
