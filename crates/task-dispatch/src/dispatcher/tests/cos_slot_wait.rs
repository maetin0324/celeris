//! ADR 2026-11-06-cos-run-launch-accounting-and-drain D2: 枠だけで route を選べない CoS run は失敗にせず、
//! claim のまま（`launched_at` なし）Waiting で待ち、次 tick で同じ入力・同じ run・同じ route 順で選び直す。
//! 枠以外の理由（cooldown 等）では待たない。期限は run の `max_wall_secs` 残りで、延ばさない。
//! 時計は `test_now` の差し替え、待ちは FIFO と task の join（sleep・負荷なし）。

use super::cos_chat_launch::{fixture, join, post, runs, thread};
use super::*;
use task_core::chat::{
    ChatEventData, ChatEventQuery, ChatPrelaunchReason, ChatRun, ChatRunState, ChatStatusPhase,
};
use task_worker::fake::FakeAdapter;

const SLOT_TEXT: &str = "CoS の枠の空き待ち";

/// 起動したら FIFO に書かれるまで終わらない harness。CoS run は自分の workspace の `release.fifo`、
/// 葉の worker（workspace に fifo が無い）は絶対 path の `leaf_fifo` を読む。
fn blocking_command(leaf_fifo: &std::path::Path) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!(
            "cat >/dev/null; f=release.fifo; [ -p \"$f\" ] || f={}; read x < \"$f\"; echo '{{\"type\":\"done\",\"summary\":\"done\",\"evidence\":[]}}'",
            leaf_fifo.display()
        ),
    ]
}

fn workspace(dir: &std::path::Path, thread: &str) -> std::path::PathBuf {
    dir.join("cos/threads").join(thread).join("workspace")
}

fn make_fifo(dir: &std::path::Path, thread: &str) {
    let ws = workspace(dir, thread);
    std::fs::create_dir_all(&ws).expect("workspace");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(ws.join("release.fifo"))
            .status()
            .expect("mkfifo")
            .success()
    );
}

async fn release(dir: &std::path::Path, thread: &str) {
    let path = workspace(dir, thread).join("release.fifo");
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || std::fs::write(path, b"go\n").expect("release")),
    )
    .await
    .expect("release deadline")
    .expect("release task");
}

fn advance(d: &Dispatcher, secs: i64) {
    let clock = d.test_now.as_ref().expect("test clock");
    let mut now = clock.lock().unwrap();
    *now += time::Duration::seconds(secs);
}

fn run_of(store: &SqliteStore, thread: &str) -> ChatRun {
    let mut all = runs(store, thread);
    assert_eq!(all.len(), 1, "exactly one run for the input");
    all.pop().unwrap()
}

fn waiting_notices(store: &SqliteStore, thread: &str, run: &str) -> usize {
    store
        .chat_events_page(
            thread,
            &ChatEventQuery {
                after: None,
                run_id: Some(run.into()),
                limit: Some(500),
            },
        )
        .expect("events")
        .items
        .iter()
        .filter(|e| {
            matches!(&e.data, ChatEventData::Status(s)
                if s.phase == ChatStatusPhase::Waiting && s.summary.contains(SLOT_TEXT))
        })
        .count()
}

struct Holder {
    task: TaskId,
    fifo: std::path::PathBuf,
    _workspace: tempfile::TempDir,
    _fifo_dir: tempfile::TempDir,
}

/// 葉の worker（CoS ではない run）が `p1` の唯一の枠を握った状態で、B の CoS run を claim させる。
/// （CoS 同士の予約は launch tick の間 dispatcher から見えない既存の制約があるため、握る側は葉にする。）
async fn leaf_holds_the_slot_b_waits() -> (
    crate::test_support::WritableTempDir,
    Arc<SqliteStore>,
    Dispatcher,
    Holder,
    String,
) {
    let holder_dir = tempfile::tempdir().unwrap();
    let fifo = holder_dir.path().join("leaf.fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let (dir, store, mut d) = fixture(blocking_command(&fifo), 2);
    d.set_account_limit("p1", "default", Some(1)).unwrap();
    let ws = tempfile::tempdir().unwrap();
    let leaf = new_task(
        ws.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&leaf).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1, "the leaf holds the slot");
    assert_eq!(d.implicit_account_in_use("p1"), 1);
    let b = thread(&store, "slot-b");
    make_fifo(dir.path(), &b);
    post(&store, &b, "b1");
    d.tick_cos_chat_launch();
    (
        dir,
        store,
        d,
        Holder {
            task: leaf.id,
            fifo,
            _workspace: ws,
            _fifo_dir: holder_dir,
        },
        b,
    )
}

async fn release_holder(d: &mut Dispatcher, holder: &Holder) {
    let path = holder.fifo.clone();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || std::fs::write(path, b"go\n").expect("release")),
    )
    .await
    .expect("release deadline")
    .expect("release task");
    await_worker_completion(d, holder.task).await;
    d.tick().unwrap();
}

