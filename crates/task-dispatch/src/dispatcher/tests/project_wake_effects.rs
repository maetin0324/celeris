//! ADR 2026-11-04-project-controller D12・I7・I13・付記 A1: `pending` の効果の門・再試行・QueueCosInput の E2E。
//!
//! 親の module（`project_wake`）の道具（一時 DB・注入した時計・fake decider）を使う。LLM は起動しない
//! （`max_cos_runs = 0`）。CoS が入力を処理したことは、入力の行を `completed` にして表す。

use super::*;

/// 呼ばれるたびに `pc-input:<project>:<n>` の入力を積む判定。
fn queue_cos_numbered(p: &Project) -> DecideFn {
    let project = p.id.to_string();
    let mut n = 0;
    Box::new(move |_| {
        n += 1;
        Ok(cos_decision(&format!("pc-input:{project}:{n}")))
    })
}

/// 毎回同じ key の入力を積む判定（crash の試験でも使う）。
pub(super) fn queue_cos_input(p: &Project) -> DecideFn {
    let key = format!("pc-input:{}:outcome-1", p.id);
    Box::new(move |_| Ok(cos_decision(&key)))
}

fn cos_decision(key: &str) -> ProjectDecision {
    ProjectDecision {
        effect: ProjectEffect::QueueCosInput {
            key: key.into(),
            text: "案件の root が終わりました。次を決めてください。".into(),
        },
        next_wake_after_secs: None,
        phase: ControllerPhase::Active,
        rule_id: "fake/cos".into(),
    }
}

fn cos_inputs(env: &Env) -> i64 {
    env.count("SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%'")
}

fn cos_inputs_queued(env: &Env) -> i64 {
    env.count(
        "SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%' \
         AND state = 'queued'",
    )
}

/// CoS が入力を処理し終えた（run の終わりで入力の行は `completed` になる）。
fn cos_processed_all(env: &Env) {
    rusqlite::Connection::open(env.db())
        .unwrap()
        .execute(
            "UPDATE chat_messages SET state = 'completed' \
             WHERE client_message_id LIKE 'pc-input:%' AND state = 'queued'",
            [],
        )
        .unwrap();
}

fn decisions(
    store: &SqliteStore,
    p: &Project,
) -> Vec<task_core::project_wake::ProjectControllerDecision> {
    let mut d = store
        .project_controller_decisions(&p.id.to_string(), None, 50)
        .unwrap();
    d.reverse();
    d
}

/// root の done → coalesce を過ぎて評価 → 効果。
fn wake_by_root_done(env: &Env, store: &SqliteStore, p: &Project, d: &mut Dispatcher) {
    let root = root_task(env, store, p, Status::Done);
    transition(store, &root, Status::Done, "review_pass");
    tick(d);
    env.advance(31);
    tick(d);
}

fn exhaustion_notices(env: &Env) -> i64 {
    env.count(
        "SELECT COUNT(*) FROM feed_sources WHERE source_key LIKE 'project_controller:effect_failed:%'",
    )
}

// ---- QueueCosInput ----

#[tokio::test]
async fn project_wake_cos_input_queues() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", queue_cos_numbered(&p));
    prime(&mut d);
    wake_by_root_done(&env, &store, &p, &mut d);
    assert_eq!(env.calls().len(), 1);
    assert_eq!(cos_inputs(&env), 1);
    let (role, thread_project): (String, Option<String>) = rusqlite::Connection::open(env.db())
        .unwrap()
        .query_row(
            "SELECT m.role, t.project_id FROM chat_messages m JOIN chat_threads t ON t.id = m.thread_id \
             WHERE m.client_message_id LIKE 'pc-input:%'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(role, "system");
    assert_eq!(
        thread_project,
        Some(p.id.to_string()),
        "the project's thread"
    );
    let ds = decisions(&store, &p);
    assert_eq!(ds.len(), 1);
    assert_eq!(ds[0].effect_state, EffectState::Applied);
    assert!(ds[0].effect_ref.is_some(), "the message id");
    assert_eq!(env.llm_launches(), 0);
}

#[tokio::test]
async fn project_wake_cos_input_backpressure() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", queue_cos_numbered(&p));
    prime(&mut d);
    wake_by_root_done(&env, &store, &p, &mut d);
    assert_eq!(cos_inputs(&env), 1);
    // 次の wake: 前の入力が未処理なので Busy。判断の効果は pending のまま backoff。
    human_bump(&store, &p.id.to_string(), env.now());
    tick(&mut d);
    assert_eq!(env.calls().len(), 2);
    assert_eq!(
        cos_inputs(&env),
        1,
        "backpressure: one open input per project"
    );
    assert_eq!(d.project_controller_last().unwrap().busy, 1);
    let second = decisions(&store, &p)[1].clone();
    assert_eq!(second.effect_state, EffectState::Pending);
    let retry = store
        .project_controller_effect_retry_state(&second.id)
        .unwrap()
        .unwrap();
    assert_eq!((retry.attempts, retry.failures), (1, 0));
    // backoff の間は試さない（Busy も同じ backoff、retry_base_secs = 60）。
    for _ in 0..3 {
        env.advance(10);
        tick(&mut d);
    }
    assert_eq!(d.project_controller_last().unwrap().busy, 0);
    // CoS が処理した。backoff が過ぎるまでは積まない。
    cos_processed_all(&env);
    tick(&mut d);
    assert_eq!(cos_inputs(&env), 1);
    env.advance(31);
    tick(&mut d);
    assert_eq!(
        cos_inputs(&env),
        2,
        "queued once after the CoS processed the first"
    );
    for _ in 0..5 {
        env.advance(120);
        tick(&mut d);
    }
    assert_eq!(cos_inputs(&env), 2, "exactly once");
    assert_eq!(cos_inputs_queued(&env), 1);
    let ds = decisions(&store, &p);
    assert!(ds.iter().all(|d| d.effect_state == EffectState::Applied));
    assert_eq!(env.calls().len(), 2);
    assert_eq!(env.llm_launches(), 0);
}

