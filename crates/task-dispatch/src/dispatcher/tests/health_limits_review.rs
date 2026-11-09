//! ADR 2026-11-08-health-enforcement-loop D3.4・D4.3・D4.4 の task の review の経路（`try_review_repair`）の試験。health の
//! Error・Unavailable、同じ指摘の往復、health の修復の上限の使い切りは repair にも `ReviewFail`（再実行・compound の
//! replan・retry 切れの Failed → review_rework）にもせず、`Trigger::ReviewBlocked`（attempts 不変）で止めて受信箱に質問を
//! 1 件出す。health でない失敗が混ざる不合格は今どおり `ReviewFail`。止めた task は既存の comment で再開する。
//!
//! 既存の試験（`health_enforce_direct_review_*`・`health_enforce_non_celeris_direct_review_routes_to_repair`）は review の
//! 項目と修復の 1 往復だけを見て、Error・往復・上限の使い切りの後の遷移（Blocked・attempts・受信箱・replan）を通らない。
//! `health_limits_repeat_*` は判定の純粋関数だけ、`health_limits_inbox_*` は受信箱の導出だけを見る。ここは dispatcher の
//! 流れで確かめる。
//!
//! 検査器は試験の repo の base に commit した stub（`health_enforce` の [`commit_base_checker`]。base の検査器は同梱より
//! 先に選ばれる）。修復の WU の run の後に作業ツリーへ `FINDING`（次の評価の fingerprint）か `FIXED`（合格）を commit
//! して、修復の前後の指摘を決める。時計は使わず、`run_until_idle`（出来事が尽きるまで tick を回す）で決まる。外部
//! ネットワークに出ず、LLM を呼ばない。
use super::health_enforce::{
    RepairActionAdapter, commit_base_checker, commit_fixed, non_celeris_dispatcher,
};
use super::target_sync::{commit_file, reviewing_task, set_check};
use super::*;
use task_core::HealthRepeatSite;
use task_core::required_checks::health::{HEALTH_CHECK_ID, HealthVerdict};
use task_ops::human_inbox::InboxKind;

/// stub の検査器の既定の fingerprint（`FINDING` が無いとき）。
const FP1: &str = "deps.violation:0000000000000001";

/// 1 件の review の場面: 試験の repo（`base_exit` が `Some` なら base に stub の検査器）と、その task の作業ツリー。
struct Scenario {
    _repo: tempfile::TempDir,
    _ws: tempfile::TempDir,
    store: Arc<dyn TaskStore>,
    task: Task,
    d: Dispatcher,
    dir: PathBuf,
}

impl Scenario {
    /// atomic の task（`Reviewing`、acceptance は `check`）。`on_repair` は修復の WU の run の後に作業ツリーで呼ぶ。
    fn atomic(
        base_exit: Option<i32>,
        check: &str,
        on_repair: impl Fn(&Path, &str) + Send + Sync + 'static,
    ) -> Self {
        Self::build(base_exit, check, None, on_repair)
    }

    fn build(
        base_exit: Option<i32>,
        check: &str,
        plan: Option<u32>,
        on_repair: impl Fn(&Path, &str) + Send + Sync + 'static,
    ) -> Self {
        let repo = tempfile::tempdir().unwrap();
        init_test_repo(repo.path());
        if let Some(exit) = base_exit {
            commit_base_checker(repo.path(), exit);
        }
        let ws = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let mut task = reviewing_task(repo.path(), &store);
        set_check(&mut task, check.to_string());
        if let Some(max_retries) = plan {
            task.status = Status::Ready;
            task.budget.max_retries = max_retries;
        }
        store.insert(&task).unwrap();
        let adapter = Arc::new(RepairActionAdapter::new(on_repair));
        let d = non_celeris_dispatcher(store.clone(), adapter, ws.path(), 1);
        let wt = d.local_worktree_for(&task).unwrap();
        wt.ensure_blocking().unwrap();
        std::fs::create_dir_all(wt.dir.join("src")).unwrap();
        commit_file(&wt.dir, "src/lib.rs", "pub fn foo() {}\n");
        let dir = wt.dir.clone();
        Self {
            _repo: repo,
            _ws: ws,
            store,
            task,
            d,
            dir,
        }
    }

    /// worker の run の後の review を起こし、出来事が尽きるまで回す。
    async fn review(&mut self) {
        assert!(
            self.d
                .spawn_review(self.task.id, "worker".into(), &ReviewSubject::default())
                .unwrap()
        );
        assert!(run_until_idle(&mut self.d, 800).await.idle);
    }

