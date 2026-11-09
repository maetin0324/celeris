//! ADR 2026-11-08-health-enforcement-loop D4.1・D4.2・D4.4・J4・J6: health 由来の修復・再 review・評価の時間が、新しい欄を
//! 足さずに既存の上限の中で数えられることを dispatcher の流れで確かめる。上限ごとの対応は
//! `agent-docs/progress/2026-11-07-health-v2-g3/limits-budgets.md` の表。
//!
//! 既存の `health_limits_review_*` は修復の上限を health の修復だけで使い切る場面と Error の止め方を見て、health でない
//! 修復との合算・rule ごとの bucket・木の上限・token 予算・review 前の同期の上限・時間切れ・使い切りの後の error /
//! unknown / unsupported を通らない。ここはそれを確かめる。
//!
//! 検査器は試験の repo の base に commit した stub（[`STUB`]）。作業ツリー（`--repo`）の file で結果を決める:
//! `FIXED` で合格、`STATUS`（`error`・`unknown`・`unsupported`）で exit 2、`FINDING` で fail 級の指摘の fingerprint
//! （rule は fingerprint の `:` の前）、`HANG` で終わらない（時間切れの試験。上限は [`test_timeout::cap_under`] で縮める）。
//! 時計は使わず、`run_until_idle`（出来事が尽きるまで tick を回す）で決まる。外部ネットワークに出ず、LLM を呼ばない。
use super::health_enforce::{RepairActionAdapter, non_celeris_dispatcher};
use super::target_sync::{commit_file, reviewing_task, set_check};
use super::*;
use crate::health_review::gate::test_timeout;
use task_core::required_checks::health::{
    CHECKER_SCRIPT, HEALTH_CHECK_ID, HealthVerdict, SOURCE_SIZE_REPORT,
};
use task_ops::human_inbox::InboxKind;

/// base の stub の検査器（上の作業ツリーの file で結果を決める。exit 2 でも report を書く）。
const STUB: &str = r#"import json, os, sys, time
here = os.path.dirname(os.path.abspath(__file__))
argv = sys.argv[1:]
if "--print-review-policy" in argv:
    sys.stdout.write(json.dumps({"schema": "celeris.repo-health.review-policy/1", "source": "absent",
        "owner_map": None, "high_risk": [], "invariants": {}, "definition_sha256": "0" * 64}))
    sys.exit(0)
repo = argv[argv.index("--repo") + 1]
def read(name):
    p = os.path.join(repo, name)
    return open(p).read().strip() if os.path.exists(p) else None
if read("HANG") is not None:
    time.sleep(3600)
with open(os.path.join(here, "stub.json")) as f:
    stub = json.load(f)
status = read("STATUS")
code = 0 if read("FIXED") is not None else (2 if status else stub["exit"])
fp = read("FINDING") or "deps.violation:0000000000000001"
rule = fp.rsplit(":", 1)[0]
findings = [] if code != 1 else [{"rule_id": rule, "fingerprint": fp,
    "severity": "error", "delta": "new", "exception": None, "evidence": [{"path": "src/lib.rs"}]}]
report = {"schema": "celeris.repo-health.report/1", "tool": {"name": "repo_health.py", "version": "stub"},
    "inputs": {"profile": {"resolved": "builtin", "source": "builtin"}},
    "policy": {"version": "1", "source": "builtin", "changed": False, "files": []},
    "rules": [{"id": rule, "required": True, "status": "pass" if code == 0 else (status or "fail"), "reason": ""}],
    "findings": findings,
    "summary": {"verdict": "pass" if code == 0 else ("fail" if code == 1 else "error"), "exit_code": code,
        "reviewable": True, "counts": {"findings": len(findings), "fail_grade": len(findings), "new": len(findings),
        "worsened": 0, "existing": 0}}}
with open(argv[argv.index("--out") + 1], "w") as f:
    json.dump(report, f)
sys.exit(code)
"#;

/// 本物の `health/engine.py` と同じ判定。
const ENGINE: &str = r#"def is_fail_grade(f):
    return f["severity"] == "error" and f["delta"] in ("new", "worsened") and f["exception"] is None