#[tokio::test]
async fn project_wake_cos_input_same_key_no_duplicate() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", queue_cos_input(&p));
    prime(&mut d);
    wake_by_root_done(&env, &store, &p, &mut d);
    // 同じ key の入力を返す判断をもう 2 回（入力は未処理のまま）。
    for _ in 0..2 {
        human_bump(&store, &p.id.to_string(), env.now());
        tick(&mut d);
    }
    assert_eq!(env.calls().len(), 3);
    assert_eq!(cos_inputs(&env), 1, "the same key queues one message");
    let ds = decisions(&store, &p);
    assert_eq!(ds.len(), 3);
    assert!(ds.iter().all(|d| d.effect_state == EffectState::Applied));
    let refs: std::collections::BTreeSet<_> =
        ds.iter().map(|d| d.effect_ref.clone().unwrap()).collect();
    assert_eq!(refs.len(), 1, "every decision points at the same message");
    // 再起動して同じ key の判断をもう一度実行しても増えない。
    let store = env.open();
    let mut d = env.daemon(&store, "i2", queue_cos_input(&p));
    human_bump(&store, &p.id.to_string(), env.now());
    tick(&mut d);
    assert_eq!(cos_inputs(&env), 1);
    assert_eq!(env.llm_launches(), 0);
}

#[tokio::test]
async fn project_wake_cos_input_pending_off() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", queue_cos_numbered(&p));
    prime(&mut d);
    wake_by_root_done(&env, &store, &p, &mut d);
    human_bump(&store, &p.id.to_string(), env.now());
    tick(&mut d);
    let pending = decisions(&store, &p)[1].clone();
    assert_eq!(pending.effect_state, EffectState::Pending, "Busy");
    let before = cos_inputs(&env);
    // 人が controller を off にした。その後に CoS が前の入力を処理し、backoff も過ぎた。
    store
        .project_controller_set_mode(
            &p.id.to_string(),
            ControllerMode::Off,
            None,
            "human:test",
            env.now(),
        )
        .unwrap();
    cos_processed_all(&env);
    for _ in 0..5 {
        env.advance(3600);
        tick(&mut d);
    }
    assert_eq!(cos_inputs(&env) - before, 0, "no CoS input after off");
    let after = store
        .project_controller_decision(&pending.id)
        .unwrap()
        .unwrap();
    assert_eq!(after.effect_state, EffectState::Suppressed);
    assert_eq!(after.effect_ref.as_deref(), Some("halted:mode_off"));
    assert_eq!(env.llm_launches(), 0);
}

// ---- pending の効果の門（mode・phase・案件の状態） ----

/// 1 回目の効果を一時的に失敗させて `pending` に残し、`halt` の後で backoff を過ぎても実行されないこと。
fn pending_halt_case(halt: impl FnOnce(&SqliteStore, &Project, OffsetDateTime), reason: &str) {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    d.set_project_controller_effect_fail(Box::new(|_| true));
    prime(&mut d);
    wake_by_root_done(&env, &store, &p, &mut d);
    let pending = decisions(&store, &p)[0].clone();
    assert_eq!(pending.effect_state, EffectState::Pending);
    halt(&store, &p, env.now());
    let mut d = env.daemon(&store, "i2", create_root(&p));
    for _ in 0..5 {
        env.advance(3600);
        tick(&mut d);
    }
    let after = store
        .project_controller_decision(&pending.id)
        .unwrap()
        .unwrap();
    assert_eq!(after.effect_state, EffectState::Suppressed, "{reason}");
    assert_eq!(
        after.effect_ref.as_deref(),
        Some(format!("halted:{reason}").as_str())
    );
    assert_eq!(controller_roots(&store, &p), 0, "{reason}");
    assert_eq!(idempotency_rows(&env, &p), 0, "{reason}");
    assert_eq!(cos_inputs(&env), 0, "{reason}");
    assert_eq!(env.llm_launches(), 0);
}

#[tokio::test]
async fn project_wake_pending_off() {
    pending_halt_case(
        |store, p, now| {
            store
                .project_controller_set_mode(
                    &p.id.to_string(),
                    ControllerMode::Off,
                    None,
                    "human:test",
                    now,
                )
                .unwrap();
        },
        "mode_off",
    );
}