    fn stored(&self) -> Task {
        self.store.get(self.task.id).unwrap().unwrap()
    }

    fn events(&self) -> Vec<Event> {
        events_of(&self.store, self.task.id)
    }

    /// 受信箱の質問のうち、この task のもの（本文）。
    fn questions(&self) -> Vec<String> {
        task_ops::inbox::inbox(
            self.store.as_ref(),
            None,
            &view_ctx(),
            OffsetDateTime::now_utc(),
            &|_, _| Vec::new(),
        )
        .unwrap()
        .questions
        .into_iter()
        .filter(|q| q.task.id == self.task.id)
        .map(|q| q.question)
        .collect()
    }

    /// 人の受信箱の `health_repeat` の項目の数。
    fn health_repeat_items(&self) -> usize {
        task_ops::human_inbox::human_inbox(
            self.store.as_ref(),
            None,
            &view_ctx(),
            OffsetDateTime::now_utc(),
            &|_, _| Vec::new(),
        )
        .unwrap()
        .items
        .iter()
        .filter(|i| i.kind == InboxKind::HealthRepeat)
        .count()
    }
}

fn view_ctx() -> task_ops::view::ViewContext {
    task_ops::view::ViewContext {
        workspace_roots: task_core::local_machine::WorkspaceRoots::new(
            PathBuf::from("/nonexistent"),
            None,
        ),
        retry_backoff_base: Duration::ZERO,
        retry_backoff_max: Duration::ZERO,
        max_requeues: 5,
        clusters: Default::default(),
    }
}

/// 修復の WU の key（`repair-<n>`）ごとに次の評価の fingerprint を変える（`deps.violation:<n+1 の 16 桁>`）。
fn next_finding(dir: &Path, key: &str) {
    let n: u64 = key.trim_start_matches("repair-").parse().unwrap();
    commit_file(dir, "FINDING", &format!("deps.violation:{:016x}\n", n + 1));
}

