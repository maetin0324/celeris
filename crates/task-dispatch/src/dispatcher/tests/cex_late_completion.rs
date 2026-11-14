//! 健全性v2 R4 cex-late: ADR 2026-11-11-dispatcher-state-ownership D7 の表で cex-review に割り当てたのに試験が無かった
//! 反例 2 つ（replan・段の repair の後に遅れて届く旧い完了、同じ quota group の run の続けての終わり）を今の実装に対して
//! 固定する試験（`agent-docs/progress/2026-11-11-health-v2-r4/cex-late.md`）。
//!
//! 規約（`cex_review_authority.rs` と同じ）: 試験は Dispatcher の private field に触らず、store の行（task・WU・計画・
//! `runs` 索引・決定）と event と `tick()` の戻り値（`TickReport`）だけを見る。待ちは [`run_until_state`]（出来事待ち・
//! 壁時計の保険）で、tick の回数の上限で待たない。run の終わりは fake adapter の gate（semaphore）で試験が決め、
//! 観測の時刻は試験が作る。CPU 負荷は使わない。
use super::cex_review_authority::{
    deliverable, git_review_task, merge_queued, reached_done, reviewer_finishes, transitions,
    write_review_json,
};
use super::*;

// ---------------------------------------------------------------------------
// (ii) 同じ quota group の run 2 本の終わりが続けて届く
// ---------------------------------------------------------------------------

/// 試験の route の model（`install_test_routes` の `m`）。100 万 input token = 1 USD の単価を与える。
const PRICED_MODEL: &str = "m";
/// 木の USD 上限（1 USD）。1 本目の run（0.6 USD）だけでは届かず、2 本目の後に届く。
const TREE_MAX_USD_MICROS: u64 = 1_000_000;
/// 1 本の run の input token（0.6 USD）。
const RUN_INPUT_TOKENS: u64 = 600_000;

/// task ごとの gate で run を止め、開いたら task ごとの終わり方を返す adapter。run の開始で account の観測
/// （`after`）を `sink.rate_limit` に送る。route の `configure` は同じ状態を共有する複製を返す。
#[derive(Clone)]
struct QuotaGroupAdapter(Arc<QuotaGroupInner>);

struct QuotaGroupInner {
    /// task ごとの `(gate, 終わり方)`。
    endings: HashMap<TaskId, (tokio::sync::Semaphore, Terminal)>,
    /// run の開始で送る account の観測。
    observation: RateLimitObservation,
    /// 開始した run の `(task_id, run_id)`。
    started: StdMutex<Vec<(TaskId, String)>>,
}

impl QuotaGroupAdapter {
    fn new(endings: Vec<(TaskId, Terminal)>, observation: RateLimitObservation) -> Self {
        QuotaGroupAdapter(Arc::new(QuotaGroupInner {
            endings: endings
                .into_iter()
                .map(|(id, t)| (id, (tokio::sync::Semaphore::new(0), t)))
                .collect(),
            observation,
            started: StdMutex::new(Vec::new()),
        }))
    }

    fn release(&self, id: TaskId) {
        self.0.endings[&id].0.add_permits(1);
    }

    fn started(&self) -> Vec<(TaskId, String)> {
        self.0.started.lock().unwrap().clone()
    }

    fn run_of(&self, id: TaskId) -> String {
        self.started()
            .into_iter()
            .find(|(t, _)| *t == id)
            .map(|(_, r)| r)
            .unwrap()
    }
}

#[async_trait]
impl WorkerAdapter for QuotaGroupAdapter {
    fn id(&self) -> &str {
        "claude-code"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.0
            .started
            .lock()
            .unwrap()
            .push((req.task.id, run_id.to_string()));
        let (gate, terminal) = &self.0.endings[&req.task.id];
        if let Ok(permit) = gate.acquire().await {
            permit.forget();
        }
        sink.rate_limit(self.0.observation.clone());
        Ok(RunOutcome {
            terminal: terminal.clone(),
            exit_code: Some(0),
        })
    }
    fn configure(&self, settings: &task_worker::AdapterSettings) -> task_worker::Configured {
        settings.configured(
            &[
                task_worker::AdapterSetting::Model,
                task_worker::AdapterSetting::Env,
            ],
            || Arc::new(self.clone()),
        )
    }
}

/// 5 時間と 7 日の窓の観測（Claude の account の `before`/`after` の有効性は両方の窓を要る）。
fn claude_observation(utilization: f64, now: i64) -> RateLimitObservation {
    RateLimitObservation {
        five_hour: Some(task_core::RateWindow {
            utilization,
            resets_at: now + 3_600,
        }),
        seven_day: Some(task_core::RateWindow {
            utilization,
            resets_at: now + 302_400,
        }),
        one_month: None,
        status: None,
        resets_at: None,
        observed_at: now,
    }
}

fn input_usage(input: u64) -> task_core::Usage {
    task_core::Usage {
        input_tokens: Some(input),
        output_tokens: Some(0),
        ..Default::default()
    }
}

/// `run_id` の `QuotaEstimated` の `(account, weighted_tokens, method)` の列（event の順）。
fn quota_events(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
    run_id: &str,
) -> Vec<(Option<String>, f64, task_core::quota::QuotaMethod)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::QuotaEstimated {
                run_id: r,
                account,
                weighted_tokens,
                method,
                ..
            } if r == run_id => Some((account, weighted_tokens, method)),
            _ => None,
        })
        .collect()
}

