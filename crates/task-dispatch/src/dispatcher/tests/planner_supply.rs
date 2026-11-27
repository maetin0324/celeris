//! ADR 2026-11-24-planner-supply-failure-cooldown D2・D3・D7: planner の run が供給側の失敗（認証の失敗・枠切れ・
//! throttle）で終わったら、計画の試行（`plan_invalid` に倒す 2 回）に数えず、worker の run と同じ関数で account を
//! cooldown にし、次の planner run を別の account に回す。本番（2026-11-24、task 01M7XQA74F）では認証の切れた
//! `claude_max_lab` に planner run が 2 回とも回り、2 回で `plan_invalid` の決定が人に出た。
//!
//! 守る不変条件と、既存の試験で検出できなかった理由: 既存の planner の試験（`planner_budget.rs` など）は pool の
//! account を使わず、adapter の `Err`（供給側の失敗）で終わる planner run を作らない。既存の認証の失敗の試験
//! （`account_auth_failure.rs`）は worker の run だけを見る。すべて偽の adapter・一時ディレクトリ・注入した時計だけで、
//! 本物の OAuth と外部ネットワークに触れない。

use std::collections::BTreeMap;

use super::tree::{assert_replay_is_clean, leaf, stage, tree_limits, v3_plan};
use super::*;
use crate::accounts::AUTH_FAILED_HOLD_SECS;

/// account ごとの供給側の失敗の種類（入っている間は毎回その失敗を返す）。
#[derive(Clone, Copy, Debug)]
enum Fail {
    Auth,
    Throttled(u64),
    Exhausted,
}

/// pool の account で走る偽の adapter。planner run には妥当な /3 の計画を、worker の run には `result.json` を書いて
/// `Done` で終わる。`broken` の account の run は、その種類の `AdapterError` で終わる。走った (account, planner か) を残す。
#[derive(Clone)]
struct SupplyAdapter {
    broken: Arc<StdMutex<BTreeMap<String, Fail>>>,
    ran: Arc<StdMutex<Vec<(String, bool)>>>,
    account: Option<String>,
}

impl SupplyAdapter {
    fn new(broken: &[(&str, Fail)]) -> Arc<Self> {
        Arc::new(Self {
            broken: Arc::new(StdMutex::new(
                broken.iter().map(|(a, f)| (a.to_string(), *f)).collect(),
            )),
            ran: Arc::new(StdMutex::new(Vec::new())),
            account: None,
        })
    }

    fn planner_accounts(&self) -> Vec<String> {
        self.ran
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, planner)| *planner)
            .map(|(a, _)| a.clone())
            .collect()
    }
}

#[async_trait]
impl WorkerAdapter for SupplyAdapter {
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
        let account = self.account.clone().unwrap_or_default();
        let planner = req.context.execution_planner.is_some();
        self.ran.lock().unwrap().push((account.clone(), planner));
        let fail = self.broken.lock().unwrap().get(&account).copied();
        match fail {
            Some(Fail::Auth) => {
                return Err(AdapterError::AuthFailed(
                    "claude result: success: Failed to authenticate: this login reached its time limit and cannot be renewed".into(),
                ));
            }
            Some(Fail::Throttled(secs)) => {
                return Err(AdapterError::Throttled {
                    retry_after: Duration::from_secs(secs),
                });
            }
            Some(Fail::Exhausted) => {
                return Err(AdapterError::Exhausted("usage limit reached".into()));
            }
            None => {}
        }
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        if planner {
            let plan = v3_plan(vec![stage("s1", false)], vec![leaf("a", "s1", &[])]);
            std::fs::write(req.artifacts_dir.join("execution-plan.json"), plan)
                .expect("write execution-plan.json");
        } else {
            std::fs::write(
                req.artifacts_dir.join("result.json"),
                serde_json::json!({"summary": "ok", "evidence": []}).to_string(),
            )
            .expect("write result.json");
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

/// 注入した時計の値（cooldown の期限をこの値から決定的に確かめる）。
const NOW: i64 = 2_000_000_000;

/// account `a`・`b` の pool で、計画の gate を開けた dispatcher。時計は [`NOW`] に固定する。
fn supply_dispatcher(
    store: &Arc<SqliteStore>,
    adapter: Arc<SupplyAdapter>,
    accounts: &std::path::Path,
    max_requeues: u32,
) -> Dispatcher {
    let mut d = pool_dispatcher_with_requeues(
        store.clone(),
        adapter,
        None,
        accounts.to_path_buf(),
        2,
        4,
        max_requeues,
    );
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.limits = tree_limits();
    d.probes.set_now_unix(Arc::new(|| NOW));
    d
}

/// 人が compound を明示した root（最初の run は planner run）。
fn compound_root(dir: &std::path::Path) -> Task {
    let mut root = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    root.routing = Some(task_core::TaskRouting {
        execution_hint: Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: true,
        }),
        ..Default::default()
    });
    root
}