fn reasons(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Transitioned { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect()
}

fn count(events: &[Event], pred: impl Fn(&Event) -> bool) -> usize {
    events.iter().filter(|e| pred(e)).count()
}

fn repairs_scheduled(events: &[Event]) -> usize {
    count(events, |e| matches!(e, Event::RepairScheduled { .. }))
}

fn raised(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::QuestionRaised { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// review の health の項目の評価（`RequiredChecksEvaluated.results[].health`）の verdict の列。
fn health_verdicts(events: &[Event]) -> Vec<HealthVerdict> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::RequiredChecksEvaluated { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .filter(|r| r.cmd == HEALTH_CHECK_ID)
        .filter_map(|r| r.health.as_ref().map(|h| h.verdict))
        .collect()
}

/// 止めた後の不変条件: 最後の遷移は `review_blocked` の 1 回で（`Reviewing → Blocked`）、attempts は 0 のまま、その後に
/// run を起こさず、`review_fail`（再実行・replan・retry 切れの Failed）と自動 replan が無い。
fn assert_blocked_without_rerun(s: &Scenario) -> Vec<Event> {
    let stored = s.stored();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    assert_eq!(stored.attempts, 0, "{stored:?}");
    let events = s.events();
    let blocked: Vec<usize> = (events.iter().enumerate())
        .filter(|(_, e)| {
            matches!(e, Event::Transitioned { from: Status::Reviewing, to: Status::Blocked, reason, .. }
                if reason == Trigger::ReviewBlocked.name())
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(blocked.len(), 1, "{:?}", reasons(&events));
    assert!(
        !events[blocked[0]..]
            .iter()
            .any(|e| matches!(e, Event::WorkerStarted { .. })),
        "no run after the block: {events:?}"
    );
    assert!(
        !reasons(&events).contains(&Trigger::ReviewFail.name()),
        "{:?}",
        reasons(&events)
    );
    assert_eq!(
        count(&events, |e| matches!(e, Event::AutoReplanRequested { .. })),
        0
    );
    events
}

/// 不変条件（D3.4）: review の health の Error（検査器の exit 2）は repair にも `ReviewFail` にもならず、task を Blocked
/// （attempts 不変）にして受信箱に `health_check_error: <理由>` の質問を 1 件出す。合格にしない。
#[tokio::test]
async fn health_limits_review_error_blocks_without_repair() {
    let mut s = Scenario::atomic(Some(2), "true", |_, _| {});
    s.review().await;
    let events = assert_blocked_without_rerun(&s);
    assert_eq!(health_verdicts(&events), vec![HealthVerdict::Error]);
    assert_eq!(repairs_scheduled(&events), 0, "{events:?}");
    assert!(
        (s.store.work_units_for(s.task.id).unwrap().iter())
            .all(|u| u.kind != task_core::WorkUnitKind::Repair)
    );
    let questions = s.questions();
    assert_eq!(questions.len(), 1, "{questions:?}");
    assert!(
        questions[0].starts_with("health_check_error: "),
        "{questions:?}"
    );
    assert_eq!(raised(&events), vec![questions[0].as_str()]);
}

/// 不変条件（D3.4・J4）: 検査器が base にも同梱にも無い（Unavailable、`health_check_error: checker_missing`）review も
/// 合格にせず、repair も再実行もせずに Blocked と質問 1 件で止まる。
#[tokio::test]
async fn health_limits_review_unavailable_is_not_pass() {
    assert_eq!(crate::health_review::gate::bundled_checker(), None);
    let mut s = Scenario::atomic(None, "true", |_, _| {});
    s.review().await;
    let events = assert_blocked_without_rerun(&s);
    assert_eq!(health_verdicts(&events), vec![HealthVerdict::Error]);
    assert_eq!(repairs_scheduled(&events), 0, "{events:?}");
    let questions = s.questions();
    assert_eq!(questions.len(), 1, "{questions:?}");
    assert!(
        questions[0].starts_with("health_check_error: checker_missing"),
        "{questions:?}"
    );
}

/// 不変条件（D4.3）: 修復の後の review に同じ fingerprint の集合が残ったら、2 回目の repair を作らず Blocked にし、木の
/// root（ここでは自分）に `HealthFindingsRepeated`（位置 review）を追記して、受信箱の `health_repeat` を 1 件出す。
#[tokio::test]
async fn health_limits_review_same_fingerprint_blocks() {
    let mut s = Scenario::atomic(Some(1), "true", |dir, key| {
        commit_file(dir, &format!("{key}.txt"), "tried\n");
    });
    s.review().await;
    let events = assert_blocked_without_rerun(&s);
    assert_eq!(
        health_verdicts(&events),
        vec![HealthVerdict::Fail, HealthVerdict::Fail]
    );
    assert_eq!(repairs_scheduled(&events), 1, "{events:?}");
    let repeated: Vec<(&HealthRepeatSite, &Vec<String>)> = events
        .iter()
        .filter_map(|e| match e {
            Event::HealthFindingsRepeated {
                source_task,
                site,
                fingerprints,
                ..
            } if *source_task == s.task.id => Some((site, fingerprints)),
            _ => None,
        })
        .collect();
    assert_eq!(
        repeated,
        vec![(&HealthRepeatSite::Review, &vec![FP1.to_string()])]
    );
    assert_eq!(s.health_repeat_items(), 1);
    let asked = raised(&events);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].starts_with("health_findings_repeated: ") && asked[0].contains(FP1),
        "{asked:?}"
    );
}

/// 不変条件（D4.4、atomic）: 不合格がすべて health の Fail で `max_repairs_per_class`（bucket `health:deps.violation`）を
/// 使い切ったら、`ReviewFail`（attempts+1 の再実行）に落ちず Blocked で止まる。attempts は不変、新しい run も repair も
/// 無く、受信箱に bucket と回数を書いた質問が 1 件出る。指摘は修復ごとに変わる（往復ではない）。
#[tokio::test]
async fn health_limits_review_repair_cap_blocks_without_rerun() {
    let mut s = Scenario::atomic(Some(1), "true", next_finding);
    s.d.config.execution.max_repairs_per_class = 1;
    s.review().await;
    let events = assert_blocked_without_rerun(&s);
    assert_eq!(
        health_verdicts(&events),
        vec![HealthVerdict::Fail, HealthVerdict::Fail]
    );
    assert_eq!(repairs_scheduled(&events), 1, "{events:?}");
    assert_eq!(
        count(&events, |e| matches!(
            e,
            Event::HealthFindingsRepeated { .. }
        )),
        0
    );
    let questions = s.questions();
    assert_eq!(questions.len(), 1, "{questions:?}");
    let q = &questions[0];
    assert!(q.starts_with("health_repair_cap: "), "{q}");
    assert!(
        q.contains("bucket health:deps.violation: 1 回 / max_repairs_per_class 1"),
        "{q}"
    );
    assert!(q.contains("修復の合計 1 回 / max_repairs 3"), "{q}");
}

/// 不変条件（D4.4、compound）: 計画のある task の最終レビューで、health だけの不合格が `max_repairs` を使い切っても、
/// `ReviewFail` の後の replan（計画の版の追加・`AutoReplanRequested`・planner の run）に進まず Blocked で止まる
/// （`max_retries` = 2 なので、旧い経路なら Ready に戻って replan した）。
#[tokio::test]
async fn health_limits_review_compound_repair_cap_does_not_replan() {
    let mut s = Scenario::build(Some(1), "true", Some(2), next_finding);
    s.d.config.execution.max_repairs = 1;
    task_ops::execution::adopt_plan(
        s.store.as_ref(),
        s.task.id,
        task_core::ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA.into(),
            rationale: "compound".into(),
            work_units: vec![wu_spec("main", &[])],
            phases: Vec::new(),
            children: Vec::new(),
            design_brief: None,
        },
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let planned = |events: &[Event]| count(events, |e| matches!(e, Event::ExecutionPlanned { .. }));
    let before = planned(&s.events());
    assert!(run_until_idle(&mut s.d, 800).await.idle);
    let events = assert_blocked_without_rerun(&s);
    assert_eq!(
        health_verdicts(&events),
        vec![HealthVerdict::Fail, HealthVerdict::Fail]
    );
    assert_eq!(repairs_scheduled(&events), 1, "{events:?}");
    assert_eq!(planned(&events), before, "no new plan version");
    let active = s.store.execution_plan_active(s.task.id).unwrap().unwrap();
    assert_eq!(active.version, 1, "{active:?}");
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::WorkerStarted {
                role: Some(task_core::RunRole::Planner),
                ..
            }
        )),
        "no planner run"
    );
    let questions = s.questions();
    assert_eq!(questions.len(), 1, "{questions:?}");
    assert!(
        questions[0].contains("修復の合計 1 回 / max_repairs 1"),
        "{questions:?}"
    );
}

