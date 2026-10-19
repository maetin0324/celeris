//! `routing.mode`（既定 fixed）と decision-assisted の lane 層（`dispatcher/lane_assist.rs`）。
//! 分類は注入した board の結果だけ。Jev・実 API は呼ばない。

use super::credential_gate::{SECRET, unit};
use super::*;
use crate::dispatcher::{BoardItem, ClassificationBoard, CredentialGate, CredentialGates};
use task_core::route_select::{
    ClassificationEntry, ClassificationHint, ClassifierStatus, LaneAssistRecord, RoutingMode,
    SpecificityHint,
};

pub(super) fn hinted_task(dir: &std::path::Path, source: task_core::TierSource) -> Task {
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
        tier_source: source,
        ..Default::default()
    });
    task
}

pub(super) fn routing_decided(store: &Arc<dyn TaskStore>, task: &Task) -> task_core::RoutingRecord {
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

fn lane_assist_of(rec: &task_core::RoutingRecord) -> Option<LaneAssistRecord> {
    rec.features
        .as_ref()
        .and_then(|f| f.decision.as_ref())
        .and_then(|d| d.lane_assist.clone())
}

pub(super) fn question_adapter() -> Arc<dyn WorkerAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Question {
            text: "not needed".into(),
        },
        delay: Duration::ZERO,
    })
}

fn dispatch_once(
    mode: Option<RoutingMode>,
    board_entry: Option<BoardItem>,
    source: task_core::TierSource,
) -> (task_core::RoutingRecord, Task) {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = hinted_task(ws.path(), source);
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    if let Some(mode) = mode {
        d.set_routing_mode(mode);
    }
    if let Some(item) = board_entry {
        d.classification_board().put(&task.id.to_string(), item);
    }
    d.tick().unwrap();
    (routing_decided(&store, &task), task)
}

pub(super) fn jev_ok(s: SpecificityHint) -> BoardItem {
    BoardItem {
        entry: ClassificationEntry {
            status: ClassifierStatus::JevOk,
            hint: Some(ClassificationHint {
                specificity: s,
                confidence: Some(0.95),
            }),
            model_snapshot: None,
            cost_usd: None,
        },
        credential: None,
    }
}

#[tokio::test]
async fn default_mode_is_fixed_and_writes_no_lane_assist() {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let d = dispatcher(store, question_adapter(), 1);
    assert_eq!(d.routing_mode(), RoutingMode::Fixed);
    drop(ws);
    // fixed では分類結果が board にあっても無視され、lane・record とも従来どおり。
    let (plain, _) = dispatch_once(None, None, task_core::TierSource::Hint);
    let (with_board, _) = dispatch_once(
        Some(RoutingMode::Fixed),
        Some(jev_ok(SpecificityHint::InsufficientInfo)),
        task_core::TierSource::Hint,
    );
    assert!(lane_assist_of(&plain).is_none());
    assert!(lane_assist_of(&with_board).is_none());
    assert_eq!(plain.decision.lane, with_board.decision.lane);
}

#[tokio::test]
async fn injected_failures_fall_back_to_fixed_and_real_api_stays_false() {
    let (fixed, _) = dispatch_once(None, None, task_core::TierSource::Hint);
    for reason in ["breaker_open", "timeout", "key_missing"] {
        let item = BoardItem {
            entry: ClassificationEntry::fell_back(reason),
            // 鍵が実際に登録済みなら（credential の宣言なし）待たずに fixed で進む。
            credential: None,
        };
        let (rec, _) = dispatch_once(
            Some(RoutingMode::DecisionAssisted),
            Some(item),
            task_core::TierSource::Hint,
        );
        let assist = lane_assist_of(&rec).expect("decision_assisted writes lane_assist");
        assert_eq!(
            assist.classifier_status,
            ClassifierStatus::FellBackToFixed {
                reason: reason.into()
            }
        );
        assert!(!assist.real_api_verified);
        assert_eq!(assist.basis, "fixed_fallback");
        assert_eq!(rec.decision.lane, fixed.decision.lane, "fixed と同じ lane");
    }
    // 分類が無い（分類段が走っていない）は not_attempted。
    let (rec, _) = dispatch_once(
        Some(RoutingMode::DecisionAssisted),
        None,
        task_core::TierSource::Hint,
    );
    assert_eq!(
        lane_assist_of(&rec).unwrap().classifier_status,
        ClassifierStatus::NotAttempted
    );
}