fn limit_decisions(store: &Arc<dyn TaskStore>, id: TaskId) -> usize {
    events_of(store, id)
        .iter()
        .filter(|e| matches!(e, Event::DecisionRequested { decision } if decision.key == "limit:max_tree_usd"))
        .count()
}

fn tree_counters_of(store: &Arc<dyn TaskStore>, root: TaskId) -> TreeCounters {
    task_ops::tree::tree_counters(store.as_ref(), root, &priced_rates()).unwrap()
}

fn priced_rates() -> task_core::pricing::BudgetRates {
    task_core::pricing::BudgetRates::new([(
        PRICED_MODEL.into(),
        task_core::pricing::BudgetRate {
            input_per_million: 1.0,
            output_per_million: 0.0,
            cache_read_per_million: 0.0,
            cache_write_per_million: 0.0,
        },
    )])
    .unwrap()
}

use task_core::TreeCounters;

/// 1 つの木（root `R`）の 2 つの task（`C` は木の節点で決定的な検査だけ、`R` は reviewer 付き）を、account が
/// `a` 1 つだけの pool で同時に走らせ、同じ quota group に入れる。`C` の run を先に終え（group は pending）、
/// 続けて `R` の run を終える（group が閉じる）。`R` は turns 切れの continuation を返し、次の run の上限の判定は
/// `R` の終わりの後に来る。`unknown` なら `R` の run は usage を返さない（費用不明）。
async fn consecutive_group_ends(unknown: bool) {
    let accounts = tempfile::tempdir().unwrap();
    let acct = accounts.path().join("a");
    std::fs::create_dir_all(&acct).unwrap();
    std::fs::write(acct.join(".credentials.json"), "{}").unwrap();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());

    let mut root = new_task(ws.path(), Check::Reviewer, 1);
    root.title = "root".into();
    root.tree = Some(task_core::TreeInfo::root(root.id));
    let mut node = new_task(
        ws.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    node.title = "node".into();
    node.tree = Some(task_core::TreeInfo {
        root_id: root.id,
        depth: 2,
        parent_unit: None,
        base_commit: None,
    });
    store.insert(&root).unwrap();
    store.insert(&node).unwrap();

    let now = OffsetDateTime::now_utc().unix_timestamp();
    let root_ending = Terminal::BudgetExhausted {
        kind: task_core::BudgetKind::Turns,
        message: "continue remaining work".into(),
        usage: (!unknown).then(|| input_usage(RUN_INPUT_TOKENS)),
    };
    let node_ending = Terminal::Done {
        summary: "node done".into(),
        evidence: vec![],
        usage: Some(input_usage(RUN_INPUT_TOKENS)),
    };
    let adapter = QuotaGroupAdapter::new(
        vec![(node.id, node_ending), (root.id, root_ending)],
        claude_observation(0.30, now),
    );
    let mut d = pool_dispatcher(
        store.clone(),
        Arc::new(adapter.clone()),
        None,
        accounts.path().to_path_buf(),
        2,
        2,
    );
    let mut config = d.config().clone();
    config.execution.limits.tree.enabled = true;
    config.execution.limits.tree.max_tree_usd_micros = Some(TREE_MAX_USD_MICROS);
    d.apply_dispatch_config(config);
    d.set_budget_rates(priced_rates());
    // group の `before`（最初の run の開始の観測。有効な時刻）。
    d.account_book(AccountAdapter::ClaudeCode)
        .unwrap()
        .lock()
        .unwrap()
        .record_observation(
            "a",
            claude_observation(0.10, now),
            crate::accounts::ObservationSource::Run,
        );

    // 2 本とも account `a` で走り、adapter の gate で止まっている（同じ quota group）。
    let started = adapter.clone();
    assert!(run_until_state(&mut d, || started.started().len() == 2).await);
    for id in [root.id, node.id] {
        let runs = store.runs_for_task(id).unwrap();
        assert_eq!(runs.len(), 1, "{id}");
        assert_eq!(runs[0].account.as_deref(), Some("a"), "{runs:?}");
    }
    let node_run = adapter.run_of(node.id);
    let root_run = adapter.run_of(root.id);

    // 1 本目（C）の終わり: group は閉じず、C の見積りは暫定（unknown）。木の費用は 0.6 USD で上限の内側。
    adapter.release(node.id);
    let s = store.clone();
    assert!(
        run_until_state(&mut d, || s
            .get(node.id)
            .unwrap()
            .unwrap()
            .status
            .is_terminal())
        .await
    );
    assert_eq!(status_of(&store, node.id), Status::Done);
    let first = quota_events(&store, node.id, &node_run);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].2, task_core::quota::QuotaMethod::Unknown);
    let after_first = tree_counters_of(&store, root.id);
    assert_eq!(
        (after_first.usd_micros, after_first.unknown_cost_runs),
        (600_000, 0)
    );
    assert!(
        task_core::tree::run_limit_breach(
            &d.config().execution.limits.tree,
            &after_first,
            task_core::NextRun::Worker,
        )
        .is_none(),
        "1 本目だけでは上限に届かない"
    );
    assert_eq!(store.runs_for_task(root.id).unwrap().len(), 1);
    assert_eq!(limit_decisions(&store, root.id), 0);

    // 2 本目（R）の終わり: group が閉じ、C と R の両方の見積りが確定する。R の continuation の判定は 2 本目の後の値。
    adapter.release(root.id);
    let s = store.clone();
    assert!(run_until_state(&mut d, || limit_decisions(&s, root.id) > 0).await);
    let report = d.tick().unwrap();
    assert_eq!((report.dispatched, report.reviewed), (0, 0), "{report:?}");
    assert_eq!(limit_decisions(&store, root.id), 1);
    assert_eq!(store.runs_for_task(root.id).unwrap().len(), 1);
    assert!(store.get(root.id).unwrap().unwrap().lease.is_none());
    let decisions = store.decisions_list(Some(root.id)).unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0]
            .request
            .question
            .contains("費用不明の run が 1 件"),
        unknown,
        "{}",
        decisions[0].request.question
    );

    // 帳簿: runs 索引の usage は 2 本とも入り、木の費用は 2 本の和（または費用不明 1 件）。
    let after_second = tree_counters_of(&store, root.id);
    if unknown {
        assert_eq!(
            (after_second.usd_micros, after_second.unknown_cost_runs),
            (600_000, 1)
        );
    } else {
        assert_eq!(
            (after_second.usd_micros, after_second.unknown_cost_runs),
            (1_200_000, 0)
        );
    }
    for (id, run) in [(node.id, &node_run), (root.id, &root_run)] {
        let row = store
            .runs_for_task(id)
            .unwrap()
            .into_iter()
            .find(|r| &r.run_id == run)
            .unwrap();
        assert_ne!(row.status, task_core::RunIndexStatus::Running, "{row:?}");
        assert_eq!(row.model.as_deref(), Some(PRICED_MODEL), "{row:?}");
    }
    // quota の帳簿: group が閉じたとき、先に終わった C の run にも按分の event が足される（最後の event が有効）。
    // R の run の event は 1 件で、両方とも按分（apportioned）で account `a` に付く。
    let node_events = quota_events(&store, node.id, &node_run);
    let root_events = quota_events(&store, root.id, &root_run);
    assert_eq!(node_events.len(), 2, "{node_events:?}");
    assert_eq!(root_events.len(), 1, "{root_events:?}");
    let expected_weighted = |usage: Option<task_core::Usage>| {
        crate::routing_runtime::quota_estimate::weighted_tokens_of(
            &usage.unwrap_or_default(),
            PRICED_MODEL,
            "claude-oauth",
        )
    };
    for (events, weighted) in [
        (
            &node_events,
            expected_weighted(Some(input_usage(RUN_INPUT_TOKENS))),
        ),
        (
            &root_events,
            expected_weighted((!unknown).then(|| input_usage(RUN_INPUT_TOKENS))),
        ),
    ] {
        let last = events.last().unwrap();
        assert_eq!(last.0.as_deref(), Some("a"));
        assert!((last.1 - weighted).abs() < 1e-9, "{events:?}");
        assert_eq!(
            last.2,
            task_core::quota::QuotaMethod::Apportioned,
            "{events:?}"
        );
    }
    assert!(!store.get(root.id).unwrap().unwrap().status.is_terminal());
}