/// 不変条件（D3.2・D4.4 の外）: health の Fail に health でない失敗（acceptance の `false`）が混ざる不合格は修復できない
/// 種類で、今どおり `ReviewFail`（`max_retries` = 0 なので Failed）に進む。Blocked にも質問にもしない。
#[tokio::test]
async fn health_limits_review_mixed_failure_keeps_review_fail() {
    let mut s = Scenario::atomic(Some(1), "false", |_, _| {});
    s.review().await;
    let events = s.events();
    assert_eq!(health_verdicts(&events), vec![HealthVerdict::Fail]);
    let stored = s.stored();
    assert_eq!(stored.status, Status::Failed, "{stored:?}");
    assert_eq!(stored.attempts, 1, "{stored:?}");
    let reasons = reasons(&events);
    assert!(reasons.contains(&Trigger::ReviewFail.name()), "{reasons:?}");
    assert!(
        !reasons.contains(&Trigger::ReviewBlocked.name()),
        "{reasons:?}"
    );
    assert_eq!(repairs_scheduled(&events), 0, "{events:?}");
    assert!(raised(&events).is_empty(), "{events:?}");
}

/// 不変条件（D3.4）: health の Error で止めた task は、人が環境を直して comment すると既存の回答（`Trigger::Answer`、
/// attempts 不変）で Ready に戻り、次の review で評価し直して合格すれば done になる。
#[tokio::test]
async fn health_limits_review_blocked_resumes_on_comment() {
    let mut s = Scenario::atomic(Some(2), "true", |_, _| {});
    s.review().await;
    assert_blocked_without_rerun(&s);
    commit_fixed(&s.dir);
    let result = task_ops::comment::post_human_comment_as(
        s.store.as_ref(),
        s.task.id,
        Some("human:web".into()),
        "検査器を直した".into(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let resumed = result.transition.expect("the comment answers the question");
    assert_eq!((resumed.from, resumed.to), (Status::Blocked, Status::Ready));
    assert_eq!(resumed.reason, Trigger::Answer.name());
    assert_eq!(s.stored().attempts, 0);
    assert!(run_until_idle(&mut s.d, 800).await.idle);
    let stored = s.stored();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert_eq!(stored.attempts, 0, "{stored:?}");
    assert_eq!(
        health_verdicts(&s.events()),
        vec![HealthVerdict::Error, HealthVerdict::Pass]
    );
}
