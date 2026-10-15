//! MLSys 最小実装の mock E2E（実 LLM・実キー・外部ネットワークなし）。
//!
//! 1. キーなし → 該当 unit だけ待機 + 受信箱 item → ダミーキー登録 → mock 認証 → 同じ unit が一度だけ再開。
//! 2. decision-assisted の分類（注入）→ fixed 方策（天井・利用可否が勝つ）→ 実行 → 判断・実行・成果ログ → campaign export。
//! 3. 分類器の故障（timeout・breaker・鍵なし）では fixed の lane に戻り、`real_api_verified` は false のまま。
//!
//! 分類は board への注入（Jev 呼び出し側の fake server 試験は `jev-classifier`・`celeris` 側）。ここで得た結果は
//! 配線の確認であり、実 Jev・実 LLM を使った実タスク性能ではない。

use super::credential_e2e::{DUMMY, E2e, Scripted};
use super::credential_gate::jev_pick;
use super::*;
use crate::credential_resume::VerifyOutcome;
use crate::dispatcher::{BoardItem, ClassificationBoard, CredentialGate};
use task_core::decision_log::RunEndLabel;
use task_core::decision_log_export::ExportParams;
use task_core::route_select::{
    ClassificationEntry, ClassificationHint, ClassifierStatus, RoutingMode, SpecificityHint,
};

const CAMPAIGN: &str = "mock-e2e-campaign";

fn jev_gate() -> CredentialGate {
    CredentialGate {
        secret_id: super::credential_gate::SECRET.into(),
        provider: "jev".into(),
        purpose: "Jev で依頼を分類する".into(),
    }
}

fn jev_ok(s: SpecificityHint) -> BoardItem {
    BoardItem {
        entry: ClassificationEntry {
            status: ClassifierStatus::JevOk,
            hint: Some(ClassificationHint {
                specificity: s,
                confidence: Some(0.9),
            }),
            model_snapshot: Some("mock-jev-snapshot".into()),
        },
        credential: None,
    }
}

fn rank(t: task_core::Tier) -> u8 {
    match t {
        task_core::Tier::Cheap => 0,
        task_core::Tier::Standard => 1,
        task_core::Tier::Frontier => 2,
    }
}

fn hinted_task(dir: &std::path::Path) -> Task {
    let mut task = new_task(
        dir,
        Check::Command {
            cmd: "cargo test".into(),
            expect_exit: 0,
        },
        2,
    );
    task.objective = "crates/task-core/src/model.rs の typo を直す".into();
    task.genre = Some("coding".into());
    task.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Hint,
        ..Default::default()
    });
    task
}

fn question_adapter() -> Arc<dyn WorkerAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Question {
            text: "not needed".into(),
        },
        delay: Duration::ZERO,
    })
}

fn routing_decided(store: &Arc<dyn TaskStore>, task: &Task) -> task_core::RoutingRecord {
    store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::RoutingDecided { record, .. } => Some(*record),
            _ => None,
        })
        .expect("routing_decided")
}

fn lane_assist(rec: &task_core::RoutingRecord) -> task_core::route_select::LaneAssistRecord {
    rec.features
        .as_ref()
        .and_then(|f| f.decision.as_ref())
        .and_then(|d| d.lane_assist.clone())
        .expect("lane_assist")
}

/// task を dispatch して run の終わりまで進め、(routing 記録, run_id) を返す。
async fn dispatch_to_end(
    store: &Arc<dyn TaskStore>,
    mode: RoutingMode,
    item: Option<BoardItem>,
    dir: &std::path::Path,
) -> (Task, task_core::RoutingRecord, String) {
    let task = hinted_task(dir);
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_routing_mode(mode);
    if let Some(item) = item {
        d.classification_board().put(&task.id.to_string(), item);
    }
    d.tick().unwrap();
    let run_id = store
        .events_for(task.id)
        .unwrap()
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerStarted { run_id, .. } => Some(run_id.clone()),
            _ => None,
        })
        .expect("worker_started");
    for _ in 0..400 {
        if store
            .decision_log_run_get(&run_id)
            .unwrap()
            .is_some_and(|r| r.outcome.run_end.is_some())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        d.tick().unwrap();
    }
    let rec = routing_decided(store, &task);
    (task, rec, run_id)
}