/// 不変条件: （ADR-0074 D4.2・ADR 2026-11-06-budget-usd-caps D2、budget-unknown-cost-stops）同じ account を使う run 2 本が
/// 重なり（同じ quota group）、終わりが続けて届いたとき、先の run の使用量も後の run の使用量も帳簿に入る（runs 索引の
/// usage と model、`QuotaEstimated` は group が閉じた時点で 2 本とも按分で確定し、先の run の暫定の event を上書きする）。
/// 木の USD 上限の判定は 2 本目の後の値（2 本の和）で決まり、1 本目だけの値（上限の内側）で次の run を起こさない。
/// 既存の試験で検出できない理由: `quota_estimate.rs` の `quota_estimate_closed_group_*` は見積りの純粋関数に作った group を
/// 渡す単体試験で、tick の経路で 2 本の run を同じ account に重ねて順に終えることも、group が閉じたときに先の run の task へ
/// 按分の event が足されることも、その後の木の上限の判定も通さない。`tree_gate.rs` と `cex_budget_tree_usd_limit_holds_continuation`
/// は終わった run を store に直接書き、run は 1 本ずつで quota group を作らない。
#[tokio::test]
async fn cex_quota_group_two_consecutive_ends_are_both_counted() {
    consecutive_group_ends(false).await;
}

/// 不変条件: （budget-unknown-cost-stops）同じ quota group の 2 本目の run が usage を返さない（費用不明）とき、1 本目の
/// 既知の費用が上限の内側でも、2 本目の後の判定で木を止め、決定要求に費用不明の件数を書く。費用不明の run を 0 USD に
/// 倒さず、group の按分の event も 2 本とも残す。
/// 既存の試験で検出できない理由: `cex_budget_unknown_cost_holds_continuation` は単価の無い model の run を store に直接
/// 書き、run の終わりの経路（quota group の確定と runs 索引の usage の記録）を通さない。
#[tokio::test]
async fn cex_quota_group_two_consecutive_ends_unknown_cost_stops() {
    consecutive_group_ends(true).await;
}

fn status_of(store: &Arc<dyn TaskStore>, id: TaskId) -> Status {
    store.get(id).unwrap().unwrap().status
}

// ---------------------------------------------------------------------------
// (i) 人の replan・段の repair が先に確定した後に、旧い統合・review の完了が遅れて届く
// ---------------------------------------------------------------------------

/// `PhaseIntegrated` の各 event の取り込んだ key の列。
fn phase_integrated_keys(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<Vec<String>> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::PhaseIntegrated { merged, .. } => {
                Some(merged.into_iter().map(|m| m.key).collect::<Vec<_>>())
            }
            _ => None,
        })
        .collect()
}

