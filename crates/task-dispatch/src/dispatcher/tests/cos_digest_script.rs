//! ADR 2026-11-02-ts-control-layer-boundary 付記 S4: digest pass の頑健性（unit robustness）。
//!
//! 偽の `PolicyScript` と注入した時計・`tokio::time::pause`・出来事待ち（worker の join、compare task の
//! join）だけで決定的に試す。CPU を焼く負荷は使わない。どの場面でも次を確かめる（[`assert_invariants`]）。
//! - inbox thread の digest message は冪等 key ごとに 1 件
//! - stopped/interrupted の run の requeue は 1 回（`running`→`pending` の遷移を試験用の trigger で数える）
//! - `script_compare_log` の記録は run ごとに高々 1 件
//! - Rust の結果（message・item）は TS の有無（mode `rust`・PolicyScript 無し と mode `compare`）で変わらない
//!
//! 障害は試験 DB に置く SQLite の trigger（書き込み）と表の改名（読み出し）で 1 回だけ起こし、外してから
//! 次の tick で回収されることを見る。

use super::*;
use crate::script_policy::{PolicyFuture, PolicyScript, PolicyScriptBundle, PolicyScriptError};
use std::sync::atomic::{AtomicUsize, Ordering};
use task_core::chat::{ChatMessageRole, ChatRunState};
use task_core::script_compare::{
    CompareOutcome, CosDigestCompareRecord, LaneRulesInput, NewScriptGeneration,
    RoutePriorityInput, ScriptGenerationState, ScriptGenerationTransition, ScriptMode,
    ScriptModeChange,
};

fn sha(byte: &str) -> String {
    byte.repeat(32)
}

/// 偽の TS の答え方。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Answer {
    /// Rust と同じ command を返す。
    Echo,
    /// 返らない（dispatcher の deadline で打ち切られる）。
    Hang,
}

struct FakeDigest {
    sha: String,
    answer: Answer,
    calls: AtomicUsize,
}

impl FakeDigest {
    fn new(sha: String, answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            sha,
            answer,
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl PolicyScript for FakeDigest {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: self.sha.clone(),
            host_api: "1.0.0".into(),
        }
    }

    fn lane_rules(&self, _input: LaneRulesInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn route_priority(&self, _input: RoutePriorityInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn cos_digest_pass(
        &self,
        call: crate::script_policy::CosDigestPassCall,
        _deadline: Duration,
    ) -> PolicyFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self.answer;
        Box::pin(async move {
            match answer {
                Answer::Echo => crate::script_policy::test_pass::reference_reply(&call).await,
                Answer::Hang => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Err(PolicyScriptError::new("exception", "woke up"))
                }
            }
        })
    }
}

/// 場面ごとの TS の有無。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ts {
    /// mode `rust`、PolicyScript 無し（基準）。
    Without,
    /// mode `compare`、`answer` を返す PolicyScript。
    Compare(Answer),
}

/// Rust の結果（id・時刻を除いた直列化）と実行 log。
#[derive(Debug)]
struct World {
    /// inbox thread の assistant message（key・text・cards・run_id）。
    messages: String,
    /// `cos_inbox_items` の (source_key, state, run_id, reason)。
    items: String,
    /// requeue の実行 log: requeue された item の (source_key, 元の run_id)。
    requeues: Vec<(String, String)>,
    records: Vec<CosDigestCompareRecord>,
}

const DIGEST_KEY: &str = task_core::chat::triage::COS_TRIAGE_DIGEST_KEY_PREFIX;

impl Fixture {
    /// 試験 DB に requeue の実行 log を置く（`running`→`pending` の遷移ごとに 1 行）。
    fn install_requeue_log(&self) {
        self.sql(
            "CREATE TABLE IF NOT EXISTS test_requeue_log(source_key TEXT NOT NULL, run_id TEXT NOT NULL); \
             CREATE TRIGGER IF NOT EXISTS test_requeue_log_t AFTER UPDATE OF state ON cos_inbox_items \
             WHEN OLD.state='running' AND NEW.state='pending' BEGIN \
             INSERT INTO test_requeue_log VALUES(OLD.source_key, OLD.run_id); END;",
        );
    }