fn planner_finishes(
    store: &Arc<SqliteStore>,
    id: TaskId,
) -> Vec<(String, String, Option<task_core::RunEnd>)> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerFinished {
                run_id,
                outcome,
                role: Some(RunRole::Planner),
                end,
                ..
            } => Some((run_id, outcome, end)),
            _ => None,
        })
        .collect()
}

fn plan_invalid_decisions(store: &Arc<SqliteStore>, id: TaskId) -> Vec<task_core::DecisionRow> {
    store
        .decisions_list(Some(id))
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == task_core::DecisionKind::PlanInvalid)
        .collect()
}

/// task の全期間で計画を拒否した進捗（`planner_retry_message`）の数。供給側の失敗は計画の拒否ではないので出さない。
fn plan_rejections(store: &Arc<SqliteStore>, id: TaskId) -> usize {
    store
        .events_for(id)
        .unwrap()
        .iter()
        .filter(|(_, e)| {
            matches!(e, Event::WorkerProgress { msg, kind: None, .. }
                if msg.starts_with(task_ops::tree::PLANNER_RETRY_PREFIX))
        })
        .count()
}

/// route の選択が断った記録（`note_route_refusal` の進捗。task の timeline に出る）。
fn route_refusals(store: &Arc<SqliteStore>, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { msg, .. } if msg.starts_with("route refused: ") => Some(msg),
            _ => None,
        })
        .collect()
}

fn account_live(d: &mut Dispatcher, id: &str) -> task_ops::daemon::AccountLive {
    let (_, _, _, items) = d.accounts_snapshot();
    items.into_iter().find(|a| a.id == id).expect("account")
}

fn cooldown_of(d: &mut Dispatcher, id: &str) -> Option<(String, i64)> {
    account_live(d, id).cooldown.map(|c| (c.reason, c.until))
}

fn auth_notices(store: &Arc<SqliteStore>) -> Vec<task_core::Notice> {
    store
        .notice_list(&NoticeQuery {
            kinds: vec![NoticeKind::BadNews],
            ..NoticeQuery::default()
        })
        .unwrap()
        .items
}

const SUPPLY_END: Option<task_core::RunEnd> = Some(task_ops::tree::SUPPLY_FAILED_END);