#[tokio::test]
async fn explicit_human_lane_beats_classification() {
    let (rec, task) = dispatch_once(
        Some(RoutingMode::DecisionAssisted),
        Some(jev_ok(SpecificityHint::NeedsInvestigationDesign)),
        task_core::TierSource::Human,
    );
    let assist = lane_assist_of(&rec).unwrap();
    assert_eq!(assist.overridden_by.as_deref(), Some("explicit"));
    assert_eq!(rec.decision.lane, task.worker_hint.tier);
    assert_eq!(assist.final_lane, assist.base_lane);
}

#[tokio::test]
async fn classification_only_raises_and_never_lowers_below_fixed() {
    let (fixed, _) = dispatch_once(None, None, task_core::TierSource::Hint);
    for s in [
        SpecificityHint::Specified,
        SpecificityHint::NeedsImplementation,
        SpecificityHint::NeedsInvestigationDesign,
        SpecificityHint::InsufficientInfo,
    ] {
        let (rec, _) = dispatch_once(
            Some(RoutingMode::DecisionAssisted),
            Some(jev_ok(s)),
            task_core::TierSource::Hint,
        );
        let rank = |t: task_core::Tier| match t {
            task_core::Tier::Cheap => 0,
            task_core::Tier::Standard => 1,
            task_core::Tier::Frontier => 2,
        };
        assert!(
            rank(rec.decision.lane) >= rank(fixed.decision.lane),
            "{s:?}"
        );
        let assist = lane_assist_of(&rec).unwrap();
        assert_eq!(assist.classifier_status, ClassifierStatus::JevOk);
        assert!(!assist.real_api_verified);
    }
}

#[test]
fn missing_key_enters_the_existing_credential_wait_for_that_unit_only() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = dir.path().join("secrets");
    std::fs::create_dir_all(&secrets).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = hinted_task(dir.path(), task_core::TierSource::Hint);
    store.insert(&task).unwrap();
    let a = unit(&task, "a", 0);
    let b = unit(&task, "b", 1);
    store
        .work_units_apply(task.id, vec![a.clone(), b.clone()], Vec::new(), Vec::new())
        .unwrap();
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_routing_mode(RoutingMode::DecisionAssisted);
    d.set_credential_gates(CredentialGates {
        secrets_dir: Some(secrets.clone()),
        by_source: Default::default(),
    });
    let board: ClassificationBoard = d.classification_board();
    board.put(
        &a.id,
        BoardItem {
            entry: ClassificationEntry::fell_back("key_missing"),
            credential: Some(CredentialGate {
                secret_id: SECRET.into(),
                provider: "jev".into(),
                purpose: "Jev で依頼を分類する".into(),
            }),
        },
    );
    let mut decision = task_core::model_policy::decide_for_task(&task, &Default::default())
        .expect("lane decision");
    let (record, held) = d.apply_lane_assist(&task, Some(&a), &mut decision).unwrap();
    assert!(held);
    assert_eq!(
        record.unwrap().classifier_status,
        ClassifierStatus::FellBackToFixed {
            reason: "key_missing".into()
        }
    );
    let waits = store.credential_waits_list_waiting().unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].secret_id, SECRET);
    assert_eq!(waits[0].work_unit_id, a.id);
    // 兄弟の unit は board に無いので待たない。
    let mut other = decision.clone();
    let (_, held_b) = d.apply_lane_assist(&task, Some(&b), &mut other).unwrap();
    assert!(!held_b);
    // 鍵を登録すれば待たず fixed で進む（待ちの解除は既存の resumer の仕事）。
    std::fs::write(secrets.join(SECRET), "sk-dummy-credential-value-123\n").unwrap();
    let mut again = decision.clone();
    let (_, held_again) = d.apply_lane_assist(&task, Some(&a), &mut again).unwrap();
    assert!(!held_again);
}