/// 受け入れ 0 と 3: 枠だけなら Waiting のまま保持し、次 tick 以降は新しい run も失敗 run も作らない。
/// 枠が空けば同じ run が起動して Running → 完了。待機中の run は使用数に数えない。
#[tokio::test]
async fn slot_only_wait_keeps_the_run_waiting_then_launches_the_same_run() {
    let (dir, store, mut d, holder, b) = leaf_holds_the_slot_b_waits().await;
    let waiting = run_of(&store, &b);
    assert_eq!(waiting.state, ChatRunState::Running, "claim stays");
    assert!(waiting.launched_at.is_none(), "nothing was launched");
    assert!(waiting.finished_at.is_none());
    assert_eq!(waiting_notices(&store, &b, &waiting.id), 1);
    assert!(
        d.cos_chat_launch
            .as_ref()
            .unwrap()
            .running
            .get(&b)
            .is_none(),
        "a waiting run has no worker"
    );
    // 受け入れ 3: 起動した run だけを数える（葉の 1 件。待機中の B の CoS run は数えない）。
    assert_eq!(d.cos_in_flight(), 0);
    assert_eq!(d.implicit_account_in_use("p1"), 1);

    for _ in 0..3 {
        advance(&d, 1);
        d.tick_cos_chat_launch();
        let again = run_of(&store, &b);
        assert_eq!(again.id, waiting.id, "same run, no new run");
        assert_eq!(again.state, ChatRunState::Running);
        assert!(again.launched_at.is_none());
        assert_eq!(waiting_notices(&store, &b, &waiting.id), 1, "no event spam");
        assert_eq!(d.cos_in_flight(), 0);
        assert_eq!(d.implicit_account_in_use("p1"), 1);
    }

    release_holder(&mut d, &holder).await;
    d.tick_cos_chat_launch();
    let launched = run_of(&store, &b);
    assert_eq!(launched.id, waiting.id, "the same run launches");
    assert!(d.cos_chat_launch.as_ref().unwrap().running.contains_key(&b));
    assert_eq!(d.cos_in_flight(), 1);
    assert_eq!(d.implicit_account_in_use("p1"), 1);
    // 待機の終わり（Running への遷移）が永続記録に残る。
    let events = store
        .chat_events_page(
            &b,
            &ChatEventQuery {
                after: None,
                run_id: Some(waiting.id.clone()),
                limit: Some(500),
            },
        )
        .unwrap()
        .items;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.data, ChatEventData::Status(s)
            if s.phase == ChatStatusPhase::Working && s.summary.contains("枠が空いた"))),
        "the end of the wait is recorded"
    );
    release(dir.path(), &b).await;
    join(&mut d, &b).await;
    assert_eq!(run_of(&store, &b).state, ChatRunState::Completed);
}

/// 受け入れ 1: cooldown は枠ではない。待たずに失敗（起動前の閉じ）にする。
#[tokio::test]
async fn cooldown_does_not_wait() {
    let (_dir, store, mut d) = fixture(FakeAdapter::default_command(), 2);
    let until = (d.now_unix_fn)() + 3600;
    d.source_cooldowns.insert(
        "p1".into(),
        crate::accounts::AccountCooldown {
            until,
            reason: crate::accounts::AccountCooldownReason::Throttled,
        },
    );
    let t = thread(&store, "cooldown");
    post(&store, &t, "one");
    d.tick_cos_chat_launch();
    let run = run_of(&store, &t);
    assert_eq!(run.state, ChatRunState::Failed);
    assert!(run.launched_at.is_none());
    assert_eq!(waiting_notices(&store, &t, &run.id), 0, "not a slot wait");
    advance(&d, 1);
    d.tick_cos_chat_launch();
    assert_eq!(runs(&store, &t).len(), 1, "no retry run");
}

/// 受け入れ 2: 期限は `started_at + max_wall_secs`。設定を後から延ばしても延びない。到達で
/// `slot_wait_expired`（起動前）で閉じ、その後に起動もしない。
#[tokio::test]
async fn slot_wait_expires_at_the_run_deadline_and_is_not_extended() {
    let (_dir, store, mut d, holder, b) = leaf_holds_the_slot_b_waits().await;
    let waiting = run_of(&store, &b);
    // 設定の max_wall_secs を延ばしても、待機中の run の期限は延びない（run 開始時の設定を使う）。
    d.reload_cos_chat_launch({
        let mut cfg = d.cos_chat_launch.as_ref().unwrap().config.clone();
        cfg.max_wall_secs = 10_000;
        cfg
    });
    advance(&d, 29);
    d.tick_cos_chat_launch();
    assert_eq!(run_of(&store, &b).state, ChatRunState::Running, "29s < 30s");
    advance(&d, 1);
    d.tick_cos_chat_launch();
    let closed = run_of(&store, &b);
    assert_eq!(closed.id, waiting.id);
    assert_eq!(closed.state, ChatRunState::Failed);
    assert!(closed.launched_at.is_none());
    assert_eq!(
        closed.prelaunch_reason,
        Some(ChatPrelaunchReason::SlotWaitExpired)
    );
    // 枠が空いても、閉じた run は起動しない。新しい run も作らない。
    release_holder(&mut d, &holder).await;
    advance(&d, 1);
    d.tick_cos_chat_launch();
    assert_eq!(runs(&store, &b).len(), 1);
    assert!(!d.cos_chat_launch.as_ref().unwrap().running.contains_key(&b));
}

/// 待機中の run を人が止めたら、起動前の stop として閉じる（起動しない）。
#[tokio::test]
async fn stop_closes_a_waiting_run_before_launch() {
    let (_dir, store, mut d, holder, b) = leaf_holds_the_slot_b_waits().await;
    let waiting = run_of(&store, &b);
    store
        .chat_run_stop(&b, &waiting.id, d.now_utc())
        .expect("stop");
    d.tick_cos_chat_launch();
    let closed = run_of(&store, &b);
    assert_eq!(closed.state, ChatRunState::Stopped);
    assert_eq!(
        closed.prelaunch_reason,
        Some(ChatPrelaunchReason::StoppedBeforeLaunch)
    );
    release_holder(&mut d, &holder).await;
    d.tick_cos_chat_launch();
    assert!(!d.cos_chat_launch.as_ref().unwrap().running.contains_key(&b));
}