    fn sql(&self, sql: &str) {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        conn.execute_batch(sql).expect("sql");
    }

    /// 次の assistant message の書き込みを失敗させる。
    fn fail_message_write(&self, on: bool) {
        self.sql(if on {
            "CREATE TRIGGER test_fail_digest BEFORE INSERT ON chat_messages \
             WHEN NEW.role='assistant' BEGIN SELECT RAISE(ABORT,'injected digest write failure'); END;"
        } else {
            "DROP TRIGGER test_fail_digest;"
        });
    }

    /// requeue（`running`→`pending`）の書き込みを失敗させる。
    fn fail_requeue_write(&self, on: bool) {
        self.sql(if on {
            "CREATE TRIGGER test_fail_requeue BEFORE UPDATE OF state ON cos_inbox_items \
             WHEN OLD.state='running' AND NEW.state='pending' BEGIN \
             SELECT RAISE(ABORT,'injected requeue failure'); END;"
        } else {
            "DROP TRIGGER test_fail_requeue;"
        });
    }

    /// digest の読み出し（走査と snapshot）を失敗させる: `cos_inbox_items` を一時的に改名する。
    fn fail_reads(&self, on: bool) {
        self.sql(if on {
            "ALTER TABLE cos_inbox_items RENAME TO test_cos_inbox_items_away;"
        } else {
            "ALTER TABLE test_cos_inbox_items_away RENAME TO cos_inbox_items;"
        });
    }

    /// `id` の run に 1 件の item を渡す（item は `running`）。
    fn claim_run(&self, id: &str) -> String {
        let now = self.d.now_utc();
        self.store
            .cos_triage_ingest_batch(
                "digest-robustness-test",
                id,
                &[task_core::chat::triage::CosTriageSource {
                    source_kind: "notice".into(),
                    source_key: id.into(),
                    source_revision: "1".into(),
                    source_event_id: None,
                    operation_id: None,
                    summary: format!("知らせ {id}"),
                    detail: String::new(),
                    policy_version: "1".into(),
                    task_id: None,
                }],
                now,
            )
            .expect("ingest");
        let general = self
            .store
            .cos_thread_open(
                &task_core::chat::ChatOpenThreadRequest { project_id: None },
                now,
            )
            .expect("general")
            .thread
            .id;
        let claim = self
            .store
            .cos_triage_claim(&general, id, now)
            .expect("claim")
            .expect("run");
        claim.item_ids[0].clone()
    }

    /// CoS が item を判断した（observed）。
    fn resolve_item(&self, item: &str) {
        assert!(
            self.store
                .cos_triage_resolve(item, "observed", None, Some("確認済み"), self.d.now_utc())
                .expect("resolve")
        );
    }

    fn finish_run(&self, id: &str, state: ChatRunState) {
        self.store
            .chat_run_finish(id, state, None, Some("人が止めた"), self.d.now_utc())
            .expect("finish");
    }

    /// dispatcher が run の終わりを観測した（worker の handle が終わった・回収した）。
    fn observe_run_end(&mut self) {
        self.d
            .cos_chat_launch
            .as_mut()
            .expect("launch")
            .triage
            .digest_due = true;
    }

    /// 新しい tick（mode を読み直す）の digest pass。
    fn next_tick(&mut self) {
        self.d.ticks += 1;
        self.tick();
    }

    /// 同じ DB で dispatcher を作り直す（daemon の再起動）。
    fn restart_digest(&mut self, ts: Ts) -> Option<Arc<FakeDigest>> {
        self.d = triage_dispatcher(
            self.dir.path(),
            &self.db_path,
            self.store.clone(),
            false,
            "true",
        );
        attach(self, ts)
    }