/// dispatcher・store の production code は Jev を直接呼ばない（分類は別段）。
#[test]
fn dispatcher_and_store_never_reference_the_jev_classifier() {
    fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                scan(&path, hits);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "lane_assist.rs")
            {
                let text = std::fs::read_to_string(&path).unwrap();
                if !path.to_string_lossy().contains("/tests/")
                    && (text.contains("jev_classifier") || text.contains("JevClassifier"))
                {
                    hits.push(path.display().to_string());
                }
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut hits = Vec::new();
    for sub in ["task-dispatch/src", "task-core/src"] {
        scan(&root.join(sub), &mut hits);
    }
    assert!(hits.is_empty(), "{hits:?}");
    let manifests = ["task-dispatch", "task-core"]
        .map(|c| std::fs::read_to_string(root.join(c).join("Cargo.toml")).unwrap());
    assert!(manifests.iter().all(|m| !m.contains("jev-classifier")));
}

/// ADR 2026-10-16-mlsys-measure-task-routing: task 欄 `routing_override` の task を、config の mode `config` の dispatcher で 1 回 dispatch する。
fn dispatch_with_override(
    config: RoutingMode,
    task_mode: Option<RoutingMode>,
    board: Option<BoardItem>,
) -> (task_core::RoutingRecord, Task) {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let task = hinted_task(ws.path(), task_core::TierSource::Hint);
    store.insert(&task).unwrap();
    if let Some(mode) = task_mode {
        let o = task_core::route_select::RoutingOverride {
            mode,
            lower_policy: None,
            evidence_mode: task_core::route_select::EvidenceMode::Pinned,
            evidence_table_id: Some("ev-2026-10-16".into()),
            online_update: false,
        };
        sqlite
            .set_task_routing_override(task.id, &o, "2026-10-16")
            .unwrap();
    }
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_routing_mode(config);
    if let Some(item) = board {
        d.classification_board().put(&task.id.to_string(), item);
    }
    d.tick().unwrap();
    (routing_decided(&store, &task), task)
}

fn resolved_of(rec: &task_core::RoutingRecord) -> Option<task_core::route_select::ResolvedRouting> {
    rec.features
        .as_ref()
        .and_then(|f| f.decision.as_ref())
        .and_then(|d| d.routing_resolved.clone())
}

#[tokio::test]
async fn task_fixed_beats_config_decision_assisted_and_no_assist_runs() {
    let (plain, _) = dispatch_once(None, None, task_core::TierSource::Hint);
    let (rec, _) = dispatch_with_override(
        RoutingMode::DecisionAssisted,
        Some(RoutingMode::Fixed),
        Some(jev_ok(SpecificityHint::NeedsInvestigationDesign)),
    );
    assert!(
        lane_assist_of(&rec).is_none(),
        "fixed の task では補助が走らない"
    );
    assert_eq!(rec.decision.lane, plain.decision.lane);
    let resolved = resolved_of(&rec).expect("解決結果が判断 record に残る");
    assert_eq!(resolved.mode, RoutingMode::Fixed);
    assert_eq!(resolved.source, task_core::route_select::ModeSource::Task);
    assert_eq!(resolved.evidence_table_id.as_deref(), Some("ev-2026-10-16"));
    assert!(!resolved.online_update);
    assert_eq!(
        resolved.lower_policy,
        task_core::route_select::DEFAULT_POLICY_VERSION
    );
}

#[tokio::test]
async fn task_decision_assisted_beats_config_fixed_and_assist_runs() {
    let (rec, _) = dispatch_with_override(
        RoutingMode::Fixed,
        Some(RoutingMode::DecisionAssisted),
        Some(jev_ok(SpecificityHint::Specified)),
    );
    let assist = lane_assist_of(&rec).expect("decision_assisted の task では補助が走る");
    assert_eq!(assist.mode, "decision_assisted");
    assert_eq!(assist.classifier_status, ClassifierStatus::JevOk);
    let resolved = resolved_of(&rec).expect("resolved");
    assert_eq!(resolved.mode, RoutingMode::DecisionAssisted);
    assert_eq!(resolved.source, task_core::route_select::ModeSource::Task);
}