/// 段 `build`（WU `a`・`b`）の v2 計画。`a` の検査は統合の作業ツリー（`a.txt` と `b.txt` が揃う）で最初に走ったときだけ、
/// 試験が `<marks>/open` を置くまで（または `marks` が消えるまで）止まる。止まったことは `<marks>/first` で分かる。
fn adopt_blocking_plan(store: &Arc<dyn TaskStore>, id: TaskId, marks: &Path) {
    let mut a = v2_wu("a", "build", &[]);
    a.checks = vec![task_core::WorkUnitCheck {
        cmd: format!(
            "if [ -f a.txt ] && [ -f b.txt ] && mkdir {first} 2>/dev/null; then \
             while [ ! -f {open} ] && [ -d {marks} ]; do sleep 0.05; done; fi; test -f a.txt",
            first = marks.join("first").display(),
            open = marks.join("open").display(),
            marks = marks.display(),
        ),
        expect_exit: 0,
        scope: false,
    }];
    adopt_v2_plan(store, id, &["build"], vec![a, v2_wu("b", "build", &[])]);
}

/// 段 `build` の repair（planner の replan が足す `kind = repair` の unit。Task の worktree で走り、`r.txt` を書く）。
const STAGE_REPAIR_KEY: &str = "repair-build-1";

/// `key` の WU が done になった最初の event の位置。
fn wu_done_at(events: &[Event], key: &str) -> Option<usize> {
    events.iter().position(|e| {
        matches!(e, Event::WorkUnitTransitioned { key: k, to: WorkUnitStatus::Done, .. } if k == key)
    })
}

/// Task が `to` に遷移した最初の event の位置。
fn task_reached_at(events: &[Event], to: Status) -> Option<usize> {
    events
        .iter()
        .position(|e| matches!(e, Event::Transitioned { to: t, .. } if *t == to))
}

/// 不変条件: （ADR-0074 D1.4・D1.7 と D6.2、CLAUDE.md「完了はレビュアーか決定的な検査が決める」）段 `build` の統合の
/// 検査が走っている間に段の repair（`kind = repair` の unit。`RepairScheduled{origin: planner}`）が先に確定したら、
/// 遅れて届いた旧い統合の完了（repair の前の HEAD の候補の合格）は捨てる。repair の done の前には `PhaseIntegrated` を
/// 付けず、統合 WU を done にせず、Task を Reviewing・Done に進めない。`PhaseIntegrated` は repair の done の後に、
/// repair の commit（`r.txt`）を含む HEAD の統合にだけ付く。Task は repair を含む候補で Done になる。
/// 既存の試験で検出できない理由: `cex_late_review_and_integration_after_replan_are_dropped` は replan が葉の WU を足す
/// 変種で、足した WU は自分のブランチを持ち、`PhaseIntegrated.merged` の key で新旧の統合を見分けられる。repair の unit は
/// Task の worktree で走って merge の item にならないので、旧い統合の完了を新しい統合と取り違えても key では分からない
/// （この試験は HEAD に repair の commit があるかで見分ける）。`cex_integration_sync_conflict_schedules_one_repair_*` と
/// `cex_integration_restart_*` は統合の結果そのものが repair を作る順序と再起動の順序だけで、統合の検査が走っている間に
/// 段の repair が確定する順序を作らない。task-ops の `replan_records_repair_scheduled_for_a_planner_authored_repair_unit`
/// は store の行と event だけを見て、dispatcher の統合の完了を通さない。
#[tokio::test]
async fn cex_late_integration_after_stage_repair_is_dropped() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let marks = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // Task の検査は repair の前の HEAD でも通る（旧い統合の候補は検査に受かる。捨てるのは古さの判定だけ）。
    let task = parallel_task(repo.path(), "test -f a.txt && test -f b.txt");
    let id = task.id;
    store.insert(&task).unwrap();
    adopt_blocking_plan(&store, id, marks.path());
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::ZERO)
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b")
            .with_file(STAGE_REPAIR_KEY, "r.txt", "r")
            .holding(STAGE_REPAIR_KEY),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 2, 2);
    // 段 `build` の統合の検査が `a.txt` と `b.txt` の揃った作業ツリーで止まるまで進める。
    let first = marks.path().join("first");
    let s = store.clone();
    assert!(
        run_until_state(&mut d, || first.is_dir()
            && events_of(&s, id)
                .iter()
                .any(|e| matches!(e, Event::IntegrationCheckStarted { .. })))
        .await
    );
    assert!(phase_integrated_keys(&store, id).is_empty());
    assert_eq!(
        wu_status(&store, id, "integrate-build"),
        Some(WorkUnitStatus::Running)
    );
    // 段 `build` の repair を先に確定する（planner の replan が `kind = repair` の unit を足す。done の `a`・`b` は変えない）。
    LateChange::StageRepair.apply(&store, id);
    // 旧い統合（repair の前の HEAD の候補）の完了を届ける。repair の run は adapter の gate で止めておく。
    std::fs::write(marks.path().join("open"), "").unwrap();
    let seen = adapter.clone();
    assert!(
        run_until_state(&mut d, || seen
            .keys_seen()
            .iter()
            .any(|k| k == STAGE_REPAIR_KEY))
        .await
    );
    // 統合の process が残る間は Task の WU を起こさないので、repair の run の開始は旧い統合の完了の後。
    // repair の done の前: 旧い完了は段を統合済みにせず、統合 WU と Task を進めない。
    let events = events_of(&store, id);
    let summary = |events: &[Event]| {
        format!(
            "repair_done={:?} integrated={:?} integ_done={:?} reviewing={:?} done={:?} status={:?} keys={:?}",
            wu_done_at(events, STAGE_REPAIR_KEY),
            phase_integrated_keys(&store, id),
            wu_done_at(events, "integrate-build"),
            task_reached_at(events, Status::Reviewing),
            task_reached_at(events, Status::Done),
            store.get(id).unwrap().unwrap().status,
            adapter.keys_seen(),
        )
    };
    assert!(
        wu_done_at(&events, STAGE_REPAIR_KEY).is_none(),
        "{}",
        summary(&events)
    );
    assert!(
        phase_integrated_keys(&store, id).is_empty(),
        "旧い統合の完了が repair の done の前に PhaseIntegrated を付けた: {}",
        summary(&events)
    );
    assert!(
        wu_done_at(&events, "integrate-build").is_none(),
        "{}",
        summary(&events)
    );
    assert!(
        task_reached_at(&events, Status::Reviewing).is_none()
            && task_reached_at(&events, Status::Done).is_none(),
        "{}",
        summary(&events)
    );
    // repair を終える（gate の permit は待ちの前に出しても残る）。Task が終端になるまで進める。
    adapter.gate.notify_one();
    let s = store.clone();
    assert!(run_until_state(&mut d, || s.get(id).unwrap().unwrap().status.is_terminal()).await);
    let _ = std::fs::remove_dir_all(marks.path());
    let events = events_of(&store, id);
    let repair_done = wu_done_at(&events, STAGE_REPAIR_KEY)
        .unwrap_or_else(|| panic!("repair never done: {}", summary(&events)));
    let integrated: Vec<(usize, String)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Event::PhaseIntegrated { head, .. } => Some((i, head.clone())),
            _ => None,
        })
        .collect();
    assert!(!integrated.is_empty(), "{}", summary(&events));
    for (i, head) in &integrated {
        assert!(
            *i > repair_done && git_ok(repo.path(), &["cat-file", "-e", &format!("{head}:r.txt")]),
            "PhaseIntegrated が repair を含まない統合に付いた（head {head}）: {}",
            summary(&events)
        );
    }
    assert!(
        wu_done_at(&events, "integrate-build").is_some_and(|i| i > repair_done),
        "{}",
        summary(&events)
    );
    assert!(
        task_reached_at(&events, Status::Reviewing).is_some_and(|i| i > repair_done),
        "{}",
        summary(&events)
    );
    assert_eq!(
        store.get(id).unwrap().unwrap().status,
        Status::Done,
        "{}",
        summary(&events)
    );
    let branch = format!("celeris/{id}");
    assert_eq!(
        git_out(repo.path(), &["show", &format!("{branch}:r.txt")]),
        "r"
    );
}