#[tokio::test]
async fn no_key_wait_register_resume_once_then_classify_run_log_export() {
    let e = E2e::new();
    let mut d = e.dispatcher();
    d.set_routing_mode(RoutingMode::DecisionAssisted);
    let task = hinted_task(e.dir.path());
    e.sqlite.insert(&task).unwrap();
    let a = super::credential_gate::unit(&task, "a", 0);
    let b = super::credential_gate::unit(&task, "b", 1);
    e.sqlite
        .work_units_apply(task.id, vec![a.clone(), b.clone()], Vec::new(), Vec::new())
        .unwrap();

    // 1. キーなし: 分類段が鍵なしを board に載せる → その unit だけ待機（兄弟は進む）。
    let board: ClassificationBoard = d.classification_board();
    board.put(
        &a.id,
        BoardItem {
            entry: ClassificationEntry::fell_back("key_missing"),
            credential: Some(jev_gate()),
        },
    );
    let mut decision =
        task_core::model_policy::decide_for_task(&task, &Default::default()).expect("lane");
    let (_, held) = d.apply_lane_assist(&task, Some(&a), &mut decision).unwrap();
    assert!(held);
    let mut other = decision.clone();
    assert!(!d.apply_lane_assist(&task, Some(&b), &mut other).unwrap().1);
    assert_eq!(
        e.unit(&task, "a"),
        (
            WorkUnitStatus::Blocked,
            Some(WorkUnitBlockedReason::WaitingForAuth)
        )
    );
    assert_eq!(e.unit(&task, "b"), (WorkUnitStatus::Ready, None));
    let items = e.inbox();
    assert_eq!(items.len(), 1, "受信箱 item は 1 件");
    assert!(items[0].links.iter().any(|l| l.href == "/accounts"));
    assert!(items[0].detail.clone().unwrap().contains("自動で続行"));

    // 2. ダミーキー登録 → mock 認証 → 同じ unit が一度だけ再開。
    e.put_secret(DUMMY);
    let v = Scripted::new(&[VerifyOutcome::Valid]);
    let r = e.resumer(&v);
    let rep = r.process().unwrap();
    assert_eq!((rep.verified_calls, rep.resumed), (1, 1));
    for _ in 0..3 {
        assert_eq!(r.process().unwrap().resumed, 0);
    }
    assert_eq!(e.unit(&task, "a"), (WorkUnitStatus::Ready, None));
    assert_eq!(e.resumed_events(&task), 1, "再開は一度だけ");
    assert!(e.inbox().is_empty());

    // 3. 再開後の同じ unit: 分類（注入）が通れば待たずに進み、fixed 方策の lane を下げない。
    board.put(&a.id, jev_ok(SpecificityHint::NeedsInvestigationDesign));
    let fixed_lane = decision.lane;
    let mut assisted = decision.clone();
    let (rec, held) = d.apply_lane_assist(&task, Some(&a), &mut assisted).unwrap();
    assert!(!held);
    assert!(rank(assisted.lane) >= rank(fixed_lane));
    let rec = rec.expect("lane_assist record");
    assert_eq!(rec.classifier_status, ClassifierStatus::JevOk);
    assert!(!rec.real_api_verified, "実 API は未検証のまま");

    // 4. 分類 → 方策 → 実行 → ログ → export（task を実際に dispatch）。
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let (t2, rec2, run_id) = dispatch_to_end(
        &store,
        RoutingMode::DecisionAssisted,
        Some(jev_ok(SpecificityHint::NeedsInvestigationDesign)),
        ws.path(),
    )
    .await;
    let assist = lane_assist(&rec2);
    assert_eq!(assist.classifier_status, ClassifierStatus::JevOk);
    assert_eq!(
        assist.final_lane,
        format!("{:?}", rec2.decision.lane).to_lowercase()
    );
    let run = store.decision_log_run_get(&run_id).unwrap().unwrap();
    assert_eq!(run.outcome.run_end, Some(RunEndLabel::Question));
    assert_eq!(
        run.proposal.task_id.as_deref(),
        Some(t2.id.to_string().as_str())
    );
    assert!(run.proposal.decided_lane.is_some());
    assert!(run.execution.exec_model.is_some());

    // campaign への所属は campaign runner の仕事。ここでは書き手の代わりに campaign_id / arm を付ける。
    let mut p = run.proposal.clone();
    p.campaign_id = Some(CAMPAIGN.into());
    p.arm = Some("decision_assisted".into());
    sqlite
        .decision_log_record_proposal(&p, time::OffsetDateTime::now_utc())
        .unwrap();
    let params = ExportParams {
        campaign_id: CAMPAIGN.into(),
        manifest_hash: None,
        export_cursor: None,
    };
    let export = sqlite.decision_log_export_campaign(&params).unwrap();
    let mut kinds: Vec<_> = export
        .records
        .iter()
        .map(|r| r["record_type"].as_str().unwrap().to_string())
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        ["decision", "outcome", "run"],
        "判断・実行・成果が 1 run 分"
    );
    let jsonl = export.records_jsonl();
    assert!(jsonl.contains(&run_id));
    // 再 export は同一（決定的）で、秘密の値は出ない。
    let again = sqlite.decision_log_export_campaign(&params).unwrap();
    assert_eq!(jsonl, again.records_jsonl());
    assert!(!jsonl.contains(DUMMY));
    assert!(
        !serde_json::to_string(&export.summary)
            .unwrap()
            .contains(DUMMY)
    );
}

