//! ADR 2026-10-29-stagnation-triage-notify-wiring: 停滞 4 種（`project_stalled`・`human_wait_aging`・
//! `release_unpromoted`・`task_unscheduled`）が通常の CoS 一次対応（`cos_inbox_items`）に入り、再 tick・再照合で
//! 増えず、解消・改訂で旧行が閉じ、停滞の判定が分からない間は閉じも退避もしない。
//!
//! 判定は dispatcher の本物の `check_stagnation`（store の事実と一時ディレクトリの releases_dir）で、時刻は偽の時計。
//! dispatcher は draining（`set_accepting_new_work(false)`）にして run を起こさない（ready の task は起きず、CoS の
//! run も起きないので、項目は pending のまま観察できる）。外部ネットワークに出ず、sleep しない。

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use crate::dispatcher::cos_chat::triage::CosTriageSettings;

const STAGNATION: [&str; 4] = [
    "project_stalled",
    "human_wait_aging",
    "release_unpromoted",
    "task_unscheduled",
];

struct Fixture {
    dir: crate::test_support::WritableTempDir,
    db_path: PathBuf,
    store: Arc<SqliteStore>,
    d: Dispatcher,
    releases: PathBuf,
    t0: OffsetDateTime,
    question: TaskId,
}

fn write(dir: &std::path::Path, name: &str, body: serde_json::Value) {
    std::fs::create_dir_all(dir).expect("mkdir");
    std::fs::write(dir.join(name), body.to_string()).expect("write");
}

/// 案件 A に ready の task 1 件と質問で止まった task 1 件、verify 済みで昇格されていない release 1 件。
/// 閾値は release 600 秒・ready 900 秒・人の待ち 1800 秒・案件 3600 秒、判定の間隔は 60 秒。
fn fixture() -> Fixture {
    let dir = crate::test_support::WritableTempDir::new();
    let db_path = dir.path().join("celeris.db");
    let store = Arc::new(SqliteStore::open(&db_path).expect("store"));
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(task_worker::fake::FakeAdapter::new(vec![
        "sh".into(),
        "-c".into(),
        "cat >/dev/null".into(),
    ]));
    let mut d = dispatcher_with_test_clock(store.clone(), adapter, 2, true);
    d.config.min_free_disk_mb = 0;
    d.config.knowledge.root = dir.path().join("knowledge");
    d.set_cos_chat_launch(
        store.clone(),
        CosChatLaunchConfig {
            worker_reserve_five_hour: 0.90,
            enabled: true,
            harness: "fake".into(),
            max_turns: 2,
            max_wall_secs: 30,
            unavailable_reason: None,
            data_dir: dir.path().to_path_buf(),
            db_path: db_path.clone(),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: Some("http://celeris.test".into()),
            // 退避は試験が明示的に縮めるまで起こさない。
            triage: CosTriageSettings {
                unavailable_after_secs: 1_000_000,
                authorization_unavailable_after_secs: 1_000_000,
                ..Default::default()
            },
        },
    );
    d.set_accepting_new_work(false);
    d.set_stagnation(
        task_ops::stagnation::StagnationPolicy {
            project_idle_secs: 3600,
            human_wait_secs: 1800,
            release_unpromoted_secs: 600,
            task_unscheduled_secs: 900,
        },
        60,
    );

    crate::test_support::project_cos_thread(&store, "案件A");
    let conn = rusqlite::Connection::open(&db_path).expect("db");
    let project: task_core::ProjectId = conn
        .query_row(
            "SELECT project_id FROM chat_threads WHERE kind='project'",
            [],
            |r| r.get::<_, String>(0),
        )
        .expect("project")
        .parse()
        .expect("project id");
    let mut ready = new_task(dir.path(), Check::Human, 0);
    ready.title = "起きない task".into();
    ready.project_id = Some(project);
    ready.status = Status::Ready;
    store.create_task(&ready, vec![]).expect("ready task");
    let mut blocked = new_task(dir.path(), Check::Human, 0);
    blocked.title = "答えを待つ task".into();
    blocked.project_id = Some(project);
    blocked.status = Status::Blocked;
    store
        .create_task(
            &blocked,
            vec![Event::QuestionRaised {
                run_id: "r0".into(),
                text: "どちらにする?".into(),
            }],
        )
        .expect("question task");

    // 事実の時刻（events.ts・task の作成）は壁時計なので、偽の時計はその直後から始める。
    let t0 = OffsetDateTime::now_utc() + time::Duration::seconds(1);
    let at = t0.format(&Rfc3339).expect("ts");
    let releases = dir.path().join("releases");
    write(
        &releases.join("ca6f52cb0000"),
        "manifest.json",
        serde_json::json!({"built_at": at}),
    );
    write(
        &releases.join("ca6f52cb0000"),
        "verify.json",
        serde_json::json!({"ok": true, "at": at}),
    );
    d.config.releases_dir = Some(releases.clone());
    d.test_now = Some(Arc::new(StdMutex::new(t0)));
    Fixture {
        dir,
        db_path,
        store,
        d,
        releases,
        t0,
        question: blocked.id,
    }
}