#[tokio::test]
async fn project_wake_pending_paused() {
    pending_halt_case(
        |store, p, _| {
            assert!(
                store
                    .project_set_status(p.id, ProjectStatus::Paused)
                    .unwrap()
            );
        },
        "project_paused",
    );
}

#[tokio::test]
async fn project_wake_pending_cancelled() {
    pending_halt_case(
        |store, p, _| {
            assert!(
                store
                    .project_set_status(p.id, ProjectStatus::Cancelled)
                    .unwrap()
            );
        },
        "project_cancelled",
    );
}

#[tokio::test]
async fn project_wake_pending_shadow() {
    pending_halt_case(
        |store, p, now| {
            store
                .project_controller_set_mode(
                    &p.id.to_string(),
                    ControllerMode::Shadow,
                    None,
                    "human:test",
                    now,
                )
                .unwrap();
        },
        "mode_shadow",
    );
}

// ---- 効果の再試行 ----

#[tokio::test]
async fn project_wake_effect_retry() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    let tries = Arc::new(StdMutex::new(0u32));
    let t = tries.clone();
    d.set_project_controller_effect_fail(Box::new(move |_| {
        let mut n = t.lock().unwrap();
        *n += 1;
        *n <= 2
    }));
    prime(&mut d);
    wake_by_root_done(&env, &store, &p, &mut d);
    let id = decisions(&store, &p)[0].id.clone();
    let state = |store: &SqliteStore| store.project_controller_decision(&id).unwrap().unwrap();
    assert_eq!(state(&store).effect_state, EffectState::Pending);
    assert_eq!(*tries.lock().unwrap(), 1);
    // backoff（60 秒）の間は試さない。
    env.advance(59);
    tick(&mut d);
    assert_eq!(*tries.lock().unwrap(), 1);
    env.advance(2);
    tick(&mut d);
    assert_eq!(*tries.lock().unwrap(), 2, "second try after 60 s");
    let r = store
        .project_controller_effect_retry_state(&id)
        .unwrap()
        .unwrap();
    assert_eq!((r.attempts, r.failures), (2, 2));
    // 2 回目の backoff は 120 秒。
    env.advance(119);
    tick(&mut d);
    assert_eq!(*tries.lock().unwrap(), 2);
    env.advance(2);
    tick(&mut d);
    assert_eq!(*tries.lock().unwrap(), 3);
    assert_eq!(state(&store).effect_state, EffectState::Applied);
    assert_eq!(controller_roots(&store, &p), 1);
    assert_eq!(idempotency_rows(&env, &p), 1);
    assert_eq!(exhaustion_notices(&env), 0);
    let s = store
        .project_controller_state(&p.id.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(s.phase, ControllerPhase::Active);
    assert_eq!(
        env.calls().len(),
        1,
        "the wake was not lost and not re-decided"
    );
    assert_eq!(env.llm_launches(), 0);
}

#[tokio::test]
async fn project_wake_effect_exhausted() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let settings = ProjectControllerSettings {
        max_retries: 2,
        ..Default::default()
    };
    let mut d = env.daemon_with(&store, "i1", create_root(&p), settings);
    d.set_project_controller_effect_fail(Box::new(|_| true));
    prime(&mut d);
    wake_by_root_done(&env, &store, &p, &mut d);
    let id = decisions(&store, &p)[0].id.clone();
    for _ in 0..10 {
        env.advance(3600);
        tick(&mut d);
    }
    let after = store.project_controller_decision(&id).unwrap().unwrap();
    assert_eq!(after.effect_state, EffectState::Failed);
    assert!(
        after
            .effect_ref
            .as_deref()
            .is_some_and(|r| r.contains("injected effect failure")),
        "{:?}",
        after.effect_ref
    );
    let r = store
        .project_controller_effect_retry_state(&id)
        .unwrap()
        .unwrap();
    assert_eq!(r.failures, 3, "max_retries 2 → the 3rd failure exhausts");
    let s = store
        .project_controller_state(&p.id.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(s.phase, ControllerPhase::Blocked);
    assert_eq!(exhaustion_notices(&env), 1);
    let notices = store
        .notice_list(&task_core::feed::NoticeQuery::default())
        .unwrap();
    assert!(
        notices
            .items
            .iter()
            .any(|n| n.kind == task_core::feed::NoticeKind::BadNews
                && n.project_id.as_deref() == Some(p.id.to_string().as_str())),
        "a bad-news notice for the project"
    );
    // blocked の後は判断も効果も止まる（人が mode を置き直すまで）。
    human_bump(&store, &p.id.to_string(), env.now());
    for _ in 0..3 {
        env.advance(3600);
        tick(&mut d);
    }
    assert_eq!(env.calls().len(), 1);
    assert_eq!(exhaustion_notices(&env), 1, "one notice");
    assert_eq!(controller_roots(&store, &p), 0);
    assert_eq!(env.llm_launches(), 0);
}