#[tokio::test]
async fn classifier_failures_fall_back_to_the_fixed_lane_and_still_run_and_log() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let ws = tempfile::tempdir().unwrap();
    // 比較の基準: fixed。
    let (_, fixed, _) = dispatch_to_end(&store, RoutingMode::Fixed, None, ws.path()).await;
    assert!(
        fixed
            .features
            .as_ref()
            .and_then(|f| f.decision.as_ref())
            .and_then(|d| d.lane_assist.as_ref())
            .is_none()
    );
    for reason in [
        "timeout",
        "breaker_open",
        "rate_limited",
        "invalid_response",
    ] {
        let (_, rec, run_id) = dispatch_to_end(
            &store,
            RoutingMode::DecisionAssisted,
            Some(BoardItem {
                entry: ClassificationEntry::fell_back(reason),
                credential: None,
            }),
            ws.path(),
        )
        .await;
        let a = lane_assist(&rec);
        assert_eq!(
            a.classifier_status,
            ClassifierStatus::FellBackToFixed {
                reason: reason.into()
            }
        );
        assert_eq!(a.basis, "fixed_fallback");
        assert!(!a.real_api_verified);
        assert_eq!(rec.decision.lane, fixed.decision.lane, "{reason}");
        // 故障しても実行とログは通常どおり進む。
        let run = store.decision_log_run_get(&run_id).unwrap().unwrap();
        assert_eq!(run.outcome.run_end, Some(RunEndLabel::Question), "{reason}");
    }
}

#[test]
fn jev_pick_source_is_covered_by_the_credential_gate() {
    // 鍵が要る source（jev）の pick は gate が待機に回す（上の通し試験と同じ経路の入口）。
    let e = E2e::new();
    let mut d = e.dispatcher();
    let (t, a, _) = e.task();
    assert!(d.credential_gate_hold(&t, Some(&a), &jev_pick()).unwrap());
}