/// 受け入れ 0・1（本番の再現）: account `a` の認証が切れている。1 本目の planner run は `a` で認証の失敗に終わり、
/// 試行に数えず（`end = HarnessError{Supply}`・計画の拒否の進捗なし）、`a` は AuthFailed の cooldown（再ログインまで
/// 保持）になり、人に bad_news が 1 件出る。2 本目の planner run は `b` に回って計画が採用され、`plan_invalid` は出ない。
/// 修正前は 2 本とも `a` に回り、2 回で `plan_invalid` が出た。
#[tokio::test]
async fn planner_auth_failure_is_not_a_plan_attempt_and_the_next_planner_runs_on_another_account() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = SupplyAdapter::new(&[("a", Fail::Auth)]);
    let mut d = supply_dispatcher(&store, adapter.clone(), accounts.path(), 0);
    let root = compound_root(ws.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    run_until_task_terminal(&mut d, &store, root_id).await;

    assert_eq!(adapter.planner_accounts(), vec!["a", "b"]);
    assert!(plan_invalid_decisions(&store, root_id).is_empty());
    assert_eq!(plan_rejections(&store, root_id), 0);
    let plan = store.execution_plan_active(root_id).unwrap().expect("plan");
    assert_eq!(plan.version, 1);
    let finishes = planner_finishes(&store, root_id);
    assert_eq!(finishes.len(), 2, "{finishes:?}");
    assert!(
        finishes[0]
            .1
            .starts_with("requeue: adapter: provider auth failed")
            && finishes[0].1.contains("Failed to authenticate"),
        "{finishes:?}"
    );
    assert_eq!(finishes[0].2, SUPPLY_END);
    assert!(finishes[1].1.starts_with("done: "), "{finishes:?}");
    let events = store.events_for(root_id).unwrap();
    assert!(task_ops::tree::planner_run_supply_failed(
        &events,
        &finishes[0].0
    ));
    assert!(!task_ops::tree::planner_run_supply_failed(
        &events,
        &finishes[1].0
    ));
    assert_eq!(store.get(root_id).unwrap().unwrap().attempts, 0);

    // worker の run と同じ規則: AuthFailed は再ログインまで保持し、一覧に「再ログインが必要」が出る。
    assert_eq!(
        cooldown_of(&mut d, "a"),
        Some(("auth_failed".to_string(), NOW + AUTH_FAILED_HOLD_SECS))
    );
    let a = account_live(&mut d, "a");
    assert!(!a.logged_in, "{a:?}");
    assert_eq!(a.excluded_reason.as_deref(), Some("cooldown"));
    assert_eq!(cooldown_of(&mut d, "b"), None);
    let notices = auth_notices(&store);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].title.contains("a (claude-code)"), "{notices:?}");
    assert_replay_is_clean(&store);
}

/// 受け入れ 2: throttle と枠切れも試行に数えず、worker と同じ `account_cooldown_for_failure` の期限（throttle は
/// `retry_after`、枠切れは `fallback_cooldown_secs` = 300 秒）で cooldown にして、次の planner run は別の account に回る。
#[tokio::test]
async fn planner_throttle_and_exhaustion_are_not_plan_attempts_and_cool_the_account_down() {
    for (fail, reason, until) in [
        (Fail::Throttled(120), "throttled", NOW + 120),
        (Fail::Exhausted, "exhausted", NOW + 300),
    ] {
        let accounts = accounts_fixture();
        let ws = tempfile::tempdir().unwrap();
        let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let adapter = SupplyAdapter::new(&[("a", fail)]);
        let mut d = supply_dispatcher(&store, adapter.clone(), accounts.path(), 2);
        let root = compound_root(ws.path());
        let root_id = root.id;
        store.create_task(&root, vec![]).unwrap();
        run_until_task_terminal(&mut d, &store, root_id).await;

        assert_eq!(adapter.planner_accounts(), vec!["a", "b"], "{fail:?}");
        assert!(
            plan_invalid_decisions(&store, root_id).is_empty(),
            "{fail:?}"
        );
        assert_eq!(plan_rejections(&store, root_id), 0, "{fail:?}");
        let finishes = planner_finishes(&store, root_id);
        assert_eq!(finishes[0].2, SUPPLY_END, "{fail:?} {finishes:?}");
        assert_eq!(
            cooldown_of(&mut d, "a"),
            Some((reason.to_string(), until)),
            "{fail:?}"
        );
        assert!(auth_notices(&store).is_empty(), "{fail:?}");
        assert_replay_is_clean(&store);
    }
}