/// 段 `build` の統合の検査を走らせたまま止め、人の replan（段 `build` に WU `c` を足す）を先に確定してから、
/// 旧い統合の完了を届ける。Task が終端になるまで進め、event の列を返す材料（store・id・repo）を返す。
async fn late_integration_after_human_replan() -> (tempfile::TempDir, Arc<dyn TaskStore>, TaskId) {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let marks = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "test -f a.txt && test -f b.txt");
    let id = task.id;
    store.insert(&task).unwrap();
    adopt_blocking_plan(&store, id, marks.path());
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::ZERO)
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b")
            .with_file("c", "c.txt", "c"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    let first = marks.path().join("first");
    let s = store.clone();
    assert!(
        run_until_state(&mut d, || first.is_dir()
            && events_of(&s, id)
                .iter()
                .any(|e| matches!(e, Event::IntegrationCheckStarted { .. })))
        .await
    );
    assert!(phase_integrated_keys(&store, id).is_empty());
    // 人の replan を先に確定する（段 `build` に WU `c` を足す。done の `a`・`b` の spec は変えない）。
    LateChange::HumanReplan.apply(&store, id);
    // 旧い統合（`a`・`b` だけを取り込んだ検査）の完了を届ける。
    std::fs::write(marks.path().join("open"), "").unwrap();
    let s = store.clone();
    assert!(run_until_state(&mut d, || s.get(id).unwrap().unwrap().status.is_terminal()).await);
    let _ = std::fs::remove_dir_all(marks.path());
    (repo, store, id)
}