    fn robust_world(&self) -> World {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let mut stmt = conn
            .prepare(
                "SELECT source_key, state, run_id, reason FROM cos_inbox_items ORDER BY source_key",
            )
            .expect("prepare");
        let items: Vec<serde_json::Value> = stmt
            .query_map([], |r| {
                Ok(serde_json::json!([
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ]))
            })
            .expect("items")
            .map(|r| r.expect("row"))
            .collect();
        let mut stmt = conn
            .prepare("SELECT source_key, run_id FROM test_requeue_log ORDER BY rowid")
            .expect("requeue log");
        let requeues = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("requeues")
            .map(|r| r.expect("row"))
            .collect();
        let messages: Vec<serde_json::Value> = match self.inbox_thread() {
            None => Vec::new(),
            Some(inbox) => self
                .store
                .chat_message_list(
                    &inbox,
                    &ChatMessageQuery {
                        limit: Some(200),
                        ..Default::default()
                    },
                )
                .expect("messages")
                .items
                .into_iter()
                .filter(|m| m.role == ChatMessageRole::Assistant)
                .map(|m| {
                    serde_json::json!({
                        "key": m.client_message_id,
                        "text": m.text,
                        "cards": m.cards,
                        "run_id": m.run_id,
                    })
                })
                .collect(),
        };
        World {
            messages: serde_json::to_string(&messages).expect("json"),
            items: serde_json::to_string(&items).expect("json"),
            requeues,
            records: self
                .store
                .script_compare_log(None)
                .expect("log")
                .into_iter()
                .map(|row| row.record)
                .collect(),
        }
    }

    /// 冪等 key ごとの digest message の数（全 thread）。
    fn digest_counts(&self) -> Vec<(String, i64)> {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let mut stmt = conn
            .prepare(
                "SELECT client_message_id, COUNT(*) FROM chat_messages \
                 WHERE client_message_id LIKE ?1 GROUP BY thread_id, client_message_id \
                 ORDER BY client_message_id",
            )
            .expect("prepare");
        stmt.query_map([format!("{DIGEST_KEY}%")], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("counts")
            .map(|r| r.expect("row"))
            .collect()
    }
}

/// mode `compare` の generation を置き（`Ts::Compare` のとき）、script を注入する。
fn attach(f: &mut Fixture, ts: Ts) -> Option<Arc<FakeDigest>> {
    f.d.set_script_domain_store(f.store.clone());
    match ts {
        Ts::Without => {
            f.d.set_policy_script(None);
            None
        }
        Ts::Compare(answer) => {
            let script = FakeDigest::new(sha("cd"), answer);
            f.d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
            Some(script)
        }
    }
}

/// generation `bundle` を記録し、mode を `to` にする。
fn set_mode(f: &Fixture, to: ScriptMode, bundle: &str) -> String {
    let now = f.d.now_utc();
    // 付記 S5-4: `rust` に戻した直後の generation は `draining` で、mode の行の行き先にできない。
    // `retired` にしてから登録し直し、`loaded` に戻す。
    if let Some(old) = f
        .store
        .script_generation_by_sha256(&sha(bundle))
        .expect("read")
        && old.state == ScriptGenerationState::Draining
    {
        f.store
            .script_generation_transition(
                &ScriptGenerationTransition {
                    id: old.id,
                    to: ScriptGenerationState::Retired,
                    actor: "human:test".into(),
                    reason: "drained".into(),
                    cancelled_in_flight: None,
                },
                now,
            )
            .expect("retire");
    }
    let generation = f
        .store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: sha(bundle),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: r#"{"domains":["cos_digest"]}"#.into(),
                state: ScriptGenerationState::Loaded,
            },
            now,
        )
        .expect("generation");
    let version = f
        .store
        .script_domain_modes()
        .expect("modes")
        .into_iter()
        .find(|r| r.domain.as_str() == "cos_digest")
        .expect("cos_digest row")
        .version;
    f.store
        .script_domain_set_mode(
            &ScriptModeChange {
                domain: "cos_digest".into(),
                to_mode: to,
                to_generation: (to == ScriptMode::Compare).then(|| generation.id.clone()),
                expected_version: version,
                actor: "human:test".into(),
                reason: "cos_digest_script robustness".into(),
            },
            now,
        )
        .expect("mode");
    generation.id
}

/// 場面の用意: 試験 DB・requeue の log・mode と script。
fn setup(ts: Ts) -> (Fixture, Option<Arc<FakeDigest>>) {
    let mut f = fixture(false);
    f.install_requeue_log();
    if matches!(ts, Ts::Compare(_)) {
        set_mode(&f, ScriptMode::Compare, "cd");
    }
    let script = attach(&mut f, ts);
    (f, script)
}