/// 受け入れ 2 の上限: throttle・枠切れが続いて `max_requeues`（ここでは 0）に達したら、worker の
/// `settle_adapter_failure` と同じく通常の不正な試行に数える（枠が戻らないまま planner を回し続けない）。cooldown は
/// 同じく付き、次の試行は別の account で採用される。
#[tokio::test]
async fn planner_throttle_at_the_requeue_limit_is_counted_as_an_invalid_attempt() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = SupplyAdapter::new(&[("a", Fail::Throttled(60))]);
    let mut d = supply_dispatcher(&store, adapter.clone(), accounts.path(), 0);
    let root = compound_root(ws.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    run_until_task_terminal(&mut d, &store, root_id).await;

    assert_eq!(adapter.planner_accounts(), vec!["a", "b"]);
    let finishes = planner_finishes(&store, root_id);
    assert!(
        finishes[0]
            .1
            .contains("invalid execution plan: requeue limit (0) reached: adapter:"),
        "{finishes:?}"
    );
    assert_ne!(finishes[0].2, SUPPLY_END, "{finishes:?}");
    assert_eq!(plan_rejections(&store, root_id), 1);
    assert!(plan_invalid_decisions(&store, root_id).is_empty());
    assert_eq!(
        cooldown_of(&mut d, "a"),
        Some(("throttled".to_string(), NOW + 60))
    );
    assert_replay_is_clean(&store);
}

/// 受け入れ 2（使える account が無いときの待ち方）: `a`・`b` とも認証が切れている。planner run は `a`、`b` の順に
/// 1 本ずつ認証の失敗で終わり、どちらも試行に数えない。その後は route の選択が断り、run を起こさない。task は
/// 失敗にも `plan_invalid` にもならず `ready` で待ち（attempts 0）、人には account ごとの「再ログインが必要」の bad_news
/// （2 件）と、一覧の `cooldown.reason = auth_failed`、task の timeline の「route refused」（除外理由 cooldown つき、
/// 1 回だけ）が出る。`a` の再ログインの後は `a` で計画が採用される。
#[tokio::test]
async fn planner_with_no_usable_account_waits_for_a_relogin_without_plan_invalid() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = SupplyAdapter::new(&[("a", Fail::Auth), ("b", Fail::Auth)]);
    let mut d = supply_dispatcher(&store, adapter.clone(), accounts.path(), 0);
    let root = compound_root(ws.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    for _ in 0..60 {
        d.tick().unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        adapter.planner_accounts(),
        vec!["a", "b"],
        "no re-pick while held"
    );
    let row = store.get(root_id).unwrap().unwrap();
    assert_eq!((row.status, row.attempts), (Status::Ready, 0), "{row:?}");
    assert!(plan_invalid_decisions(&store, root_id).is_empty());
    assert_eq!(plan_rejections(&store, root_id), 0);
    assert_eq!(d.planner_attempts_in_window(root_id).unwrap(), 0);
    let finishes = planner_finishes(&store, root_id);
    assert!(
        finishes.iter().all(|(_, _, end)| *end == SUPPLY_END),
        "{finishes:?}"
    );
    for id in ["a", "b"] {
        assert_eq!(
            cooldown_of(&mut d, id),
            Some(("auth_failed".to_string(), NOW + AUTH_FAILED_HOLD_SECS)),
            "{id}"
        );
    }
    let notices = auth_notices(&store);
    assert_eq!(notices.len(), 2, "{notices:?}");
    let refusals = route_refusals(&store, root_id);
    assert_eq!(refusals.len(), 1, "recorded once while held: {refusals:?}");
    assert!(
        refusals[0].contains("is eligible") && refusals[0].contains(r#""reason":"Cooldown""#),
        "{refusals:?}"
    );

    adapter.broken.lock().unwrap().remove("a");
    d.record_account_login(AccountAdapter::ClaudeCode, "a");
    run_until_task_terminal(&mut d, &store, root_id).await;
    assert_eq!(adapter.planner_accounts(), vec!["a", "b", "a"]);
    assert!(plan_invalid_decisions(&store, root_id).is_empty());
    assert_eq!(store.get(root_id).unwrap().unwrap().attempts, 0);
    assert_replay_is_clean(&store);
}

/// 受け入れ 3（再起動 1）: `a` が認証の失敗で AuthFailed の cooldown になった後、daemon を再起動する（同じ account の
/// root で新しい `Dispatcher` を作る＝`.celeris-usage.json` から `AccountBook` を読み直す）。cooldown は残り、
/// 再起動後の最初の planner run は `a` を試さず `b` に回る。
/// 守る不変条件: 認証の切れた account は再起動で routing に戻らない。既存の `accounts/tests.rs` の保存・読み直しの試験は
/// 帳簿だけを見て、planner run の account の選択まで通さないので、この経路は検出できない。
#[tokio::test]
async fn auth_failed_cooldown_survives_restart() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = SupplyAdapter::new(&[("a", Fail::Auth)]);
    let mut d = supply_dispatcher(&store, adapter.clone(), accounts.path(), 0);
    let first = compound_root(ws.path());
    let first_id = first.id;
    store.create_task(&first, vec![]).unwrap();
    run_until_task_terminal(&mut d, &store, first_id).await;
    assert_eq!(adapter.planner_accounts(), vec!["a", "b"]);
    assert!(accounts.path().join(".celeris-usage.json").exists());
    drop(d);

    // 再起動: 同じ root から帳簿を読み直す。
    let mut d = supply_dispatcher(&store, adapter.clone(), accounts.path(), 0);
    assert_eq!(
        cooldown_of(&mut d, "a"),
        Some(("auth_failed".to_string(), NOW + AUTH_FAILED_HOLD_SECS))
    );
    assert!(!account_live(&mut d, "a").logged_in);
    let second = compound_root(ws.path());
    let second_id = second.id;
    store.create_task(&second, vec![]).unwrap();
    run_until_task_terminal(&mut d, &store, second_id).await;

    assert_eq!(
        adapter.planner_accounts(),
        vec!["a", "b", "b"],
        "after restart the held account is not tried again"
    );
    assert!(plan_invalid_decisions(&store, second_id).is_empty());
    assert_eq!(store.get(second_id).unwrap().unwrap().attempts, 0);
    assert_replay_is_clean(&store);
}