/// 不変条件: （ADR-0074 D1.4・D1.7 と D17、CLAUDE.md「完了はレビュアーか決定的な検査が決める」）段の統合の検査が走って
/// いる間に人の replan が先に確定した（段 `build` に WU `c` を足した）なら、遅れて届いた旧い統合の完了（`a`・`b` だけを
/// 取り込んだ候補の合格）は新しい計画の段を統合済みにしない。`PhaseIntegrated` は `c` を取り込んだ統合にだけ付き、
/// 統合 WU の done と Task の最終 review は `c` の done の後に来る。Task は `c` を含む候補で Done になる。
/// 既存の試験で検出できない理由: `work_units.rs` の `a_pending_human_replan_runs_the_planner_before_retrying_the_integration`
/// は統合が終わった後の replan の要求（planner の前に統合をやり直さない）だけを見て、統合の検査が走っている間に replan が
/// 確定し、その後に旧い完了が届く順序を作らない。`cex_integration_restart_*` は再起動の順序だけで、計画は変えない。
#[tokio::test]
async fn cex_late_review_and_integration_after_replan_are_dropped() {
    let (repo, store, id) = late_integration_after_human_replan().await;
    let events = events_of(&store, id);
    let c_done = events
        .iter()
        .position(|e| matches!(e, Event::WorkUnitTransitioned { key, to: WorkUnitStatus::Done, .. } if key == "c"));
    let integrated: Vec<(usize, Vec<String>)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Event::PhaseIntegrated { merged, .. } => {
                Some((i, merged.iter().map(|m| m.key.clone()).collect()))
            }
            _ => None,
        })
        .collect();
    let integ_done = events.iter().position(|e| matches!(e, Event::WorkUnitTransitioned { key, to: WorkUnitStatus::Done, .. } if key == "integrate-build"));
    let reviewing = events.iter().position(|e| {
        matches!(
            e,
            Event::Transitioned {
                to: Status::Reviewing,
                ..
            }
        )
    });
    let summary = format!(
        "c_done={c_done:?} integrated={integrated:?} integ_done={integ_done:?} reviewing={reviewing:?} status={:?}",
        store.get(id).unwrap().unwrap().status
    );
    let c_done = c_done.unwrap_or_else(|| panic!("c never done: {summary}"));
    assert!(!integrated.is_empty(), "{summary}");
    for (i, keys) in &integrated {
        assert!(
            *i > c_done && keys.iter().any(|k| k == "c"),
            "旧い統合の完了が新しい計画の段に PhaseIntegrated を付けた: {summary}"
        );
    }
    assert!(integ_done.is_some_and(|i| i > c_done), "{summary}");
    assert!(reviewing.is_some_and(|i| i > c_done), "{summary}");
    assert_eq!(
        store.get(id).unwrap().unwrap().status,
        Status::Done,
        "{summary}"
    );
    let branch = format!("celeris/{id}");
    assert_eq!(
        git_out(repo.path(), &["show", &format!("{branch}:c.txt")]),
        "c"
    );
}

// ---------------------------------------------------------------------------
// (i) 人の replan・段の repair が先に確定した後に、旧い最終 review の合格が遅れて届く
// ---------------------------------------------------------------------------

/// WU の run は [`ParallelWuAdapter`] に任せ、reviewer run（`TaskKind::Review`）は試験が gate（semaphore）を開けるまで
/// 止めてから全条件の合格を `review.json` に書く adapter（`cex_review_authority.rs` の `ReviewGateAdapter` と同じ止め方と
/// 同じ `review.json`）。
struct ReviewGateWuAdapter {
    units: ParallelWuAdapter,
    /// review の候補（Task のブランチ `branch`）を読む repo。
    repo: PathBuf,
    branch: String,
    gate: tokio::sync::Semaphore,
    /// reviewer run ごとの `(run_id, 起動時の Task のブランチの commit)`。
    reviews: StdMutex<Vec<(String, String)>>,
}

impl ReviewGateWuAdapter {
    fn new(units: ParallelWuAdapter, repo: &Path, task: TaskId) -> Arc<Self> {
        Arc::new(ReviewGateWuAdapter {
            units,
            repo: repo.to_path_buf(),
            branch: format!("celeris/{task}"),
            gate: tokio::sync::Semaphore::new(0),
            reviews: StdMutex::new(Vec::new()),
        })
    }

    fn reviews(&self) -> Vec<(String, String)> {
        self.reviews.lock().unwrap().clone()
    }

    /// 止めている reviewer run を 1 本だけ終わらせる。
    fn release_one(&self) {
        self.gate.add_permits(1);
    }

    /// 以後の reviewer run を止めない（semaphore を閉じると `acquire` は待たずに `Err` を返す）。
    fn open_for_good(&self) {
        self.gate.close();
    }
}

#[async_trait]
impl WorkerAdapter for ReviewGateWuAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        if req.task.kind != TaskKind::Review {
            return self.units.run(req, run_id, limits, sink).await;
        }
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        let head = git_out(&self.repo, &["rev-parse", &self.branch]);
        self.reviews
            .lock()
            .unwrap()
            .push((run_id.to_string(), head));
        if let Ok(permit) = self.gate.acquire().await {
            permit.forget();
        }
        write_review_json(&req.artifacts_dir, true);
        Ok(done_outcome())
    }
}

/// 段 `build` に足す unit（試験が先に確定する計画の変更）。
#[derive(Clone, Copy, Debug)]
enum LateChange {
    /// 人の replan（`PlanOrigin::Human`）が葉の WU `c`（`c.txt` を書く）を足す。
    HumanReplan,
    /// planner の replan（`PlanOrigin::Planner`）が段の repair（`kind = repair` の [`STAGE_REPAIR_KEY`]。Task の worktree で
    /// 走り、`r.txt` を書く）を足す。`RepairScheduled{origin: planner}` が記録される。
    StageRepair,
}

impl LateChange {
    fn key(self) -> &'static str {
        match self {
            LateChange::HumanReplan => "c",
            LateChange::StageRepair => STAGE_REPAIR_KEY,
        }
    }

    fn file(self) -> &'static str {
        match self {
            LateChange::HumanReplan => "c.txt",
            LateChange::StageRepair => "r.txt",
        }
    }

    /// 段 `build` にこの unit を足す replan を確定する（done の `a`・`b` の spec は変えない）。
    fn apply(self, store: &Arc<dyn TaskStore>, id: TaskId) {
        let mut spec = store.execution_plan_active(id).unwrap().unwrap().spec;
        let mut unit = v2_wu(self.key(), "build", &[]);
        let (origin, reason) = match self {
            LateChange::HumanReplan => (PlanOrigin::Human, "human replan: add c to build"),
            LateChange::StageRepair => {
                unit.kind = WorkUnitKind::Repair;
                (PlanOrigin::Planner, "planner repair: fix the build stage")
            }
        };
        spec.work_units.push(unit);
        task_ops::execution::replan(
            store.as_ref(),
            id,
            spec,
            reason.to_string(),
            origin,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        if let LateChange::StageRepair = self {
            assert!(
                events_of(store, id).iter().any(|e| matches!(
                    e,
                    Event::RepairScheduled {
                        key,
                        origin: task_core::execution::RepairOrigin::Planner,
                        ..
                    } if key == STAGE_REPAIR_KEY
                )),
                "{:?}",
                events_of(store, id)
            );
        }
    }
}