/// 受け入れ条件 1: 二重の message・requeue・compare 記録が 0。
fn assert_invariants(f: &Fixture, world: &World) {
    for (key, n) in f.digest_counts() {
        assert_eq!(n, 1, "digest message {key} written {n} times");
    }
    let mut requeued = world.requeues.clone();
    requeued.sort();
    requeued.dedup();
    assert_eq!(
        requeued.len(),
        world.requeues.len(),
        "an item of a run was requeued twice: {:?}",
        world.requeues
    );
    let mut runs: Vec<&str> = world.records.iter().map(|r| r.run_id.as_str()).collect();
    runs.sort_unstable();
    let before = runs.len();
    runs.dedup();
    assert_eq!(
        runs.len(),
        before,
        "two compare records for one run: {:?}",
        world.records
    );
}

/// 同じ場面を TS 無し・TS 有り（compare, echo）で走らせ、Rust の結果が一致することを確かめる。
async fn same_with_and_without_ts<F, Fut>(scenario: F) -> (World, World)
where
    F: Fn(Ts) -> Fut,
    Fut: std::future::Future<Output = World>,
{
    let base = scenario(Ts::Without).await;
    let with = scenario(Ts::Compare(Answer::Echo)).await;
    assert_eq!(with.messages, base.messages, "messages are Rust's");
    assert_eq!(with.items, base.items, "item states are Rust's");
    assert_eq!(with.requeues, base.requeues, "requeues are Rust's");
    assert!(base.records.is_empty(), "mode rust records nothing");
    (base, with)
}

/// tick して compare の task を待ち、世界を読んで不変条件を確かめる。
async fn settle(f: &mut Fixture) -> World {
    f.tick();
    f.d.wait_script_compares().await;
    let world = f.robust_world();
    assert_invariants(f, &world);
    world
}

fn assert_all_match(world: &World, runs: usize) {
    assert_eq!(world.records.len(), runs, "{:?}", world.records);
    for r in &world.records {
        assert_eq!(r.outcome, CompareOutcome::Match, "{r:?}");
    }
}

// ---------------------------------------------------------------- (1) 順序

/// item の更新（CoS の判断）→ run の終了 → digest、と run の終了 → digest → item の遅れた更新、の 2 順序。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_order_item_update_before_and_after_run_end() {
    async fn scenario(ts: Ts) -> World {
        let (mut f, _script) = setup(ts);
        // 先: item を判断してから run が終わる。
        let early = f.claim_run("run-1-item-first");
        f.resolve_item(&early);
        f.finish_run("run-1-item-first", ChatRunState::Completed);
        // 後: run が終わってから（digest の後で）item が更新される。completed と stopped の両方。
        let late_done = f.claim_run("run-2-run-first");
        f.finish_run("run-2-run-first", ChatRunState::Completed);
        let late_stop = f.claim_run("run-3-run-first-stop");
        f.finish_run("run-3-run-first-stop", ChatRunState::Stopped);
        f.observe_run_end();
        let first = settle(&mut f).await;
        // 遅れて item の更新が届く。completed の run の未処理 item は digest と同じ pass で退避
        // （fallback）に渡っているので、遅れた判断は効かない。digest pass がもう一度立っても何も増えない。
        for item in [&late_done, &late_stop] {
            assert!(
                !f.store
                    .cos_triage_resolve(item, "observed", None, Some("遅れた判断"), f.d.now_utc())
                    .expect("resolve"),
                "the item already went to the person"
            );
        }
        f.observe_run_end();
        let world = settle(&mut f).await;
        assert_eq!(world.messages, first.messages, "no second digest");
        assert_eq!(world.records.len(), first.records.len());
        assert!(
            first.messages.contains("確認済み"),
            "item-first: the digest shows the judgment: {}",
            first.messages
        );
        assert_eq!(f.digest_counts().len(), 3);
        assert_eq!(
            world.requeues,
            vec![(
                "run-3-run-first-stop".to_owned(),
                "run-3-run-first-stop".to_owned()
            )],
            "only the stopped run is requeued, once"
        );
        world
    }
    let (_, with) = same_with_and_without_ts(scenario).await;
    assert_all_match(&with, 3);
}