#[tokio::test]
async fn without_the_task_field_the_config_mode_decides_and_no_resolution_is_written() {
    let (rec, _) = dispatch_with_override(RoutingMode::DecisionAssisted, None, None);
    assert!(lane_assist_of(&rec).is_some(), "従来どおり config の mode");
    assert!(
        resolved_of(&rec).is_none(),
        "task 欄の無い task の record は従来と同一"
    );
}

/// Jev の分類費用は campaign の消費行（role=jev）に入る。費用が返らなければ欠測、再処理で増えない。
#[tokio::test]
async fn jev_classification_cost_is_metered_once_per_campaign() {
    for (cost, expect) in [(Some(0.0004), Some(0.0004)), (None, None)] {
        let ws = tempfile::tempdir().unwrap();
        let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
        let store: Arc<dyn TaskStore> = sqlite.clone();
        let task = hinted_task(ws.path(), task_core::TierSource::Hint);
        store.insert(&task).unwrap();
        sqlite
            .task_campaign_set(
                &task.id.to_string(),
                &task_core::decision_log::CampaignIdentity {
                    campaign_id: "camp-j".into(),
                    case_id: None,
                    arm: None,
                    repetition: None,
                    manifest_hash: None,
                    routing_setting: None,
                },
                OffsetDateTime::now_utc(),
            )
            .unwrap();
        let mut item = jev_ok(SpecificityHint::InsufficientInfo);
        item.entry.cost_usd = cost;
        let mut d = dispatcher(store.clone(), question_adapter(), 1);
        d.set_routing_mode(RoutingMode::DecisionAssisted);
        d.classification_board().put(&task.id.to_string(), item);
        d.tick().unwrap();
        d.tick().unwrap();
        let rows: Vec<_> = sqlite
            .decision_log_usage_list(Some("camp-j"))
            .unwrap()
            .into_iter()
            .filter(|u| u.role == task_core::decision_log::UsageRole::Jev)
            .collect();
        assert_eq!(rows.len(), 1, "同じ分類を 2 tick 読んでも 1 行");
        assert_eq!(rows[0].list_cost_usd, expect);
        assert!(!rows[0].shared_overhead);
    }
}

/// Jev の複数呼び出し（失敗・再試行・再分類）は call_no 固定で上書きされず、別行として残る。
/// 費用が返らない呼び出しは欠測（NULL）で、0 にならない。再読み込みで増えない。
#[tokio::test]
async fn jev_calls_are_kept_per_call_and_missing_cost_stays_null() {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let task = hinted_task(ws.path(), task_core::TierSource::Hint);
    store.insert(&task).unwrap();
    sqlite
        .task_campaign_set(
            &task.id.to_string(),
            &task_core::decision_log::CampaignIdentity {
                campaign_id: "camp-k".into(),
                case_id: None,
                arm: None,
                repetition: None,
                manifest_hash: None,
                routing_setting: None,
            },
            OffsetDateTime::now_utc(),
        )
        .unwrap();
    let subject = task.id.to_string();
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_routing_mode(RoutingMode::DecisionAssisted);
    let board = d.classification_board();
    let jev_rows = |sqlite: &SqliteStore| {
        sqlite
            .decision_log_usage_list(Some("camp-k"))
            .unwrap()
            .into_iter()
            .filter(|u| u.role == task_core::decision_log::UsageRole::Jev)
            .collect::<Vec<_>>()
    };
    // 1 回目: タイムアウトで fixed に戻る（費用なし）。2 回目: 成功（費用あり）。
    // 3 回目: 再分類が成功したが費用は返らない。dispatcher が読む前に置き換わっても 3 件とも残る。
    board.put(
        &subject,
        BoardItem {
            entry: ClassificationEntry::fell_back("timeout"),
            credential: None,
        },
    );
    let mut ok = jev_ok(SpecificityHint::InsufficientInfo);
    ok.entry.cost_usd = Some(0.0004);
    board.put(&subject, ok);
    board.put(&subject, jev_ok(SpecificityHint::InsufficientInfo));
    d.tick().unwrap();
    d.tick().unwrap();
    let rows = jev_rows(&sqlite);
    assert_eq!(
        rows.len(),
        3,
        "3 呼び出しが別行で残り、再読み込みで増えない"
    );
    let mut costs: Vec<_> = rows.iter().map(|r| r.list_cost_usd).collect();
    costs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(costs, vec![None, None, Some(0.0004)], "欠測は 0 にならない");
    assert_eq!(rows.iter().filter(|r| r.is_retry).count(), 2);
    assert!(rows.iter().any(|r| {
        r.decision_id
            .as_deref()
            .is_some_and(|d| d.contains(":fallback:timeout:"))
    }));
}