impl Fixture {
    fn at(&mut self, secs: i64) {
        if let Some(clock) = &self.d.test_now {
            *clock.lock().expect("clock") = self.t0 + time::Duration::seconds(secs);
        }
        self.d.tick().expect("tick");
    }

    /// `(source_kind, source_key, source_revision, state)` of the stagnation rows.
    fn rows(&self) -> Vec<(String, String, String, String)> {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let mut stmt = conn
            .prepare(
                "SELECT source_kind,source_key,source_revision,state FROM cos_inbox_items \
                 WHERE source_kind IN ('project_stalled','human_wait_aging','release_unpromoted','task_unscheduled') \
                 ORDER BY source_kind,created_at,source_revision",
            )
            .expect("prepare");
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    fn open_kinds(&self) -> Vec<String> {
        self.rows()
            .into_iter()
            .filter(|r| r.3 == "pending")
            .map(|r| r.0)
            .collect()
    }

    fn count(&self, sql: &str) -> i64 {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.query_row(sql, [], |r| r.get(0)).expect("count")
    }

    /// The derived inbox with the dispatcher's stagnation snapshot (what `GET /inbox/items` shows).
    fn inbox_triples(&self) -> Vec<(String, String, String)> {
        let ctx = task_ops::view::ViewContext {
            workspace_roots: self.d.config.workspace_roots.clone(),
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            max_requeues: 5,
            clusters: HashMap::new(),
        };
        let snapshot = self.d.stagnation_snapshot().expect("stagnation known");
        let now = *self
            .d
            .test_now
            .as_ref()
            .expect("clock")
            .lock()
            .expect("clock");
        let mut out: Vec<_> = task_ops::human_inbox::human_inbox(
            self.store.as_ref(),
            Some(&snapshot),
            &ctx,
            now,
            &|_, _| Vec::new(),
        )
        .expect("inbox")
        .items
        .into_iter()
        .filter(|i| STAGNATION.contains(&i.kind.as_str()))
        .map(|i| (i.kind.as_str().to_string(), i.id, i.created_at))
        .collect();
        out.sort();
        out
    }

    /// Ticks until all four kinds are open (+3700 s), checking each threshold on the way.
    fn reach_all_four(&mut self) {
        self.at(0);
        assert!(self.rows().is_empty(), "nothing before any threshold");
        assert_eq!(
            self.count("SELECT COUNT(*) FROM cos_inbox_items WHERE source_kind='question'"),
            1,
            "the question itself is triaged as before"
        );
        self.at(700);
        assert_eq!(self.open_kinds(), vec!["release_unpromoted"]);
        self.at(1000);
        assert_eq!(
            self.open_kinds(),
            vec!["release_unpromoted", "task_unscheduled"]
        );
        self.at(1900);
        assert_eq!(
            self.open_kinds(),
            vec!["human_wait_aging", "release_unpromoted", "task_unscheduled"]
        );
        self.at(3700);
        assert_eq!(
            self.open_kinds(),
            vec![
                "human_wait_aging",
                "project_stalled",
                "release_unpromoted",
                "task_unscheduled"
            ]
        );
    }
}

/// 受け入れ (1)(2): 4 種それぞれが閾値で CoS の取り込みに 1 行ずつ入り、triple は受信箱の項目
/// （kind・id・created_at）と一致する。再 tick と再照合（`request_cos_triage_reconcile`）で増えない。
#[tokio::test]
async fn stagnation_kinds_enter_cos_triage_at_their_thresholds_once() {
    let mut f = fixture();
    f.reach_all_four();
    let rows = f.rows();
    let triples: Vec<_> = rows
        .iter()
        .map(|r| (r.0.clone(), r.1.clone(), r.2.clone()))
        .collect();
    let mut sorted = triples.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        f.inbox_triples(),
        "same triple as the derived inbox"
    );

    for secs in [3760, 3820, 4500] {
        f.at(secs);
        assert_eq!(f.rows(), rows, "re-tick at +{secs}s adds nothing");
    }
    f.d.request_cos_triage_reconcile();
    f.at(4560);
    assert_eq!(f.rows(), rows, "the reconcile pass adds nothing");
    assert_eq!(
        f.count("SELECT COUNT(*) FROM chat_runs"),
        0,
        "draining: no CoS run"
    );
}