// ---------------------------------------------------------------- (2) 再配送

/// 同じ run を 2 回の tick と digest_due の再設定で処理しても、message・requeue・記録は 1 回。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_redelivery_same_run_twice() {
    async fn scenario(ts: Ts) -> World {
        let (mut f, script) = setup(ts);
        f.claim_run("run-a-done");
        f.finish_run("run-a-done", ChatRunState::Completed);
        f.claim_run("run-b-intr");
        f.finish_run("run-b-intr", ChatRunState::Interrupted);
        f.observe_run_end();
        let first = settle(&mut f).await;
        for _ in 0..2 {
            f.observe_run_end();
            f.next_tick();
            f.d.wait_script_compares().await;
        }
        // 同じ tick の中でも 2 回目の pass を立てる。
        f.observe_run_end();
        f.tick();
        f.observe_run_end();
        let world = settle(&mut f).await;
        assert_eq!(world.messages, first.messages);
        assert_eq!(world.items, first.items);
        assert_eq!(world.requeues.len(), 1, "{:?}", world.requeues);
        if let Some(script) = script {
            assert_eq!(script.calls(), 1, "one TS call per pass");
        }
        world
    }
    let (_, with) = same_with_and_without_ts(scenario).await;
    assert_all_match(&with, 2);
}

// ---------------------------------------------------------------- (3) retry

/// 1 回目の message の書き込みが失敗する → 次の tick で回収（requeue も compare も 1 回）。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_retry_after_failed_message_write() {
    async fn scenario(ts: Ts) -> World {
        let (mut f, _script) = setup(ts);
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.fail_message_write(true);
        f.observe_run_end();
        let failed = settle(&mut f).await;
        assert!(f.digest_counts().is_empty(), "the write failed");
        assert!(failed.requeues.is_empty(), "requeue follows the message");
        assert!(failed.records.is_empty(), "no record of a failed apply");
        f.fail_message_write(false);
        // digest_due は保たれている: run の終わりを観測し直さなくても次の tick で回収する。
        // 書き込みに失敗した tick では requeue されていないので、同じ pass の退避（fallback）が
        // stopped の run の item を人へ渡している（item は running のまま残らない）。
        assert!(
            failed.items.contains(r#""run-stop","fallback","run-stop""#),
            "{}",
            failed.items
        );
        let world = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 1);
        assert!(
            world.messages.contains("CoS 不在の退避"),
            "{}",
            world.messages
        );
        assert!(
            world.requeues.is_empty(),
            "the item is already with the person"
        );
        settle(&mut f).await;
        world
    }
    let (_, with) = same_with_and_without_ts(scenario).await;
    assert_all_match(&with, 1);
}

/// 1 回目の読み出し（走査）が失敗する → 次の tick で回収。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_retry_after_failed_read() {
    async fn scenario(ts: Ts) -> World {
        let (mut f, script) = setup(ts);
        f.claim_run("run-intr");
        f.finish_run("run-intr", ChatRunState::Interrupted);
        f.observe_run_end();
        f.fail_reads(true);
        f.tick();
        f.d.wait_script_compares().await;
        f.fail_reads(false);
        assert!(f.digest_counts().is_empty(), "the read failed");
        if let Some(script) = &script {
            assert_eq!(script.calls(), 0);
        }
        let world = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 1);
        assert_eq!(world.requeues.len(), 1);
        settle(&mut f).await;
        world
    }
    let (_, with) = same_with_and_without_ts(scenario).await;
    assert_all_match(&with, 1);
}

