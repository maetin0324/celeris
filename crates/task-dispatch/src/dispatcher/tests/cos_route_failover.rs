//! ADR 2026-11-24-planner-supply-failure-cooldown D5 / D7 の 6・7: CoS の run が供給側の失敗（ここでは
//! ログインの期限切れ）で終わったら、その account を worker の run と同じ owner で cooldown にし、同じ chat run を
//! `cos.route_candidates` の全体から選び直す。候補が 1 件（source 1 つ・account 2 つ）でも、もう一方の account で
//! 起動する。2026-11-24 の事象では候補の番号で読み飛ばしたため 2 回目の候補列が空になり、
//! "no route_defaults candidates are declared" で閉じた（既存の試験に CoS の代替経路の試験は無かった）。
//! 偽の adapter・固定の時計・task の join だけを使う（sleep・負荷なし）。

use super::cos_chat_launch::{install_cos_launch, join, post, runs, thread};
use super::*;
use crate::accounts::AUTH_FAILED_HOLD_SECS;
use task_core::chat::{ChatMessageQuery, ChatMessageRole, ChatRun, ChatRunState};

const AUTH_MESSAGE: &str =
    "Failed to authenticate: this login reached its time limit and cannot be renewed";

/// `broken` の account の run はログインの期限切れ（`AdapterError::AuthFailed`）、それ以外は done。
/// 走った `(run id, account)` を `ran` に残す（通知を拾う受信箱の仕分けの run も同じ adapter で走る）。
/// account は route が重ねた環境変数の login dir の名前で知る。
#[derive(Clone)]
struct ExpiredLoginAdapter {
    broken: Vec<String>,
    ran: Ran,
    account: Option<String>,
}

type Ran = Arc<StdMutex<Vec<(String, String)>>>;

#[async_trait]
impl WorkerAdapter for ExpiredLoginAdapter {
    fn id(&self) -> &str {
        "claude-code"
    }
    async fn run(
        &self,
        _req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let account = self.account.clone().unwrap_or_default();
        self.ran
            .lock()
            .unwrap()
            .push((run_id.to_owned(), account.clone()));
        if self.broken.contains(&account) {
            return Err(AdapterError::AuthFailed(AUTH_MESSAGE.into()));
        }
        Ok(done_outcome())
    }
    fn configure(&self, settings: &task_worker::AdapterSettings) -> task_worker::Configured {
        settings.configured(
            &[
                task_worker::AdapterSetting::Model,
                task_worker::AdapterSetting::Env,
            ],
            || {
                let account = settings
                    .env
                    .iter()
                    .find(|(k, _)| k == AccountAdapter::ClaudeCode.env_var())
                    .and_then(|(_, v)| std::path::Path::new(v).file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .or_else(|| self.account.clone());
                Arc::new(Self {
                    account,
                    ..self.clone()
                })
            },
        )
    }
}

struct Fixture {
    _dir: crate::test_support::WritableTempDir,
    _accounts: tempfile::TempDir,
    store: Arc<SqliteStore>,
    d: Dispatcher,
    ran: Ran,
    now: i64,
}

impl Fixture {
    /// The accounts the attempts of chat run `run` ran on, in order.
    fn accounts_of(&self, run: &str) -> Vec<String> {
        self.ran
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| id == run)
            .map(|(_, account)| account.clone())
            .collect()
    }
}

/// OAuth の source 1 つ（account `a`・`b`、どちらもログイン済み）が CoS の唯一の候補。時計は固定。
fn fixture(broken: &[&str]) -> Fixture {
    let dir = crate::test_support::WritableTempDir::new();
    let accounts = accounts_fixture();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).expect("store"));
    let ran = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(ExpiredLoginAdapter {
        broken: broken.iter().map(|s| s.to_string()).collect(),
        ran: Arc::clone(&ran),
        account: None,
    });
    let mut d = pool_dispatcher(
        store.clone(),
        adapter,
        None,
        accounts.path().to_path_buf(),
        2,
        4,
    );
    let at = OffsetDateTime::now_utc();
    let now = at.unix_timestamp();
    d.test_hooks.now = Some(Arc::new(StdMutex::new(at)));
    d.probes.set_now_unix(Arc::new(move || now));
    install_cos_launch(&mut d, dir.path(), &store, 2);
    assert_eq!(
        d.routing.registry().cos_route_candidates.len(),
        1,
        "the incident's shape: one CoS candidate, two accounts of its source"
    );
    Fixture {
        _dir: dir,
        _accounts: accounts,
        store,
        d,
        ran,
        now,
    }
}