def policy_review_files(report, changed):
    return None
"#;

/// 試験の repo の base に [`STUB`]（既定は exit 1）を commit する。
fn commit_checker(repo: &Path) {
    let dir = repo.join("scripts/dev/repo-health");
    std::fs::create_dir_all(dir.join("health")).unwrap();
    std::fs::write(repo.join(CHECKER_SCRIPT), STUB).unwrap();
    std::fs::write(dir.join("stub.json"), "{\"exit\": 1}").unwrap();
    std::fs::write(dir.join("health/engine.py"), ENGINE).unwrap();
    std::fs::write(dir.join("health/__init__.py"), "").unwrap();
    let side = repo.join(SOURCE_SIZE_REPORT);
    std::fs::create_dir_all(side.parent().unwrap()).unwrap();
    std::fs::write(side, "# side\n").unwrap();
    git_out(repo, &["add", "-A"]);
    git_out(repo, &["commit", "-q", "-m", "checker"]);
}

/// 修復の WU の run を [`RepairActionAdapter`] で回し、修復の run の `Terminal::Done` に `repair_tokens` の usage を載せる
/// adapter（token 予算の試験。ほかの run は usage なし）。
struct UsageAdapter {
    inner: RepairActionAdapter,
    repair_tokens: u64,
}

#[async_trait]
impl WorkerAdapter for UsageAdapter {
    fn id(&self) -> &str {
        self.inner.id()
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let repair = (req.context.work_unit.as_ref()).is_some_and(|w| w.key.starts_with("repair-"));
        let mut out = self.inner.run(req, run_id, limits, sink).await?;
        if repair && let Terminal::Done { usage, .. } = &mut out.terminal {
            *usage = Some(task_core::Usage {
                input_tokens: Some(self.repair_tokens),
                output_tokens: Some(0),
                ..Default::default()
            });
        }
        Ok(out)
    }
}

/// 1 件の review の場面（`Reviewing` の atomic の task と、その作業ツリー）。
struct Scenario {
    _repo: tempfile::TempDir,
    ws: tempfile::TempDir,
    store: Arc<dyn TaskStore>,
    task: Task,
    d: Dispatcher,
    dir: PathBuf,
}

impl Scenario {
    /// `check` は acceptance の cmd、`prepare` は task を入れる前に task を変える、`on_repair` は修復の WU の run の後に
    /// 作業ツリーで呼ぶ。`repair_tokens` は修復の run の usage。
    fn new(
        check: &str,
        prepare: impl FnOnce(&mut Task),
        repair_tokens: u64,
        on_repair: impl Fn(&Path, &str) + Send + Sync + 'static,
    ) -> Self {
        let repo = tempfile::tempdir().unwrap();
        init_test_repo(repo.path());
        commit_checker(repo.path());
        let ws = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let mut task = reviewing_task(repo.path(), &store);
        set_check(&mut task, check.to_string());
        prepare(&mut task);
        store.insert(&task).unwrap();
        let adapter = Arc::new(UsageAdapter {
            inner: RepairActionAdapter::new(on_repair),
            repair_tokens,
        });
        let d = non_celeris_dispatcher(store.clone(), adapter, ws.path(), 1);
        let wt = d.local_worktree_for(&task).unwrap();
        wt.ensure_blocking().unwrap();
        std::fs::create_dir_all(wt.dir.join("src")).unwrap();
        commit_file(&wt.dir, "src/lib.rs", "pub fn foo() {}\n");
        let dir = wt.dir.clone();
        Self {
            _repo: repo,
            ws,
            store,
            task,
            d,
            dir,
        }
    }

    fn atomic(on_repair: impl Fn(&Path, &str) + Send + Sync + 'static) -> Self {
        Self::new("true", |_| {}, 0, on_repair)
    }

    /// worker の run の後の review を起こし、出来事が尽きるまで回す（終わらない待ちがあれば idle にならず落ちる）。
    async fn review(&mut self) {
        assert!(
            self.d
                .spawn_review(self.task.id, "worker".into(), &ReviewSubject::default())
                .unwrap()
        );
        self.settle().await;
    }