/// requeue の書き込みが失敗する → その run は message も含めて何も残らない（ADR 2026-11-03-cos-digest-persist:
/// message・requeue・記録は同じ transaction）。item は同じ pass の退避（fallback）で人へ渡り、書き込みが直った次の
/// pass で message が 1 件だけ書かれる（requeue は item が running でないので 0 件）。記録は run ごとに 1 件。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_retry_after_failed_requeue_write() {
    async fn scenario(ts: Ts) -> World {
        let (mut f, _script) = setup(ts);
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.fail_requeue_write(true);
        f.observe_run_end();
        let failed = settle(&mut f).await;
        assert!(f.digest_counts().is_empty(), "the run was rolled back");
        assert!(failed.requeues.is_empty());
        assert!(failed.records.is_empty(), "{:?}", failed.records);
        f.fail_requeue_write(false);
        assert!(
            failed.items.contains(r#""run-stop","fallback","run-stop""#),
            "{}",
            failed.items
        );
        f.observe_run_end();
        let world = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 1, "written on the next pass");
        assert!(world.requeues.is_empty(), "{:?}", world.requeues);
        assert_eq!(world.items, failed.items, "not stuck in running");
        settle(&mut f).await;
        world
    }
    let (_, with) = same_with_and_without_ts(scenario).await;
    assert_eq!(with.records.len(), 1, "{:?}", with.records);
}

// ---------------------------------------------------------------- (4) cancel

/// TS の評価が deadline 前に終わらない → 打ち切られて ts_error。Rust の結果は変わらない。
/// stopped と interrupted の run の requeue は 1 回ずつ。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_ts_cancelled_at_deadline() {
    async fn scenario(ts: Ts) -> World {
        let ts = match ts {
            Ts::Compare(_) => Ts::Compare(Answer::Hang),
            other => other,
        };
        let (mut f, script) = setup(ts);
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.claim_run("run-intr");
        f.finish_run("run-intr", ChatRunState::Interrupted);
        f.observe_run_end();
        let world = settle(&mut f).await;
        assert_eq!(world.requeues.len(), 2, "{:?}", world.requeues);
        if let Some(script) = script {
            assert_eq!(script.calls(), 1, "one TS call per pass");
            assert_eq!(world.records.len(), 2);
            for r in &world.records {
                assert_eq!(r.outcome, CompareOutcome::TsError, "{r:?}");
                assert_eq!(r.error.as_ref().map(|e| e.kind.as_str()), Some("timeout"));
            }
        }
        f.observe_run_end();
        let again = settle(&mut f).await;
        assert_eq!(again.records.len(), world.records.len());
        world
    }
    same_with_and_without_ts(scenario).await;
}

/// dispatcher が TS の評価の途中で落ちる → 作り直した dispatcher は同じ run を二度 digest しない。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_dispatcher_dropped_mid_compare() {
    async fn scenario(ts: Ts) -> World {
        let hang = match ts {
            Ts::Compare(_) => Ts::Compare(Answer::Hang),
            other => other,
        };
        let (mut f, _script) = setup(hang);
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.observe_run_end();
        f.tick();
        // 落ちる: compare の task を abort して dispatcher を捨てる（runtime ごと消えた daemon と同じ）。
        f.d.abort_script_compares().await;
        let script = f.restart_digest(ts);
        let world = settle(&mut f).await;
        if let Some(script) = script {
            assert_eq!(script.calls(), 0, "the digested run is not compared again");
        }
        // ADR 2026-11-03-cos-digest-persist 3: the restarted dispatcher closes the shadow's outbox
        // row with the `unavailable` record (one record, not counted by script-guard).
        if let Ts::Compare(_) = ts {
            assert_eq!(world.records.len(), 1, "{:?}", world.records);
            assert_eq!(world.records[0].outcome, CompareOutcome::Unavailable);
            assert_eq!(world.records[0].applied.requeued, 1);
            assert!(
                f.store
                    .cos_digest_shadow_pending()
                    .expect("outbox")
                    .is_empty()
            );
        }
        assert_eq!(world.requeues.len(), 1);
        world
    }
    same_with_and_without_ts(scenario).await;
}

// ---------------------------------------------------------------- (5) 再起動