/// 受け入れ (3): 昇格・質問への回答（解消）で旧行は resolved。案件の最後の出来事が変われば（改訂）旧行は
/// resolved で、新しい revision の行が 1 つだけ pending になる。変わっていない `task_unscheduled` はそのまま。
#[tokio::test]
async fn stagnation_resolution_and_revision_close_the_old_rows() {
    let mut f = fixture();
    f.reach_all_four();
    let before = f.rows();
    let revision_of = |rows: &[(String, String, String, String)], kind: &str| {
        rows.iter()
            .find(|r| r.0 == kind)
            .map(|r| r.2.clone())
            .expect("row")
    };
    let project_rev = revision_of(&before, "project_stalled");
    let unscheduled = before
        .iter()
        .find(|r| r.0 == "task_unscheduled")
        .cloned()
        .expect("unscheduled");

    write(
        &f.releases.join("ca6f52cb0000"),
        "promoted.json",
        serde_json::json!({"promoted_at": f.t0.format(&Rfc3339).expect("ts")}),
    );
    f.store
        .apply_transition_with_events(
            f.question,
            Trigger::Answer,
            vec![Event::Answered {
                by: None,
                question: "どちらにする?".into(),
                answer: "A".into(),
            }],
        )
        .expect("answer");
    f.at(3760);

    let after = f.rows();
    let state = |kind: &str, rev: &str| {
        after
            .iter()
            .find(|r| r.0 == kind && r.2 == rev)
            .map(|r| r.3.clone())
    };
    assert_eq!(
        state(
            "release_unpromoted",
            &revision_of(&before, "release_unpromoted")
        )
        .as_deref(),
        Some("resolved")
    );
    assert_eq!(
        state(
            "human_wait_aging",
            &revision_of(&before, "human_wait_aging")
        )
        .as_deref(),
        Some("resolved")
    );
    assert_eq!(
        state("project_stalled", &project_rev).as_deref(),
        Some("resolved")
    );
    let project_rows: Vec<_> = after.iter().filter(|r| r.0 == "project_stalled").collect();
    assert_eq!(project_rows.len(), 2, "old revision + new revision");
    let new_rev = project_rows
        .iter()
        .find(|r| r.2 != project_rev)
        .expect("new revision");
    assert_eq!(new_rev.3, "pending");
    assert!(after.contains(&unscheduled), "untouched kind unchanged");
    // The answered task is ready again and its last event is already past the threshold on the
    // fake clock, so it is a second, separate `task_unscheduled` key.
    assert_eq!(
        f.open_kinds(),
        vec!["project_stalled", "task_unscheduled", "task_unscheduled"],
        "only the current revisions are open"
    );
    assert!(
        f.rows()
            .iter()
            .any(|r| r.1 == format!("task_unscheduled-{}", f.question) && r.3 == "pending")
    );
    // The current inbox and the open rows agree.
    let open: Vec<_> = after
        .iter()
        .filter(|r| r.3 == "pending")
        .map(|r| (r.0.clone(), r.1.clone(), r.2.clone()))
        .collect();
    let mut open_sorted = open.clone();
    open_sorted.sort();
    assert_eq!(open_sorted, f.inbox_triples());
}

/// 受け入れ (4): 停滞の判定が失敗している（releases_dir が読めない）間は snapshot が無く、再照合でも
/// 停滞の行を resolved にせず、CoS 不在の退避も停滞の行だけは保留する（他の種類は今までどおり退避する）。
/// 判定が戻れば退避が進み、再 tick で通知の行は増えない。
#[tokio::test]
async fn unknown_stagnation_holds_rows_until_the_check_succeeds_again() {
    let mut f = fixture();
    f.reach_all_four();
    let rows = f.rows();

    let broken = f.dir.path().join("releases-file");
    std::fs::write(&broken, "not a directory").expect("file");
    f.d.config.releases_dir = Some(broken);
    f.at(3760);
    assert!(f.d.stagnation_snapshot().is_none(), "the check failed");
    f.d.request_cos_triage_reconcile();
    f.at(3770);
    assert_eq!(f.rows(), rows, "held: no row resolved while unknown");

    // The unavailable fallback is due for every item now.
    if let Some(launch) = f.d.cos_chat_launch.as_mut() {
        launch.config.triage.unavailable_after_secs = 60;
    }
    f.at(3780);
    assert_eq!(f.rows(), rows, "fallback holds the stagnation rows");
    assert_eq!(
        f.count(
            "SELECT COUNT(*) FROM cos_inbox_items WHERE source_kind='question' AND state='fallback'"
        ),
        1,
        "other kinds still go to the person"
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'"),
        1
    );

    // The check works again (next scan at +60 s): the four rows go to the person, once each.
    f.d.config.releases_dir = Some(f.releases.clone());
    f.at(3830);
    assert!(f.d.stagnation_snapshot().is_some());
    let states: Vec<_> = f.rows().into_iter().map(|r| (r.0, r.3)).collect();
    assert_eq!(
        states,
        [
            "human_wait_aging",
            "project_stalled",
            "release_unpromoted",
            "task_unscheduled"
        ]
        .iter()
        .map(|k| (k.to_string(), "fallback".to_string()))
        .collect::<Vec<_>>()
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'"),
        5
    );
    for secs in [3840, 3900, 4000] {
        f.at(secs);
    }
    assert_eq!(
        f.count("SELECT COUNT(*) FROM notifications WHERE kind='cos_fallback'"),
        5,
        "re-ticks never notify twice"
    );
}
