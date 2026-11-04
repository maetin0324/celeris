//! 付記 A1 の 12: decide の失敗で止まるときの notice と、案件別 cursor（`event_cursor`）の読み手。

use super::*;
use task_core::project_wake::NewProjectWake;

fn decide_failed_notices(env: &Env, p: &Project) -> i64 {
    rusqlite::Connection::open(env.db())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM feed_sources WHERE source_key LIKE ?1",
            [format!("project_controller:decide_failed:{}:%", p.id)],
            |r| r.get(0),
        )
        .unwrap()
}

fn project_notices(env: &Env, p: &Project) -> i64 {
    rusqlite::Connection::open(env.db())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM feed_notices WHERE project_id = ?1 AND kind = 'bad_news'",
            [p.id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
}

fn state(store: &SqliteStore, p: &Project) -> task_core::project_wake::ProjectControllerState {
    store
        .project_controller_state(&p.id.to_string())
        .unwrap()
        .unwrap()
}

fn failing(max_retries: u32) -> ProjectControllerSettings {
    ProjectControllerSettings {
        retry_base_secs: 1,
        retry_max_secs: 4,
        max_retries,
        ..ProjectControllerSettings::default()
    }
}

/// decide が毎回失敗する daemon で、root の失敗の wake を 1 件入れる。
fn failing_case(env: &Env, max_retries: u32) -> (Arc<SqliteStore>, Project, Dispatcher) {
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon_with(
        &store,
        "i1",
        Box::new(|_| Err("decider is down".into())),
        failing(max_retries),
    );
    prime(&mut d);
    let t = root_task(env, &store, &p, Status::Failed);
    transition(&store, &t, Status::Failed, "worker_error");
    (store, p, d)
}

fn run_ticks(env: &Env, d: &mut Dispatcher, n: usize) {
    for _ in 0..n {
        tick(d);
        env.advance(10);
    }
}

#[tokio::test]
async fn project_wake_decide_exhausted_notice() {
    let env = Env::new();
    let (store, p, mut d) = failing_case(&env, 2);
    run_ticks(&env, &mut d, 20);
    let s = state(&store, &p);
    assert_eq!(s.phase, ControllerPhase::Blocked);
    assert_eq!(s.retry_count, 3);
    assert_eq!(env.calls().len(), 3, "decide stops once blocked");
    assert_eq!(decide_failed_notices(&env, &p), 1);
    assert_eq!(project_notices(&env, &p), 1);
    let summary: String = rusqlite::Connection::open(env.db())
        .unwrap()
        .query_row(
            "SELECT summary FROM feed_notices WHERE project_id = ?1",
            [p.id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(summary.contains(&p.id.to_string()), "{summary}");
    assert!(summary.contains("decider is down"), "{summary}");
    // 次の tick・再起動の後も増えない。
    run_ticks(&env, &mut d, 10);
    drop(d);
    let store = env.open();
    let mut d = env.daemon_with(
        &store,
        "i2",
        Box::new(|_| Err("decider is down".into())),
        failing(2),
    );
    run_ticks(&env, &mut d, 10);
    assert_eq!(decide_failed_notices(&env, &p), 1);
    assert_eq!(env.calls().len(), 3);
    // 人が mode を置き直す（revision + 1）と再開し、また止まれば新しい notice（source_key が別）。
    let rev = state(&store, &p).revision;
    store
        .project_controller_set_mode(
            &p.id.to_string(),
            ControllerMode::Enabled,
            Some(rev),
            "human:test",
            env.now(),
        )
        .unwrap();
    run_ticks(&env, &mut d, 20);
    assert_eq!(state(&store, &p).phase, ControllerPhase::Blocked);
    assert_eq!(env.calls().len(), 6);
    assert_eq!(decide_failed_notices(&env, &p), 2);
    assert_eq!(env.llm_launches(), 0);
}

#[tokio::test]
async fn project_wake_decide_retry_no_notice() {
    let env = Env::new();
    let (store, p, mut d) = failing_case(&env, 5);
    // 失敗 1〜3 回目（上限 5 以内）: backoff だけで notice は無い。
    for _ in 0..200 {
        if env.calls().len() >= 3 {
            break;
        }
        tick(&mut d);
        env.advance(1);
    }
    assert_eq!(env.calls().len(), 3);
    let s = state(&store, &p);
    assert_eq!(s.phase, ControllerPhase::Active);
    assert_eq!(s.retry_count, 3);
    assert_eq!(decide_failed_notices(&env, &p), 0);
    assert_eq!(project_notices(&env, &p), 0);
}

/// 案件の root を `to` にする event を足し、その event id を返す。
fn root_event(env: &Env, store: &SqliteStore, p: &Project, to: Status) -> u64 {
    let t = root_task(env, store, p, to);
    let reason = if to == Status::Done {
        "review_pass"
    } else {
        "worker_error"
    };
    transition(store, &t, to, reason);
    store.latest_event_id().unwrap()
}

fn settle(env: &Env, d: &mut Dispatcher) {
    tick(d);
    env.advance(31);
    tick(d);
    tick(d);
}

fn redeliver_from(store: &SqliteStore, before: i64) {
    store
        .feed_cursor_set(
            crate::dispatcher::cron_tick::project_wake::PROJECT_WAKE_CURSOR,
            &before.to_string(),
        )
        .unwrap();
}

#[tokio::test]
async fn project_wake_cursor_per_project() {
    let env = Env::new();
    let store = env.open();
    let a = project(&store, Some(ControllerMode::Enabled));
    let b = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", none());
    prime(&mut d);
    let ea = root_event(&env, &store, &a, Status::Failed);
    let eb = root_event(&env, &store, &b, Status::Failed);
    settle(&env, &mut d);
    assert_eq!(state(&store, &a).event_cursor, ea as i64);
    assert_eq!(state(&store, &b).event_cursor, eb as i64);
    // A だけに新しい event: A の cursor だけが進み、B は動かない。
    let ea2 = root_event(&env, &store, &a, Status::Done);
    settle(&env, &mut d);
    assert_eq!(state(&store, &a).event_cursor, ea2 as i64);
    assert_eq!(state(&store, &b).event_cursor, eb as i64);
    assert_eq!(env.calls().len(), 3);
    // 人の wake（source_event_id なし）は cursor を動かさない。
    human_bump(&store, &b.id.to_string(), env.now());
    settle(&env, &mut d);
    assert_eq!(env.calls().len(), 4);
    assert_eq!(state(&store, &b).event_cursor, eb as i64);
    assert_eq!(state(&store, &a).event_cursor, ea2 as i64);
}

#[tokio::test]
async fn project_wake_cursor_monotonic() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", none());
    prime(&mut d);
    let before = store.latest_event_id().unwrap() as i64;
    root_event(&env, &store, &p, Status::Failed);
    let last = root_event(&env, &store, &p, Status::Done) as i64;
    settle(&env, &mut d);
    assert_eq!(state(&store, &p).event_cursor, last);
    let mut seen = vec![state(&store, &p).event_cursor];
    for round in 0..5 {
        // 全体の cursor を戻した再配送と、store を開き直した daemon の再起動を交互に。
        redeliver_from(&store, before);
        settle(&env, &mut d);
        seen.push(state(&store, &p).event_cursor);
        if round % 2 == 0 {
            drop(d);
            let store2 = env.open();
            d = env.daemon(&store2, &format!("r{round}"), none());
            settle(&env, &mut d);
            seen.push(state(&store2, &p).event_cursor);
        }
    }
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert!(seen.iter().all(|c| *c == last), "{seen:?}");
    assert_eq!(env.calls().len(), 1, "{:?}", env.calls());
}

#[tokio::test]
async fn project_wake_cursor_blocks_redelivery() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    prime(&mut d);
    let before = store.latest_event_id().unwrap() as i64;
    let ev = root_event(&env, &store, &p, Status::Done) as i64;
    settle(&env, &mut d);
    assert_eq!(env.calls().len(), 1, "{:?}", env.calls());
    assert_eq!(state(&store, &p).event_cursor, ev);
    // wake の行が消えていても（INSERT OR IGNORE の重複検出に頼れない）、cursor 以下の再配送は wake にしない。
    rusqlite::Connection::open(env.db())
        .unwrap()
        .execute(
            "DELETE FROM project_wakes WHERE project_id = ?1",
            [p.id.to_string()],
        )
        .unwrap();
    for _ in 0..10 {
        redeliver_from(&store, before);
        settle(&env, &mut d);
    }
    assert_eq!(store.project_wakes_all(&p.id.to_string()).unwrap().len(), 0);
    assert_eq!(env.calls().len(), 1);
    assert_eq!(controller_roots(&store, &p), 1);
    assert_eq!(env.llm_launches(), 0);
}

#[tokio::test]
async fn project_wake_cursor_keeps_new_events() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let pid = p.id.to_string();
    let mut d = env.daemon(&store, "i1", none());
    prime(&mut d);
    // (1) 再配送と新しい event が同じ取り込みに混ざっても、新しい event の wake は入る。
    let before = store.latest_event_id().unwrap() as i64;
    let old = root_event(&env, &store, &p, Status::Failed) as i64;
    settle(&env, &mut d);
    assert_eq!(state(&store, &p).event_cursor, old);
    let new = root_event(&env, &store, &p, Status::Done) as i64;
    redeliver_from(&store, before);
    settle(&env, &mut d);
    let calls = env.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    let srcs: Vec<Option<i64>> = calls[1].wakes.iter().map(|w| w.source_event_id).collect();
    assert_eq!(srcs, vec![Some(new)]);
    assert_eq!(state(&store, &p).event_cursor, new);
    // (2) 1 回の評価で畳み切れず残った wake（WAKES_PER_EVALUATION を超えた分）は、cursor を越えた
    // 再配送の後も落ちずに次の評価に入る。
    let n = task_core::store::WAKES_PER_EVALUATION + 1;
    let base = new + 1000;
    let wakes: Vec<NewProjectWake> = (1..=n as i64)
        .map(|i| NewProjectWake {
            project_id: pid.clone(),
            wake_key: format!("root_failed:synthetic:{}", base + i),
            kind: ProjectWakeKind::RootFailed,
            source_event_id: Some(base + i),
            source_ref: None,
        })
        .collect();
    assert_eq!(
        store
            .project_wake_ingest(&wakes, None, 0, env.now())
            .unwrap(),
        n
    );
    tick(&mut d);
    assert_eq!(env.calls().len(), 3);
    assert_eq!(env.calls()[2].wakes.len(), n - 1);
    assert_eq!(state(&store, &p).event_cursor, base + n as i64 - 1);
    assert_eq!(pending_wakes(&store, &p), 1);
    // 全部を再配送: cursor 以下は入らず、残りの 1 件は既にある。
    assert_eq!(
        store
            .project_wake_ingest(&wakes, None, 0, env.now())
            .unwrap(),
        0
    );
    tick(&mut d);
    let calls = env.calls();
    assert_eq!(calls.len(), 4);
    let last: Vec<Option<i64>> = calls[3].wakes.iter().map(|w| w.source_event_id).collect();
    assert_eq!(last, vec![Some(base + n as i64)]);
    assert_eq!(state(&store, &p).event_cursor, base + n as i64);
    assert_eq!(pending_wakes(&store, &p), 0);
}