/// 同じ DB で dispatcher を作り直す: digest 前に落ちた run は再起動後に 1 回だけ digest され、
/// digest 済みの run は再起動後も増えない。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_restart_same_db() {
    async fn scenario(ts: Ts) -> World {
        let (mut f, _script) = setup(ts);
        f.claim_run("run-1-before-crash");
        f.finish_run("run-1-before-crash", ChatRunState::Interrupted);
        // digest pass の前に落ちる。
        f.restart_digest(ts);
        let first = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 1, "reconcile after start digests");
        // digest 済みのまま再起動: 何も増えない。
        f.claim_run("run-2-after");
        f.finish_run("run-2-after", ChatRunState::Completed);
        f.restart_digest(ts);
        let second = settle(&mut f).await;
        assert_eq!(f.digest_counts().len(), 2);
        f.restart_digest(ts);
        let world = settle(&mut f).await;
        assert_eq!(world.messages, second.messages);
        assert_eq!(world.records.len(), second.records.len());
        assert_eq!(world.requeues, first.requeues);
        assert_eq!(world.requeues.len(), 1);
        world
    }
    let (_, with) = same_with_and_without_ts(scenario).await;
    assert_all_match(&with, 2);
}

/// requeue の書き込みが失敗したまま落ちる → 何も残らず、再起動後の pass で message が 1 件、requeue は 0 回
/// （item は退避のまま）。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_restart_after_failed_requeue() {
    async fn scenario(ts: Ts) -> World {
        let (mut f, _script) = setup(ts);
        f.claim_run("run-stop");
        f.finish_run("run-stop", ChatRunState::Stopped);
        f.fail_requeue_write(true);
        f.observe_run_end();
        settle(&mut f).await;
        assert!(f.digest_counts().is_empty(), "the run was rolled back");
        f.fail_requeue_write(false);
        f.restart_digest(ts);
        let world = settle(&mut f).await;
        assert!(world.requeues.is_empty(), "{:?}", world.requeues);
        assert!(
            world.items.contains(r#""run-stop","fallback","run-stop""#),
            "{}",
            world.items
        );
        assert_eq!(f.digest_counts().len(), 1);
        world
    }
    same_with_and_without_ts(scenario).await;
}

// ---------------------------------------------------------------- (7) compare の途中の mode・generation

/// compare の評価の途中で mode を rust に戻し、generation を替えて compare に戻す。in-flight の記録は元の
/// generation のまま 1 件、rust の間の run は TS に渡らず、新しい generation の run は 1 件。
#[tokio::test(start_paused = true)]
async fn cos_digest_script_mode_back_to_rust_and_generation_change_mid_compare() {
    async fn scenario(ts: Ts) -> World {
        let compare = matches!(ts, Ts::Compare(_));
        let (mut f, script) = setup(ts);
        let first_generation = f
            .store
            .script_domain_modes()
            .expect("modes")
            .into_iter()
            .find(|r| r.domain.as_str() == "cos_digest")
            .and_then(|r| r.generation_id);
        f.claim_run("run-1");
        f.finish_run("run-1", ChatRunState::Stopped);
        f.observe_run_end();
        f.tick(); // compare は in-flight（echo の future は spawn 済みでまだ poll されていない）
        if compare {
            set_mode(&f, ScriptMode::Rust, "cd");
        }
        f.d.wait_script_compares().await;
        // mode rust の間の run は TS に渡らない。
        f.claim_run("run-2");
        f.finish_run("run-2", ChatRunState::Stopped);
        f.observe_run_end();
        f.next_tick();
        f.d.wait_script_compares().await;
        let calls_while_rust = script.as_ref().map(|s| s.calls());
        // generation を替えて compare に戻す（同じ bundle の新しい generation）。
        let second_generation = compare.then(|| set_mode(&f, ScriptMode::Compare, "cd"));
        f.claim_run("run-3");
        f.finish_run("run-3", ChatRunState::Completed);
        f.observe_run_end();
        f.next_tick();
        let world = settle(&mut f).await;
        if let Some(script) = script {
            assert_eq!(calls_while_rust, Some(1), "run-2 was not compared");
            assert_eq!(script.calls(), 2);
            let mut records = world.records.clone();
            records.sort_by(|a, b| a.run_id.cmp(&b.run_id));
            assert_eq!(records.len(), 2, "{records:?}");
            assert_eq!(records[0].run_id, "run-1");
            assert_eq!(Some(&records[0].generation), first_generation.as_ref());
            assert_eq!(records[1].run_id, "run-3");
            assert_eq!(Some(&records[1].generation), second_generation.as_ref());
        }
        assert_eq!(world.requeues.len(), 2);
        // 再配送しても何も増えない。
        f.observe_run_end();
        f.next_tick();
        let again = settle(&mut f).await;
        assert_eq!(again.records.len(), world.records.len());
        world
    }
    let (_, with) = same_with_and_without_ts(scenario).await;
    assert!(
        with.records
            .iter()
            .all(|r| r.outcome == CompareOutcome::Match)
    );
}