fn account(d: &mut Dispatcher, id: &str) -> task_ops::daemon::AccountLive {
    let (_, _, _, items) = d.accounts_snapshot();
    items.into_iter().find(|a| a.id == id).expect("account")
}

fn assert_auth_failed_held(f: &mut Fixture, id: &str) {
    let live = account(&mut f.d, id);
    let cooldown = live.cooldown.as_ref().expect("cooldown");
    assert_eq!(cooldown.reason, "auth_failed", "{live:?}");
    assert!(cooldown.until >= f.now + AUTH_FAILED_HOLD_SECS, "{live:?}");
    assert_eq!(
        live.excluded_reason.as_deref(),
        Some("cooldown"),
        "{live:?}"
    );
    assert!(
        !live.logged_in,
        "an auth failure reads as re-login needed: {live:?}"
    );
}

fn auth_notices(store: &SqliteStore) -> Vec<task_core::Notice> {
    store
        .notice_list(&NoticeQuery {
            kinds: vec![NoticeKind::BadNews],
            ..NoticeQuery::default()
        })
        .unwrap()
        .items
}

fn system_texts(store: &SqliteStore, thread: &str) -> Vec<String> {
    store
        .chat_message_list(
            thread,
            &ChatMessageQuery {
                limit: Some(200),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .filter(|m| m.role == ChatMessageRole::System)
        .map(|m| m.text)
        .collect()
}

/// 1 つの chat run を、起動・join を繰り返して終端まで進める（試行ごとに 1 tick・1 join）。
async fn run_to_end(f: &mut Fixture, thread: &str, attempts: usize) -> ChatRun {
    f.d.tick_cos_chat_launch();
    for _ in 0..attempts {
        join(&mut f.d, thread).await;
        f.d.tick_cos_chat_launch();
    }
    let mut all = runs(&f.store, thread);
    assert_eq!(all.len(), 1, "the retry reuses the same chat run: {all:?}");
    all.pop().expect("run")
}

/// D7 の 6: 1 つ目の account の認証の失敗 → その account は auth_failed の cooldown（再ログインまで保持）・
/// 人への通知 1 件、同じ chat run は同じ source の別の account で起動して完了する。
#[tokio::test]
async fn cos_route_auth_failure_cools_the_account_and_retries_on_another_account() {
    let mut f = fixture(&["a"]);
    let t = thread(&f.store, "expired-login");
    post(&f.store, &t, "one");
    let run = run_to_end(&mut f, &t, 2).await;

    assert_eq!(run.state, ChatRunState::Completed, "{run:?}");
    assert_eq!(run.account_id.as_deref(), Some("b"), "{run:?}");
    assert_eq!(f.accounts_of(&run.id), vec!["a", "b"]);
    assert_auth_failed_held(&mut f, "a");
    let b = account(&mut f.d, "b");
    assert!(b.cooldown.is_none() && b.logged_in, "{b:?}");
    let notices = auth_notices(&f.store);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].title.contains("a (claude-code)"), "{notices:?}");

    let texts = system_texts(&f.store, &t);
    assert!(
        texts.iter().any(|t| t.contains("別の経路で再試行します")),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("CoS 実行経路:") && t.contains(" / b / ")),
        "{texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("CoS route unavailable")),
        "{texts:?}"
    );
}

/// D7 の 7: どちらの account もログインが切れている → 両方 auth_failed の cooldown（通知は account ごとに 1 件）、
/// run は候補を使い切った理由（`NoEligible`）で閉じ、誤解を招く "no route_defaults candidates are declared" は出ない。
#[tokio::test]
async fn cos_route_every_account_expired_closes_with_no_eligible_route() {
    let mut f = fixture(&["a", "b"]);
    let t = thread(&f.store, "all-expired");
    post(&f.store, &t, "one");
    let run = run_to_end(&mut f, &t, 2).await;

    assert_eq!(run.state, ChatRunState::Failed, "{run:?}");
    let reason = run.reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("no declared route candidate for harness claude_code is eligible"),
        "{run:?}"
    );
    assert!(!reason.contains("no route_defaults candidates are declared"));
    assert_eq!(f.accounts_of(&run.id), vec!["a", "b"]);
    assert_auth_failed_held(&mut f, "a");
    assert_auth_failed_held(&mut f, "b");
    assert_eq!(auth_notices(&f.store).len(), 2);
    let texts = system_texts(&f.store, &t);
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("no route_defaults candidates are declared")),
        "{texts:?}"
    );
}