    async fn settle(&mut self) {
        assert!(run_until_idle(&mut self.d, 800).await.idle);
    }

    fn stored(&self) -> Task {
        self.store.get(self.task.id).unwrap().unwrap()
    }

    fn events(&self) -> Vec<Event> {
        events_of(&self.store, self.task.id)
    }

    fn repair_titles(&self) -> Vec<String> {
        (self.store.work_units_for(self.task.id).unwrap().into_iter())
            .filter(|u| u.kind == task_core::WorkUnitKind::Repair)
            .map(|u| u.spec.title)
            .collect()
    }

    /// 人の受信箱の項目の種類（この task のもの）。
    fn inbox_kinds(&self) -> Vec<InboxKind> {
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
        .filter(|i| i.task.as_ref().is_some_and(|t| t.id == self.task.id))
        .map(|i| i.kind)
        .collect()
    }

    /// 人が comment で再開する（`Trigger::Answer`）。
    fn comment(&self, body: &str) {
        let result = task_ops::comment::post_human_comment_as(
            self.store.as_ref(),
            self.task.id,
            Some("human:web".into()),
            body.into(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let resumed = result.transition.expect("the comment answers the question");
        assert_eq!((resumed.from, resumed.to), (Status::Blocked, Status::Ready));
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

/// 修復の WU の key（`repair-<n>`）ごとに次の評価の fingerprint を変える（rule は同じ `deps.violation`）。
fn next_finding(dir: &Path, key: &str) {
    let n: u64 = key.trim_start_matches("repair-").parse().unwrap();
    commit_file(dir, "FINDING", &format!("deps.violation:{:016x}\n", n + 1));
}

fn count(events: &[Event], pred: impl Fn(&Event) -> bool) -> usize {
    events.iter().filter(|e| pred(e)).count()
}

fn repairs_scheduled(events: &[Event]) -> usize {
    count(events, |e| matches!(e, Event::RepairScheduled { .. }))
}

fn worker_runs(events: &[Event]) -> usize {
    count(events, |e| matches!(e, Event::WorkerStarted { .. }))
}

fn raised(events: &[Event]) -> Vec<String> {
    (events.iter())
        .filter_map(|e| match e {
            Event::QuestionRaised { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn reasons(events: &[Event]) -> Vec<String> {
    (events.iter())
        .filter_map(|e| match e {
            Event::Transitioned { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

/// review の health の項目の証跡（verdict と reason）の列。
fn health_evidence(events: &[Event]) -> Vec<(HealthVerdict, String)> {
    (events.iter())
        .filter_map(|e| match e {
            Event::RequiredChecksEvaluated { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .filter(|r| r.cmd == HEALTH_CHECK_ID)
        .filter_map(|r| r.health.as_ref())
        .map(|h| (h.verdict, h.reason.clone().unwrap_or_default()))
        .collect()
}

fn verdicts(events: &[Event]) -> Vec<HealthVerdict> {
    health_evidence(events).into_iter().map(|(v, _)| v).collect()
}

/// 上限の使い切りの止まり方（既存の扱い）: `review_blocked` で Blocked（attempts 不変）、受信箱に質問が 1 件、止めた後に
/// run も修復も replan も無い（repair の連鎖を作らない）。`run_until_idle` が idle で返ったので終わらない待ちも無い。
fn assert_stopped(s: &Scenario, question_prefix: &str) -> Vec<Event> {
    let stored = s.stored();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    assert_eq!(stored.attempts, 0, "{stored:?}");
    let events = s.events();
    let last_block = (events.iter())
        .rposition(|e| matches!(e, Event::Transitioned { to: Status::Blocked, reason, .. } if reason == Trigger::ReviewBlocked.name()))
        .unwrap_or_else(|| panic!("no review_blocked: {:?}", reasons(&events)));
    let after = &events[last_block..];
    assert_eq!(worker_runs(after), 0, "no run after the stop: {after:?}");
    assert_eq!(repairs_scheduled(after), 0, "no repair after the stop");
    assert_eq!(
        count(after, |e| matches!(e, Event::AutoReplanRequested { .. })),
        0
    );
    assert!(
        !reasons(&events).contains(&Trigger::ReviewFail.name().to_string()),
        "{:?}",
        reasons(&events)
    );
    let asked = raised(&events);
    let last = asked.last().unwrap_or_else(|| panic!("no question"));
    assert!(last.starts_with(question_prefix), "{asked:?}");
    assert!(
        s.inbox_kinds().contains(&InboxKind::Question),
        "{:?}",
        s.inbox_kinds()
    );
    events
}

/// D4.1（`max_repairs`）: task の修復の合計は health と health でない修復が同じ数で使う。fmt の修復（health でない）で
/// `max_repairs` = 1 を使い切った後の health の Fail は、health の bucket が 0 回でも修復を足さず
/// `health_repair_cap` で止まる。
#[tokio::test]
async fn health_limits_cap_max_repairs_shared_with_non_health_repairs() {
    let mut s = Scenario::new("test -f FMT_OK # cargo fmt --check", |_| {}, 0, |dir, _| {
        commit_file(dir, "FMT_OK", "ok\n");
        git_out(dir, &["rm", "-q", "FIXED"]);
        git_out(dir, &["commit", "-q", "-m", "unfix health"]);
    });
    commit_file(&s.dir, "FIXED", "ok\n");
    s.d.config.execution.max_repairs = 1;
    s.review().await;
    let events = assert_stopped(&s, "health_repair_cap: ");
    assert_eq!(
        verdicts(&events),
        vec![HealthVerdict::Pass, HealthVerdict::Fail]
    );
    assert_eq!(s.repair_titles(), vec!["repair (format): 修復".to_string()]);
    let q = raised(&events).pop().unwrap();
    assert!(q.contains("bucket health:deps.violation: 0 回"), "{q}");
    assert!(q.contains("修復の合計 1 回 / max_repairs 1"), "{q}");
}

/// D4.1（`max_repairs_per_class`）: health の修復は rule ごと（`health:<rule_id>`）に数える。`health:deps.violation` を
/// 1 回で使い切っても `health:size.growth` の指摘には修復が出る。同じ rule に戻ると、その bucket の上限で止まる。
#[tokio::test]
async fn health_limits_cap_per_class_counts_each_rule() {
    let mut s = Scenario::atomic(|dir, key| {
        let next = match key {
            "repair-1" => "size.growth:0000000000000001",
            _ => "deps.violation:0000000000000002",
        };
        commit_file(dir, "FINDING", &format!("{next}\n"));
    });
    s.d.config.execution.max_repairs_per_class = 1;
    s.review().await;
    let events = assert_stopped(&s, "health_repair_cap: ");
    assert_eq!(
        s.repair_titles(),
        vec![
            "repair (health:deps.violation): 修復".to_string(),
            "repair (health:size.growth): 修復".to_string(),
        ]
    );
    assert_eq!(verdicts(&events), vec![HealthVerdict::Fail; 3]);
    let q = raised(&events).pop().unwrap();
    assert!(
        q.contains("bucket health:deps.violation: 1 回 / max_repairs_per_class 1"),
        "{q}"
    );
}

/// 木の上限の場面: 木の節点（`tree` を持つ root）で、修復の前に木の run・費用を `seed` で積んでおく。health の Fail の
/// 修復の WU は作られるが、その run は `dispatch_one` の `tree_limit_hold_for` で起きず、root に `limit:<key>` の決定の
/// 要求が 1 件だけ出る（既存の木の上限の扱い。task は run を起こさずに待ち、tick は idle になる）。
async fn tree_limit_holds_repair(limit_key: &str, configure: impl FnOnce(&mut Dispatcher), seed: [u64; 4]) {
    let mut s = Scenario::new(
        "true",
        |t| t.tree = Some(task_core::TreeInfo::root(t.id)),
        0,
        |_, _| panic!("the repair run must not start"),
    );
    s.d.config.execution.limits.tree.enabled = true;
    configure(&mut s.d);
    super::tree_gate::seed_tree_run(&s.store, &s.task, "w-before", "claude-sonnet-5", seed);
    s.review().await;
    s.settle().await;
    let events = s.events();
    assert_eq!(verdicts(&events), vec![HealthVerdict::Fail]);
    assert_eq!(repairs_scheduled(&events), 1, "{events:?}");
    assert_eq!(worker_runs(&events), 0, "the repair run is held: {events:?}");
    let decisions = s.store.decisions_list(Some(s.task.id)).unwrap();
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0].key, limit_key);
    assert!(s.inbox_kinds().contains(&InboxKind::Decision), "{:?}", s.inbox_kinds());
    assert_ne!(s.stored().status, Status::Done);
}

/// D4.1（`max_tree_runs`）: health の修復の WU の run も木の run の上限に数え、上限で起こさない。
#[tokio::test]
async fn health_limits_cap_tree_runs_hold_health_repair() {
    tree_limit_holds_repair(
        "limit:max_tree_runs",
        |d| d.config.execution.limits.tree.max_tree_runs = 1,
        [1_000, 0, 0, 0],
    )
    .await;
}

/// D4.1（`max_tree_usd`）: 木の費用が上限に達していれば、health の修復の WU の run も起こさない。
#[tokio::test]
async fn health_limits_cap_tree_usd_holds_health_repair() {
    tree_limit_holds_repair(
        "limit:max_tree_usd",
        |d| d.config.execution.limits.tree.max_tree_usd_micros = Some(1_000_000),
        [2_000_000, 0, 0, 0],
    )
    .await;
}

/// D4.1（`max_tokens`、`token_budget`）: health の修復の run の usage は task 木の token の消費に入る。修復の run で
/// `max_tokens` に達すると、次の review の reviewer を起こさず既存の扱い（`token_budget_exhausted`）で止まり、修復を
/// 足さない。
#[tokio::test]
async fn health_limits_cap_token_budget_counts_health_repair_runs() {
    let mut s = Scenario::new(
        "true",
        |t| t.budget.max_tokens = Some(1_000),
        5_000,
        next_finding,
    );
    s.review().await;
    let events = s.events();
    assert_eq!(repairs_scheduled(&events), 1, "{events:?}");
    let repair_runs: Vec<task_core::RunRow> = (s.store.runs_for_task(s.task.id).unwrap().into_iter())
        .filter(|r| r.work_unit_id.is_some())
        .collect();
    assert_eq!(repair_runs.len(), 1, "{repair_runs:?}");
    assert_eq!(
        repair_runs[0].usage.as_ref().map(|u| u.budget_tokens()),
        Some(5_000)
    );
    assert_eq!(
        crate::dispatcher::token_budget::tree_tokens_used(s.store.as_ref(), s.task.id, None).unwrap(),
        5_000
    );
    let stored = s.stored();
    assert_eq!(stored.status, Status::Failed, "{stored:?}");
    assert!(
        reasons(&events).contains(&Trigger::TokenBudgetExhausted.name().to_string()),
        "{:?}",
        reasons(&events)
    );
    assert_eq!(verdicts(&events), vec![HealthVerdict::Fail], "no second review");
    assert!(s.inbox_kinds().contains(&InboxKind::Failed), "{:?}", s.inbox_kinds());
}

/// D4.1（`MAX_TARGET_RESYNCS`）: health の修復の後の評価し直しは review 前の同期の数（`ReviewTargetAdvanced`）を
/// 増やさない。`MAX_TARGET_RESYNCS` を超える回数の review を回しても `ReviewResyncHalted` で止まらず、修復の上限
/// （`max_repairs` = 3）で `health_repair_cap` に止まる。
#[tokio::test]
async fn health_limits_cap_reevaluation_does_not_count_target_resyncs() {
    let mut s = Scenario::atomic(next_finding);
    s.d.config.execution.max_repairs_per_class = 3;
    s.review().await;
    let events = assert_stopped(&s, "health_repair_cap: ");
    let reviews = verdicts(&events).len();
    assert_eq!(reviews, 4);
    assert!(reviews as u32 > task_ops::delivery::MAX_TARGET_RESYNCS + 1);
    assert_eq!(
        count(&events, |e| matches!(e, Event::ReviewTargetAdvanced { .. })),
        0
    );
    assert_eq!(
        count(&events, |e| matches!(e, Event::ReviewResyncHalted { .. })),
        0
    );
    assert_eq!(repairs_scheduled(&events), 3);
}

/// CoS の `review_rework`（上限 2 回、task-api の `REVIEW_REWORK_MAX`）は Failed の task だけに当たる。health だけの
/// 不合格は修復の上限の後も Failed にならず（Blocked と質問）、`review_rework` に入らない。health でない失敗と、health
/// の Fail と health でない失敗が混ざる不合格は今どおり 1 回の `review_fail` で Failed（attempts 1）になり、修復も
/// 質問も足さない（上限の数え方は `cos_review_rework.rs` の `health_limits_cap_*`）。
#[tokio::test]
async fn health_limits_cap_review_rework_sees_only_non_health_failures() {
    let mut only = Scenario::atomic(next_finding);
    only.d.config.execution.max_repairs = 1;
    only.review().await;
    assert_stopped(&only, "health_repair_cap: ");
    assert!(!only.inbox_kinds().contains(&InboxKind::Failed));

    for fixed in [true, false] {
        let mut s = Scenario::new("false", |_| {}, 0, |_, _| {});
        if fixed {
            commit_file(&s.dir, "FIXED", "ok\n");
        }
        s.review().await;
        let events = s.events();
        let expected = if fixed { HealthVerdict::Pass } else { HealthVerdict::Fail };
        assert_eq!(verdicts(&events), vec![expected]);
        let stored = s.stored();
        assert_eq!((stored.status, stored.attempts), (Status::Failed, 1), "{stored:?}");
        let fails = reasons(&events)
            .iter()
            .filter(|r| *r == Trigger::ReviewFail.name())
            .count();
        assert_eq!(fails, 1);
        assert_eq!(repairs_scheduled(&events), 0);
        assert!(raised(&events).is_empty(), "{events:?}");
        assert!(s.inbox_kinds().contains(&InboxKind::Failed), "{:?}", s.inbox_kinds());
    }
}

/// D4.2: 検査器の 1 回の実行は `HEALTH_INPUT_TIMEOUT` が上限で、時間切れは Error（D3.4）。repair に回さず
/// `health_check_error:` で止まる。終わらない stub と、作業ツリーの下だけ縮めた上限（2 秒）で作る（600 秒待たない）。
#[tokio::test]
async fn health_limits_cap_checker_timeout_is_error_without_repair() {
    let mut s = Scenario::atomic(|_, _| panic!("no repair on a timeout"));
    test_timeout::cap_under(s.ws.path(), Duration::from_secs(2));
    commit_file(&s.dir, "HANG", "1\n");
    s.review().await;
    let events = assert_stopped(&s, "health_check_error: ");
    let evidence = health_evidence(&events);
    assert_eq!(evidence.len(), 1, "{evidence:?}");
    assert_eq!(evidence[0].0, HealthVerdict::Error);
    assert!(evidence[0].1.contains("時間"), "{evidence:?}");
    assert_eq!(repairs_scheduled(&events), 0);
}

/// D4.4・J4: 修復の上限を使い切った後に人が comment で再開しても、検査器の `error`・`unknown`・`unsupported`
/// （exit 2）は合格にならない。どれも `health_check_error:` で Blocked に止まり、修復の数は増えない。
#[tokio::test]
async fn health_limits_cap_exhausted_error_unknown_unsupported_are_not_pass() {
    let mut s = Scenario::atomic(next_finding);
    s.d.config.execution.max_repairs = 1;
    s.review().await;
    assert_stopped(&s, "health_repair_cap: ");
    for status in ["error", "unknown", "unsupported"] {
        commit_file(&s.dir, "STATUS", &format!("{status}\n"));
        s.comment("検査器を見直した");
        s.settle().await;
        let events = assert_stopped(&s, "health_check_error: ");
        let (verdict, reason) = health_evidence(&events).pop().unwrap();
        assert_eq!(verdict, HealthVerdict::Error, "{status}");
        assert!(reason.contains(&format!("{status}:deps.violation")), "{reason}");
        assert_eq!(repairs_scheduled(&events), 1, "{status}");
        assert_ne!(s.stored().status, Status::Done);
    }
}