// ---------------------------------------------------------------- (6) policy_version と session の freshness

/// 本物の digest 経路（FakeAdapter の CoS run → worker の join → digest）で、`[cos.triage] policy_version`
/// を上げた後の run が fresh（policy_changed）で始まり、その run の digest も 1 件、記録も 1 件であること。
/// 時計は注入した試験の時計、run の終わりは worker の join で待つ（sleep しない）。
#[tokio::test]
async fn cos_digest_script_policy_version_change_and_fresh_session() {
    async fn scenario(ts: Ts) -> (World, Vec<Option<String>>) {
        let mut f = fixture(true);
        f.install_requeue_log();
        if matches!(ts, Ts::Compare(_)) {
            set_mode(&f, ScriptMode::Compare, "cd");
        }
        attach(&mut f, ts);
        f.tick();
        f.question("before-policy", vec![]);
        f.tick();
        assert!(f.join_inbox().await, "first triage run");
        f.tick();
        f.d.wait_script_compares().await;
        let mut cfg = f.d.cos_chat_launch.as_ref().expect("launch").config.clone();
        cfg.triage.policy_version = "7".into();
        f.d.reload_cos_chat_launch(cfg);
        f.question("after-policy", vec![]);
        f.tick();
        assert!(f.join_inbox().await, "second triage run");
        f.tick();
        f.d.wait_script_compares().await;
        // 再配送。
        f.observe_run_end();
        f.tick();
        f.d.wait_script_compares().await;
        f.join_inbox().await;
        f.tick();
        f.d.wait_script_compares().await;
        let world = f.robust_world();
        assert_invariants(&f, &world);
        let conn = rusqlite::Connection::open(&f.db_path).expect("db");
        let mut stmt = conn
            .prepare(
                "SELECT r.run_id FROM chat_runs r JOIN chat_messages m ON m.id=r.input_message_id \
                 WHERE m.role='system' AND m.text LIKE '%item_ids=%' ORDER BY r.started_at, r.run_id",
            )
            .expect("runs");
        let runs: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .expect("runs")
            .map(|r| r.expect("row"))
            .collect();
        assert_eq!(runs.len(), 2, "two triage runs: {runs:?}");
        let reasons: Vec<Option<String>> = runs
            .iter()
            .map(|r| f.store.chat_run_session_record(r).expect("record").reason)
            .collect();
        assert_eq!(
            f.store.chat_run_policy_version(&runs[1]).expect("policy"),
            Some("7".into())
        );
        let digests = f.digest_counts();
        assert_eq!(digests.len(), 2, "one digest per triage run: {digests:?}");
        for run in &runs {
            assert!(
                digests.iter().any(|(k, _)| k.ends_with(run.as_str())),
                "{run} digested: {digests:?}"
            );
        }
        (world, reasons)
    }
    let (base, base_reasons) = scenario(Ts::Without).await;
    let (with, reasons) = scenario(Ts::Compare(Answer::Echo)).await;
    assert_eq!(base_reasons, reasons);
    assert_eq!(
        reasons[1].as_deref(),
        Some("policy_changed"),
        "the run after the version change is fresh: {reasons:?}"
    );
    assert_eq!(base.requeues, with.requeues);
    assert!(base.records.is_empty());
    assert_all_match(&with, 2);
}

#[path = "cos_digest_pass.rs"]
mod cos_digest_pass;

#[path = "cos_digest_script_ts.rs"]
mod ts;

#[path = "cos_digest_script_canary.rs"]
mod canary;

#[path = "cos_digest_behavior_change.rs"]
mod cos_digest_behavior_change;