/// v2 の計画（段 `build` の WU `a`・`b`）の Task が段の統合を終えて最終 review に入り、reviewer run を gate で止めて
/// いる間に `change` を確定し、その後に旧い reviewer の合格を届ける。旧い合格の後の assert をしてから、gate を開けて
/// Task が終端になるまで進め、新しい unit を含む候補の review だけが Task を Done にしたことを assert する。
async fn late_review_pass_after(change: LateChange) {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // 案件の repo を持つ自動取り込みの root task（`cex_review_authority.rs` と同じ組織。review 前の同期が候補を束縛し、
    // `ReviewerPassed` に候補の commit が残る）。この fixture は Task の worker run を経ない（`adopt_v2_plan`）ので
    // `worktree.json` が無く、配送の行は `DeliverySkipped{NoMarker}` で作られない。`MergeQueued` の assert は、配送の行が
    // できる経路（planner run が目印を書く本番の順序）でも旧い合格が行を進めないことを守る。Task の検査は変更の前の候補
    // でも通る（旧い review は検査に受かる。捨てるのは古さの判定だけ）。条件 1 は reviewer。
    let mut task = git_review_task(repo.path(), &store, "test -f a.txt && test -f b.txt".into());
    deliverable(&store, &mut task);
    let id = task.id;
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &[])],
    );
    let adapter = ReviewGateWuAdapter::new(
        ParallelWuAdapter::new(Duration::ZERO)
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b")
            .with_file(change.key(), change.file(), change.key()),
        repo.path(),
        id,
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 2, 2);
    // 段 `build` の統合を終えて最終 review に入り、reviewer run が gate で止まるまで進める。
    let (s, a) = (store.clone(), adapter.clone());
    assert!(
        run_until_state(&mut d, || !a.reviews().is_empty()
            || status_of(&s, id).is_terminal())
        .await
    );
    assert_eq!(
        status_of(&store, id),
        Status::Reviewing,
        "{:?}",
        transitions(&store, id)
    );
    let (old_review, old_head) = adapter.reviews()[0].clone();
    let candidate_has = |head: &str| {
        git_ok(
            repo.path(),
            &["cat-file", "-e", &format!("{head}:{}", change.file())],
        )
    };
    assert!(!candidate_has(&old_head), "{old_head}");
    assert_eq!(
        wu_status(&store, id, "integrate-build"),
        Some(WorkUnitStatus::Done)
    );

    // 計画の変更を先に確定してから、旧い reviewer の合格を届ける。
    change.apply(&store, id);
    adapter.release_one();
    let s = store.clone();
    assert!(
        run_until_state(&mut d, || !reviewer_finishes(&s, id, &old_review)
            .is_empty()
            || status_of(&s, id).is_terminal())
        .await
    );
    let summary = || {
        let events = events_of(&store, id);
        format!(
            "unit_done={:?} status={:?} transitions={:?} reviews={:?} old_end={:?} reviewer_passed={:?} \
             delivery={:?} delivery_skipped={:?} units={:?}",
            wu_done_at(&events, change.key()),
            status_of(&store, id),
            transitions(&store, id),
            adapter.reviews(),
            reviewer_finishes(&store, id, &old_review),
            reviewer_passes(&events),
            store.delivery_get(id).unwrap().map(|row| row.state),
            events
                .iter()
                .filter_map(|e| match e {
                    Event::DeliverySkipped { reason, detail, .. } => Some((reason, detail)),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            store
                .work_units_for(id)
                .unwrap()
                .iter()
                .map(|u| (u.key.clone(), u.status))
                .collect::<Vec<_>>(),
        )
    };
    // 旧い合格は Task を Done にせず、配送の行を `MergeQueued` にしない。
    assert!(
        !reached_done(&store, id) && status_of(&store, id) != Status::Done,
        "旧い reviewer の合格が計画の変更の後の Task を Done にした: {}",
        summary()
    );
    assert!(
        !merge_queued(&store, id),
        "旧い reviewer の合格が配送の行を MergeQueued にした: {}",
        summary()
    );
    assert_old_pass_stays_on(&store, id, &old_review, &old_head);

    // 新しい unit を走らせ、Task が終端になるまで進める（以後の reviewer run は gate で止めない）。
    adapter.open_for_good();
    let s = store.clone();
    assert!(
        run_until_state(&mut d, || status_of(&s, id).is_terminal()).await,
        "{}",
        summary()
    );
    let events = events_of(&store, id);
    let unit_done = wu_done_at(&events, change.key())
        .unwrap_or_else(|| panic!("{} never done: {}", change.key(), summary()));
    assert_eq!(status_of(&store, id), Status::Done, "{}", summary());
    // Done にした review は新しい unit の done の後に起きた reviewer run で、その候補は新しい unit を含む。
    assert!(
        task_reached_at(&events, Status::Done).is_some_and(|i| i > unit_done),
        "{}",
        summary()
    );
    let last_reviewer_start = events.iter().rposition(|e| {
        matches!(
            e,
            Event::WorkerStarted {
                role: Some(RunRole::Reviewer),
                ..
            }
        )
    });
    assert!(
        last_reviewer_start.is_some_and(|i| i > unit_done),
        "{}",
        summary()
    );
    let reviews = adapter.reviews();
    let (last_review, last_head) = reviews.last().unwrap();
    assert_ne!(last_review, &old_review, "{}", summary());
    assert!(candidate_has(last_head), "{}", summary());
    assert_old_pass_stays_on(&store, id, &old_review, &old_head);
    // Done の時点で、有効な計画の unit（統合 WU を含む）はすべて done。
    let unfinished: Vec<(String, WorkUnitStatus)> = store
        .work_units_for(id)
        .unwrap()
        .into_iter()
        .filter(|u| u.status.is_active() && u.status != WorkUnitStatus::Done)
        .map(|u| (u.key, u.status))
        .collect();
    assert!(unfinished.is_empty(), "{unfinished:?}: {}", summary());
}

/// `ReviewerPassed` の `(review_run, reviewed_sha)` の列（event の順）。
fn reviewer_passes(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ReviewerPassed {
                review_run,
                reviewed_sha,
                ..
            } => Some((review_run.clone(), reviewed_sha.clone())),
            _ => None,
        })
        .collect()
}