/// 修正 3: campaign root の routing 固定は子・孫・再試行（root の別の子）へ継承され、
/// 全体 config を逆の mode にしても各 arm の routing（mode・online_update・根拠表）が保たれる。
#[tokio::test]
async fn root_routing_pin_is_inherited_by_child_grandchild_and_retry_whatever_the_config() {
    for (config, pinned) in [
        (RoutingMode::DecisionAssisted, RoutingMode::Fixed),
        (RoutingMode::Fixed, RoutingMode::DecisionAssisted),
    ] {
        let ws = tempfile::tempdir().unwrap();
        let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
        let store: Arc<dyn TaskStore> = sqlite.clone();
        let root = hinted_task(ws.path(), task_core::TierSource::Hint);
        store.insert(&root).unwrap();
        let mut child = hinted_task(ws.path(), task_core::TierSource::Hint);
        child.parent_id = Some(root.id);
        store.insert(&child).unwrap();
        let mut grand = hinted_task(ws.path(), task_core::TierSource::Hint);
        grand.parent_id = Some(child.id);
        store.insert(&grand).unwrap();
        let mut retry = hinted_task(ws.path(), task_core::TierSource::Hint);
        retry.parent_id = Some(root.id);
        store.insert(&retry).unwrap();
        let outsider = hinted_task(ws.path(), task_core::TierSource::Hint);
        store.insert(&outsider).unwrap();
        let o = task_core::route_select::RoutingOverride {
            mode: pinned,
            lower_policy: None,
            evidence_mode: task_core::route_select::EvidenceMode::Pinned,
            evidence_table_id: Some("ev-arm".into()),
            online_update: false,
        };
        sqlite
            .set_task_routing_override(root.id, &o, "2026-10-16")
            .unwrap();
        let mut d = dispatcher(store.clone(), question_adapter(), 1);
        d.set_routing_mode(config);
        for t in [&root, &child, &grand, &retry] {
            let r = d.routing_for(t);
            assert_eq!(r.mode, pinned, "{config:?} config must not leak");
            assert_eq!(r.source, task_core::route_select::ModeSource::Task);
            assert_eq!(r.evidence_table_id.as_deref(), Some("ev-arm"));
            assert!(!r.online_update);
        }
        // campaign に属さない task は従来どおり config に従う。
        let r = d.routing_for(&outsider);
        assert_eq!(r.mode, config);
        assert_eq!(r.source, task_core::route_select::ModeSource::Config);
    }
}

#[tokio::test]
async fn task_override_evidence_mode_none_is_recorded_without_a_table_version() {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let task = hinted_task(ws.path(), task_core::TierSource::Hint);
    store.insert(&task).unwrap();
    let o = task_core::route_select::RoutingOverride {
        mode: RoutingMode::Fixed,
        lower_policy: None,
        evidence_mode: task_core::route_select::EvidenceMode::None,
        evidence_table_id: None,
        online_update: false,
    };
    sqlite
        .set_task_routing_override(task.id, &o, "2026-10-17")
        .unwrap();
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.tick().unwrap();
    let rec = routing_decided(&store, &task);
    let resolved = resolved_of(&rec).expect("解決結果が判断 record に残る");
    assert_eq!(
        resolved.evidence_mode,
        task_core::route_select::EvidenceMode::None
    );
    assert_eq!(resolved.evidence_table_id, None);
}