/// 受け入れ 3（再起動 2）: 帳簿の file が失われた再起動（保存の失敗・file の削除）でも、最初の供給側の失敗で `a` は
/// 再び AuthFailed の cooldown になり、2 本目の planner run は `b` に回る。どちらの失敗も計画の試行に数えない。
/// 守る不変条件: cooldown の永続化に頼らず、失敗の観測が cooldown を作り直す（ADR D6）。
#[tokio::test]
async fn auth_failed_without_saved_book_is_cooled_down_again_by_the_first_failure() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = SupplyAdapter::new(&[("a", Fail::Auth)]);
    let mut d = supply_dispatcher(&store, adapter.clone(), accounts.path(), 0);
    let first = compound_root(ws.path());
    let first_id = first.id;
    store.create_task(&first, vec![]).unwrap();
    run_until_task_terminal(&mut d, &store, first_id).await;
    drop(d);

    std::fs::remove_file(accounts.path().join(".celeris-usage.json")).unwrap();
    let mut d = supply_dispatcher(&store, adapter.clone(), accounts.path(), 0);
    assert_eq!(cooldown_of(&mut d, "a"), None, "book lost: no cooldown");
    let second = compound_root(ws.path());
    let second_id = second.id;
    store.create_task(&second, vec![]).unwrap();
    run_until_task_terminal(&mut d, &store, second_id).await;

    assert_eq!(adapter.planner_accounts(), vec!["a", "b", "a", "b"]);
    assert_eq!(
        cooldown_of(&mut d, "a"),
        Some(("auth_failed".to_string(), NOW + AUTH_FAILED_HOLD_SECS))
    );
    assert!(plan_invalid_decisions(&store, second_id).is_empty());
    assert_eq!(plan_rejections(&store, second_id), 0);
    assert_eq!(store.get(second_id).unwrap().unwrap().attempts, 0);
    assert_replay_is_clean(&store);
}