/// 旧い reviewer run `old_review` の合格は、変更の前の候補 `old_head` にだけ束縛して残る（`ReviewerPassed` の
/// `reviewed_sha` は `old_head`）。新しい候補に付けず、`ReviewerPassReused` で新しい候補に持ち越さない。
fn assert_old_pass_stays_on(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
    old_review: &str,
    old_head: &str,
) {
    let events = events_of(store, id);
    for (run, sha) in reviewer_passes(&events) {
        assert!(
            run != old_review || sha == old_head,
            "旧い reviewer の合格が別の候補 {sha} に付いた（変更の前の候補は {old_head}）"
        );
    }
    let reused: Vec<&Event> = events
        .iter()
        .filter(|e| {
            matches!(e, Event::ReviewerPassReused { from_reviewed_sha, .. } if from_reviewed_sha == old_head)
        })
        .collect();
    assert!(
        reused.is_empty(),
        "旧い reviewer の合格を新しい候補に再利用した: {reused:?}"
    );
}

/// 不変条件: （ADR-0074 D1.4・D1.7 と D17、ADR-0118 D3〜D5、CLAUDE.md「完了はレビュアーか決定的な検査が決める」）v2 の
/// 計画の Task の最終 review で reviewer run が候補（段 `build` の `a`・`b` だけを統合した Task のブランチ）を検査している
/// 間に人の replan（段 `build` に WU `c` を足す）が確定したら、遅れて届いた旧い reviewer の合格は捨てる。旧い合格は Task を
/// Done にせず、配送の行を `MergeQueued` にせず、`ReviewerPassed`・`ReviewerPassReused` で `c` を含む候補に付かない。
/// Task を Done にできるのは、`c` の done（と段の統合のやり直し）の後に起きた reviewer run の、`c.txt` を含む候補の合格だけで、
/// Done の時点で有効な計画の unit はすべて done。
/// 既存の試験で検出できない理由: `cex_late_review_and_integration_after_replan_are_dropped` と
/// `cex_late_integration_after_stage_repair_is_dropped` は統合の検査が走っている間に計画を変え、Task が Reviewing に入る前の
/// 順序だけを見る。`cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate` は reviewer を止めている
/// 間に target を進める（候補の commit が変わる）順序で、計画は変えない（候補の commit も検査後の snapshot も変わらないまま
/// 計画だけが変わる順序を作らない）。`stage_reopen.rs` の試験は統合済みの段に unit を足す replan を、Task が Running の
/// 間（最終 review の前）に確定する。
#[tokio::test]
async fn cex_late_review_pass_after_replan_is_dropped() {
    late_review_pass_after(LateChange::HumanReplan).await;
}

/// 不変条件: （ADR-0074 D6.2 と D1.4・D1.7、ADR-0118 D3〜D5、CLAUDE.md「完了はレビュアーか決定的な検査が決める」）v2 の
/// 計画の Task の最終 review で reviewer run が候補を検査している間に段 `build` の repair（planner の replan が足す
/// `kind = repair` の unit。`RepairScheduled{origin: planner}`）が確定したら、遅れて届いた旧い reviewer の合格は捨てる。
/// 旧い合格は Task を Done にせず、配送の行を `MergeQueued` にせず、`ReviewerPassed`・`ReviewerPassReused` で repair の
/// commit（`r.txt`）を含む候補に付かない。Task を Done にできるのは、repair の done の後に起きた reviewer run の、
/// `r.txt` を含む候補の合格だけで、Done の時点で有効な計画の unit はすべて done。
/// 既存の試験で検出できない理由: `cex_late_integration_after_stage_repair_is_dropped` は統合の検査が走っている間に repair を
/// 確定し、最終 review の前の順序だけを見る。`cex_integration_sync_conflict_schedules_one_repair_*` と review.rs の
/// `review_repair_*` は review の結果そのものが repair を作る順序（daemon の `try_review_repair`）で、reviewer run が
/// 走っている間に外から repair が確定する順序を作らない。task-ops の
/// `replan_records_repair_scheduled_for_a_planner_authored_repair_unit` は store の行と event だけを見て、dispatcher の
/// review の完了を通さない。
#[tokio::test]
async fn cex_late_review_pass_after_repair_is_dropped() {
    late_review_pass_after(LateChange::StageRepair).await;
}
