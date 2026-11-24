//! ADR 2026-11-22-parent-checkpoint-evaluator D3.1.1・D8.3: 盲検の評価の入力（E-mid・D-done）と live の sample。
//!
//! - `parent_checkpoint_live_request_renders`: E-mid・D-done の slice（`tests/fixtures/parent_checkpoint_shadow/` の写し。manifest の
//!   sha256 と cutoff を確かめる）から `ParentCheckpointRequest` を作り、task-worker の本物の型と prompt（`parent_checkpoint_section` と
//!   `result_section`）で request と prompt を出し、commit した `fixtures/parent_checkpoint_live/<時点>.request.json`・`<時点>.prompt.md` と
//!   `fixtures/parent_checkpoint_shadow/<時点>/reports.json` と byte 単位で一致することを確かめる。子の報告は本番と同じ
//!   `task_core::parent_checkpoint::child_reports`（D3.1.1）で選ぶ。他の欄は dispatcher の `review::parent_evaluation::request`
//!   （task-api から呼べない private の関数）と同じ出どころで、id は固定の値。`goal.outcomes` と子の `criteria` は、盲検の seed と
//!   同じ [`seed`] で一時 DB に置いた案件から、dispatcher の `goal` と同じ D6 の読み口（`project_controller_view::reach`・
//!   `unverified_outcomes`）と `project_snapshot::acceptance_criteria` で読む（WU eval-seed）。`PARENT_CHECKPOINT_LIVE_WRITE=1` の
//!   ときだけ書き直す。
//! - `parent_checkpoint_blind_`: commit した入力に D#1446 の本文が入る・承認済み Roadmap の Outcome と子の criteria が入る・answer key
//!   の印と時点より後の記録（時刻・報告の本文・未来にだけ現れる語）が無い・出力の欄と axes の定義がある・上限で切る、を確かめる。
//! - `parent_checkpoint_live_sample_checks`（`#[ignore]`）: live-evaluator-sample の WU が書いた `E-mid.evaluation.json` を
//!   `check_evaluation` に通し、形・規則・全参照が slice の中で cutoff 以前かを検査し、answer key との一致を出力する。
//!
//! 守る不変条件: 評価器の入力（request・prompt・slice）に子の報告の本文が時点まで入り、answer key の印と時点より後の記録が入らない。
//! 既存の試験で検出できない理由: `parent_checkpoint_live_request_renders` の前の形は報告の欄を持たず、E-mid の入力に本文が無いこと
//! （D#1446 を読めないこと）と、prompt に出力の形が無いことを見なかった。
//!
//! 守る不変条件（WU eval-seed。ADR 2026-11-24-strategy-pilot-ablation D8）: 盲検の seed は承認済み Roadmap の Outcome（goal.md の
//! 共通受入条件の文。status・evidence は cutoff 以前の記録だけから決める）と、cutoff 以前に review の記録がある子の受け入れ条件を置き、
//! E-mid・D-done の request・prompt に Outcome と criteria が出て「（Outcome の記録は無い）」が出ない。Outcome にも criteria にも
//! cutoff より後の値（未来にだけ現れる語）が入らない。
//! 既存の試験で検出できない理由: 前の写しは `goal.outcomes` と `criteria` を空で組み立て、commit した fixture（空の入力）と byte で
//! 比べるだけだった。seed は Roadmap も子の受け入れ条件も置かず、dispatcher の `request` を直しても盲検の入力は空のままで、どの試験も
//! 落ちない。否定対照は時点より後の時刻と 40 字を超える報告の本文だけを見て、Outcome の文に入りうる最終の値（`17 → 17` など）を見なかった。

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};
use task_core::parent_checkpoint as pc;
use task_core::project_proposal::{self as pp, SnapshotCriterion};
use task_core::roadmap::{RoadmapOutcomeSpec, RoadmapProposalInput};
use task_core::store::SqliteStore;
use task_core::strategy::Reference;
use task_core::strategy_ref::StrategyRef;
use task_core::{Check, Criterion, Event, ProjectId, RunRole, Status, TaskId};
use task_worker::protocol::{
    ParentCheckpointChild, ParentCheckpointGoal, ParentCheckpointMetrics, ParentCheckpointOutcome,
    ParentCheckpointRequest, ReviewRequest,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const SHADOW: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/parent_checkpoint_shadow"
);
const LIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/parent_checkpoint_live"
);
const MANIFEST: &str =
    include_str!("../../../agent-docs/progress/2026-11-21-strategy-v1-0/replay/manifest.sha256");
const ANSWER_KEY: &str =
    include_str!("../../../agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json");
const ADR: &str = include_str!("../../../agent-docs/adr/2026-11-22-parent-checkpoint-evaluator.md");
const PROGRESS: &str =
    include_str!("../../../agent-docs/progress/2026-11-22-strategy-v1-6/blind-eval-input.md");
const ANSWER_MARKS: [&str; 3] = ["ideal_verdict", "ideal_action", "answer_key"];
const CHILDREN: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];
/// 盲検の評価の時点（D8.3）と、その判断の trigger（commit した request・prompt の時点）。
const CASES: [(&str, &str); 2] = [("E-mid", "children:2"), ("D-done", "children:4")];
/// 対照 pilot の replay の時点（ADR 2026-11-24-strategy-pilot-ablation D2.1）と trigger。盲検の seed はこの時点と [`CASES`] を受ける。
const PILOT_POINTS: [(&str, &str); 3] = [
    ("C-mid", "children:2"),
    ("D-done", "children:4"),
    ("F-done", "children:6"),
];
/// fixture の固定の id（root はアーキv4 の親の実 id、子は letter の順の固定 ULID）。
const ROOT_ID: &str = "01M7MZT8KX547464YYS76QNP4G";
/// prompt の Result 節の成果物の dir（固定）。
const ARTIFACTS: &str = "<artifacts>";
/// 未来にだけ現れる語（子 0 の replay の sentinels。`first_seen` が時点より後の語は入力に入らない）。
const SENTINELS: &str =
    include_str!("../../../agent-docs/progress/2026-11-21-strategy-v1-0/replay/sentinels.json");
/// 使い捨ての案件の承認済み Roadmap の Outcome の id（版 1 の 1 件）。
const OUTCOME_ID: &str = "arch-v4";
/// 子の受け入れ条件の本文（slice は条件の本文を持たない。replay の export が task の acceptance を落とす）。
const NO_TEXT: &str = "（条件の本文は slice に無い）";
/// 使い捨ての DB の Roadmap の提案と承認の actor。人の承認ではなく、一時 DB の中の段取り（ADR 2026-11-24-strategy-pilot-ablation）。
const SEED_ACTOR: &str = "human:blind-seed";
/// 評価器の prompt が Outcome の無い request に出す行（eval-input の前の盲検の入力にあった）。
const ABSENT: &str = "（Outcome の記録は無い）";

fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn manifest_sha(path: &str) -> Option<&'static str> {
    MANIFEST.lines().find_map(|line| {
        let (sha, p) = line.split_once("  ")?;
        (p == path).then_some(sha)
    })
}

fn ts(raw: &str) -> OffsetDateTime {
    OffsetDateTime::parse(raw, &Rfc3339).expect("rfc3339")
}

fn child_id(i: usize) -> TaskId {
    format!("01M7MZT8KX5474640000000{i:03}")
        .parse()
        .expect("child id")
}

/// 時点の slice: cutoff 以下の events（G-done を cutoff で切る。manifest の sha256 と一致を確かめる）と goal.md・cutoff。
fn slice(case: &str) -> (Vec<Value>, String, String) {
    let meta: Value = serde_json::from_slice(
        &std::fs::read(Path::new(SHADOW).join(case).join("slice.json")).expect("slice"),
    )
    .expect("slice json");
    let cutoff = meta["cutoff"].as_str().expect("cutoff").to_string();
    let cut = ts(&cutoff);
    let all = std::fs::read_to_string(Path::new(SHADOW).join("G-done/events.jsonl")).expect("all");
    let mut text = String::new();
    let mut rows = Vec::new();
    for line in all.lines().filter(|l| !l.trim().is_empty()) {
        let row: Value = serde_json::from_str(line).expect("row");
        if ts(row["ts"].as_str().expect("ts")) <= cut {
            text.push_str(line);
            text.push('\n');
            rows.push(row);
        }
    }
    assert_eq!(
        manifest_sha(&format!("{case}/events.jsonl")),
        Some(sha_hex(text.as_bytes()).as_str()),
        "{case} の events が子 0 の slice と違う"
    );
    let goal = std::fs::read_to_string(Path::new(SHADOW).join("E-mid/goal.md")).expect("goal");
    assert_eq!(
        manifest_sha(&format!("{case}/goal.md")),
        Some(sha_hex(goal.as_bytes()).as_str())
    );
    (rows, goal, cutoff)
}

/// replay の行を本番の event にする（報告の選択が読む `WorkerFinished` だけ。`role` が無い行は worker の run）。
fn worker_finished(row: &Value) -> Option<Event> {
    if row["type"] != "worker_finished" {
        return None;
    }
    let role = match row["role"].as_str() {
        None => None,
        Some(r) => Some(serde_json::from_value::<RunRole>(Value::from(r)).ok()?),
    };
    Some(Event::WorkerFinished {
        run_id: format!("replay-{}-{}", row["task"].as_str()?, row["seq"]),
        outcome: row["outcome"].as_str().unwrap_or_default().to_string(),
        usage: None,
        role,
        metrics: None,
        end: None,
    })
}

fn status_of(v: &Value) -> Status {
    serde_json::from_value(v.clone()).expect("status")
}

/// 承認済み Roadmap の Outcome（D8）: title は goal.md の「目的：」の最初の文、acceptance は「共通受入条件：」の文（。で区切る）。
/// どちらも goal.md（親の起票の時点の目的）の文字どおりで、cutoff より後の値（v4 の最終の結果）は入らない。
fn outcome_spec(goal: &str) -> RoadmapOutcomeSpec {
    let line = |head: &str| {
        goal.lines()
            .find_map(|l| l.strip_prefix(head))
            .unwrap_or_else(|| panic!("goal.md に {head} の行が無い"))
    };
    let sentences = |text: &str| -> Vec<String> {
        text.split_inclusive('。')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    };
    let title = sentences(line("目的："))
        .into_iter()
        .next()
        .expect("目的の文")
        .trim_end_matches('。')
        .to_string();
    RoadmapOutcomeSpec {
        id: OUTCOME_ID.into(),
        title,
        acceptance: sentences(line("共通受入条件：")),
        priority: 0,
        depends_on: Vec::new(),
        dropped: false,
        checks: Vec::new(),
    }
}

/// 子 `key` の slice の行（cutoff 以前）。
fn rows_of<'a>(rows: &'a [Value], key: &str) -> Vec<&'a Value> {
    rows.iter().filter(|r| r["task"] == key).collect()
}

/// 子の受け入れ条件（cutoff 以前の記録から。本文は [`NO_TEXT`]）。最後の `reviewer_passed` の `criteria`（`Check::Reviewer` の
/// 条件の index）と、判定の理由が `cmd=`（`Check::Command`。workspace の検査は `workspace.toml check: ` が付く）・`path=`
/// （`Check::ArtifactExists`）で始まる index を自分の条件とし、0 から隙間なく並ぶときだけ置く。`reviewer_passed` の無い子
/// （review の前・不合格だけ）は条件の数と種類が slice から決まらないので置かない。
fn child_criteria(rows: &[&Value]) -> Vec<Criterion> {
    let Some(passed) = rows.iter().rfind(|r| r["type"] == "reviewer_passed") else {
        return Vec::new();
    };
    let reviewer: Vec<usize> =
        serde_json::from_value(passed["criteria"].clone()).expect("reviewer_passed criteria");
    let mut checks: BTreeMap<usize, Check> =
        reviewer.into_iter().map(|i| (i, Check::Reviewer)).collect();
    for r in rows.iter().filter(|r| r["type"] == "review_verdict") {
        let idx = r["criterion_idx"].as_u64().expect("criterion_idx") as usize;
        let reason = r["reason"].as_str().unwrap_or_default();
        if let Some(rest) = reason.strip_prefix("cmd=") {
            let cmd = serde_json::Deserializer::from_str(rest)
                .into_iter::<String>()
                .next()
                .and_then(Result::ok)
                .expect("cmd の文字列");
            let expect_exit = reason
                .split(" expected=")
                .nth(1)
                .and_then(|s| s.split(' ').next())
                .and_then(|s| s.parse().ok())
                .expect("expected の値");
            checks.insert(idx, Check::Command { cmd, expect_exit });
        } else if let Some(rest) = reason.strip_prefix("path=") {
            let path = rest.split(' ').next().unwrap_or_default();
            let name = path.strip_prefix("artifacts/").unwrap_or(path).to_string();
            checks.insert(idx, Check::ArtifactExists { name });
        }
    }
    let n = checks.keys().next_back().map_or(0, |i| i + 1);
    assert_eq!(
        checks.len(),
        n,
        "条件の index に隙間がある: {:?}",
        checks.keys()
    );
    checks
        .into_values()
        .map(|check| Criterion {
            text: NO_TEXT.into(),
            check,
        })
        .collect()
}

/// 子の slice の行を本番の event にする（遷移・`WorkerFinished`（報告の本文）・review の判定・`ReviewerPassed`）。
fn child_events(rows: &[&Value]) -> Vec<Event> {
    rows.iter()
        .filter_map(|row| {
            let run = format!(
                "replay-{}-{}",
                row["task"].as_str().unwrap_or("-"),
                row["seq"]
            );
            Some(match row["type"].as_str()? {
                "transitioned" => Event::Transitioned {
                    from: status_of(&row["from"]),
                    to: status_of(&row["to"]),
                    reason: row["reason"].as_str().unwrap_or_default().to_string(),
                },
                "worker_finished" => worker_finished(row)?,
                "review_verdict" => Event::ReviewVerdict {
                    run_id: run,
                    criterion_idx: row["criterion_idx"].as_u64()? as usize,
                    pass: row["pass"].as_bool()?,
                    reason: row["reason"].as_str().unwrap_or_default().to_string(),
                },
                "reviewer_passed" => Event::ReviewerPassed {
                    review_run: run,
                    repo_id: task_core::repos::RepoId::new(),
                    target_sha: String::new(),
                    reviewed_sha: String::new(),
                    criteria: serde_json::from_value(row["criteria"].clone()).unwrap_or_default(),
                },
                _ => return None,
            })
        })
        .collect()
}

/// [`seed`] が置いた案件の id。
struct Seeded {
    project_id: ProjectId,
    root_id: TaskId,
    children: BTreeMap<String, TaskId>,
}

/// 時点 `point` の slice から、使い捨ての `store` に案件・承認済み Roadmap・strategy root・子を store の公開の関数で置く
/// （盲検の seed と、写しの `goal.outcomes`・`criteria` の出どころ）。
///
/// - 案件の request と root の objective は slice の goal.md。Roadmap の版 1 は [`outcome_spec`] の Outcome 1 件を [`SEED_ACTOR`] が
///   提案して承認する。Outcome の status・evidence は cutoff 以前の記録だけから決める: slice に親（P）の到達の記録（done への遷移・
///   `ReviewerPassed`）は無いので Outcome の event を書かない（status は承認の時点の open、evidence は空、D6 の到達は偽）。
/// - 子の行の status は cutoff の時点の値。時点で ready の子は案件の外の draft の task に依存させ、daemon が起こさない。子は slice の
///   title と、[`child_criteria`] の受け入れ条件と、[`child_events`] の events を持つ。root と子の作業場所は `repo`（mode shared）。
/// - strategy root の正本（ADR 2026-11-22-parent-checkpoint-evaluator D1.1）は gate と同じ `task.create` の操作の記録。
fn seed(store: &SqliteStore, point: &str, repo: &Path) -> Seeded {
    use task_core::chat::{AuditContext, ChatActor};
    use task_core::{Project, Task, TaskKind, WorkspaceMode, WorkspaceSpec};

    let (rows, goal, _) = slice(point);
    let now = OffsetDateTime::now_utc();
    let project = Project {
        id: ProjectId::new(),
        title: format!("blind:{point}"),
        request: goal.clone(),
        status: task_core::ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("project");
    let pid = project.id.to_string();
    assert!(
        !rows
            .iter()
            .any(|r| r["task"] == "P" && (r["type"] == "reviewer_passed" || r["to"] == "done")),
        "{point}: cutoff 以前に親の到達の記録がある（Outcome の status を決め直す）"
    );
    let input = RoadmapProposalInput {
        base_version: None,
        change_reason: format!("盲検の評価の seed（{point}）: goal.md の共通受入条件"),
        outcomes: vec![outcome_spec(&goal)],
    };
    let version = store
        .write_tx(|tx| {
            SqliteStore::roadmap_propose_tx(
                tx,
                &pid,
                &input,
                &pp::sha256_hex(&goal),
                SEED_ACTOR,
                now,
            )
        })
        .expect("roadmap propose");
    store
        .write_tx(|tx| {
            SqliteStore::roadmap_decide_tx(tx, &pid, version.version, true, None, SEED_ACTOR, now)
        })
        .expect("roadmap approve");
    let workspace = WorkspaceSpec::Local {
        path: repo.to_path_buf(),
        mode: Some(WorkspaceMode::Shared),
    };
    let task = |status: Status, title: String, objective: String, parent: Option<TaskId>| Task {
        title,
        objective,
        project_id: Some(project.id),
        parent_id: parent,
        workspace: workspace.clone(),
        ..common::new_task(TaskKind::Execute, status)
    };
    let root = task(Status::Running, "[アーキv4 親]".into(), goal.clone(), None);
    store.create_task(&root, vec![]).expect("root");
    // 時点で ready の子を daemon が起こさないように、案件の外の draft の task に依存させる（評価器の入力に依存は入らない）。
    let mut holder = common::new_task(TaskKind::Execute, Status::Draft);
    holder.title = "blind: hold".into();
    store.create_task(&holder, vec![]).expect("holder");
    let mut children = BTreeMap::new();
    for key in CHILDREN {
        let mine = rows_of(&rows, key);
        let Some(title) = mine.iter().find_map(|r| r["title"].as_str()) else {
            continue;
        };
        // 子の行の status は cutoff の時点の値（遷移の events を足しても行の status は変わらない）。
        let last = mine
            .iter()
            .filter(|r| r["type"] == "transitioned")
            .map(|r| status_of(&r["to"]))
            .next_back()
            .unwrap_or(Status::Draft);
        let mut t = task(last, title.to_string(), String::new(), Some(root.id));
        t.acceptance = child_criteria(&mine);
        if last == Status::Ready {
            t.depends_on = vec![holder.id];
        }
        store.create_task(&t, vec![]).expect("child");
        for event in child_events(&mine) {
            store.append_event(t.id, &event).expect("append");
        }
        children.insert(key.to_string(), t.id);
    }
    let op = ulid::Ulid::new().to_string();
    let strategy = serde_json::json!({
        "task_id": root.id.to_string(),
        "name": "strategy/arch-v4-cycle/v1.json",
        "sha256": format!("sha256:{}", "a".repeat(64)),
    });
    store
        .cos_operation_apply_at(
            &AuditContext {
                actor: ChatActor::Cos,
                thread_id: "blind-seed".into(),
                run_id: "blind-seed".into(),
                operation_id: op.clone(),
                reason: "盲検の評価の seed（アーキv4 の親）".into(),
                policy_version: "v1".into(),
            },
            &op,
            "blind-seed",
            "task",
            &root.id.to_string(),
            None,
            "task.create",
            &serde_json::json!({"method": "POST", "path": "/api/v1/tasks",
                                "body": {"title": root.title}, "strategy": strategy}),
            now,
            |_, _| Ok(serde_json::json!({})),
        )
        .expect("root operation");
    Seeded {
        project_id: project.id,
        root_id: root.id,
        children,
    }
}

/// 写しの `goal.outcomes` と子の `criteria`（letter ごと）: [`seed`] で一時 DB に置いた案件から、dispatcher の
/// `review::parent_evaluation::goal` と同じ D6 の読み口（`reach`・`unverified_outcomes`）と、`request` と同じ
/// `project_snapshot::acceptance_criteria` で読む。
fn seeded_inputs(
    case: &str,
) -> (
    Vec<ParentCheckpointOutcome>,
    BTreeMap<String, Vec<SnapshotCriterion>>,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::open(&dir.path().join("celeris.db")).expect("store");
    let seeded = seed(&store, case, dir.path());
    let pid = seeded.project_id.to_string();
    let now = OffsetDateTime::now_utc();
    let state = store
        .project_controller_state(&pid)
        .expect("state")
        .unwrap_or_else(|| {
            task_core::store::project_controller_default_state(
                &pid,
                &now.format(&Rfc3339).expect("now"),
            )
        });
    let (snapshot, recorded) =
        task_ops::project_controller_view::reach(&store, seeded.project_id, &state, now)
            .expect("reach");
    let unverified = pp::unverified_outcomes(&snapshot, &recorded);
    let outcomes = snapshot
        .outcomes
        .iter()
        .map(|o| ParentCheckpointOutcome {
            id: o.id.clone(),
            title: o.title.clone(),
            acceptance: o.acceptance.clone(),
            reached: !unverified.contains(&o.id),
        })
        .collect();
    let criteria = seeded
        .children
        .iter()
        .map(|(key, id)| {
            let child = store.get(*id).expect("get").expect("child");
            let events = store.events_for(*id).expect("events");
            let criteria = task_ops::project_snapshot::acceptance_criteria(
                &child,
                events.iter().map(|(_, e)| e),
            );
            (key.clone(), criteria)
        })
        .collect();
    (outcomes, criteria)
}

/// D3.1 の入力を時点の slice から作る（dispatcher の `request` と同じ出どころ。報告は本番の `child_reports`、Outcome と criteria
/// は [`seeded_inputs`]）。
fn request(case: &str, trigger: &str) -> (ParentCheckpointRequest, BTreeMap<String, TaskId>) {
    let (rows, goal, cutoff) = slice(case);
    let (outcomes, mut criteria) = seeded_inputs(case);
    let ids: BTreeMap<String, TaskId> = CHILDREN
        .iter()
        .enumerate()
        .map(|(i, k)| (k.to_string(), child_id(i + 1)))
        .collect();
    let present: Vec<&str> = CHILDREN
        .into_iter()
        .filter(|k| {
            rows.iter()
                .any(|r| r["task"] == *k && r["title"].is_string())
        })
        .collect();
    let events: Vec<Vec<(u64, String, Event)>> = present
        .iter()
        .map(|k| {
            rows.iter()
                .filter(|r| r["task"] == *k)
                .filter_map(|r| {
                    let e = worker_finished(r)?;
                    Some((r["seq"].as_u64()?, r["ts"].as_str()?.to_string(), e))
                })
                .collect()
        })
        .collect();
    let borrowed: Vec<Vec<(u64, &str, &Event)>> = events
        .iter()
        .map(|es| es.iter().map(|(s, t, e)| (*s, t.as_str(), e)).collect())
        .collect();
    let reports = pc::child_reports(&borrowed, &cutoff);
    let mut children = Vec::new();
    for (key, reports) in present.iter().zip(reports) {
        let mine = || rows.iter().filter(|r| r["task"] == *key);
        let title = mine().find_map(|r| r["title"].as_str()).unwrap_or_default();
        let status = mine()
            .filter(|r| r["type"] == "transitioned")
            .filter_map(|r| serde_json::from_value::<Status>(r["to"].clone()).ok())
            .last()
            .unwrap_or(Status::Draft);
        children.push(ParentCheckpointChild {
            task_id: ids[*key],
            title: title.to_string(),
            status,
            reviewer_passed: mine().any(|r| r["type"] == "reviewer_passed"),
            criteria: criteria.remove(*key).unwrap_or_default(),
            reports,
        });
    }
    let metrics = ParentCheckpointMetrics {
        children_terminal: children.iter().filter(|c| c.status.is_terminal()).count() as u32,
        children_total: children.len() as u32,
        tokens_used: 0,
        max_tokens: None,
    };
    let req = ParentCheckpointRequest {
        decision_id: format!("{}-blind", case.to_lowercase()),
        root_id: ROOT_ID.parse().expect("root id"),
        triggers: vec![trigger.into()],
        goal: ParentCheckpointGoal {
            request_sha256: format!("sha256:{}", sha_hex(goal.as_bytes())),
            outcomes,
        },
        strategy: StrategyRef {
            task_id: ROOT_ID.into(),
            name: "strategy/arch-v4-cycle/v1.json".into(),
            sha256: format!("sha256:{}", "a".repeat(64)),
            candidate: None,
        },
        children,
        metrics,
        previous: Vec::new(),
        rubric: ParentCheckpointRequest::fixed_rubric(),
    };
    (req, ids)
}

/// request.json・prompt.md・reports.json の本文。
fn rendered(case: &str, trigger: &str) -> (String, String, String) {
    let (req, _) = request(case, trigger);
    let (_, ids) = request(case, trigger);
    let reports: BTreeMap<&str, &[pc::ChildReport]> = req
        .children
        .iter()
        .filter_map(|c| {
            let key = ids.iter().find(|(_, id)| **id == c.task_id)?.0;
            Some((key.as_str(), c.reports.as_slice()))
        })
        .collect();
    let (_, _, cutoff) = slice(case);
    let reports = serde_json::to_string_pretty(&serde_json::json!({
        "case": case, "cutoff": cutoff, "reports": reports,
    }))
    .expect("reports json")
        + "\n";
    let review = ReviewRequest {
        parent_checkpoint: Some(Box::new(req)),
        ..ReviewRequest::default()
    };
    let json = serde_json::to_string_pretty(&review).expect("request json") + "\n";
    let prompt = task_worker::change_rules::review::parent_checkpoint_section(Some(&review))
        + &task_worker::change_rules::review::result_section(Some(&review), ARTIFACTS);
    (json, prompt, reports)
}

fn committed(case: &str) -> [(String, String); 3] {
    let read = |p: std::path::PathBuf| {
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        (p.display().to_string(), text)
    };
    [
        read(Path::new(LIVE).join(format!("{case}.request.json"))),
        read(Path::new(LIVE).join(format!("{case}.prompt.md"))),
        read(Path::new(SHADOW).join(case).join("reports.json")),
    ]
}

#[test]
fn parent_checkpoint_live_request_renders() {
    for (case, trigger) in CASES {
        let (json, prompt, reports) = rendered(case, trigger);
        if std::env::var_os("PARENT_CHECKPOINT_LIVE_WRITE").is_some() {
            std::fs::write(Path::new(LIVE).join(format!("{case}.request.json")), &json)
                .expect("write request");
            std::fs::write(Path::new(LIVE).join(format!("{case}.prompt.md")), &prompt)
                .expect("write prompt");
            std::fs::write(Path::new(SHADOW).join(case).join("reports.json"), &reports)
                .expect("write reports");
        }
        let [(_, req), (_, pr), (_, rep)] = committed(case);
        assert_eq!(json, req, "{case}.request.json が本物の型の出力と違う");
        assert_eq!(prompt, pr, "{case}.prompt.md が本物の prompt と違う");
        assert_eq!(
            reports, rep,
            "{case}/reports.json が child_reports の出力と違う"
        );
        let (r, _) = request(case, trigger);
        let n: usize = r.children.iter().map(|c| c.reports.len()).sum();
        let bytes: usize = r
            .children
            .iter()
            .flat_map(|c| &c.reports)
            .map(|x| x.text.len())
            .sum();
        println!(
            "{case} children_terminal={} children_total={} reports={n} report_bytes={bytes} bytes_request={} bytes_prompt={}",
            r.metrics.children_terminal,
            r.metrics.children_total,
            json.len(),
            prompt.len()
        );
    }
}

/// D3.1.1: commit した E-mid・D-done の request と prompt に、D の worker の結果 D#1446 の本文が入る（前の入力には無かった）。
#[test]
fn parent_checkpoint_blind_request_has_d1446_body() {
    let all = std::fs::read_to_string(Path::new(SHADOW).join("G-done/events.jsonl")).expect("all");
    let d1446 = all
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|r| r["task"] == "D" && r["seq"] == 1446 && r["type"] == "worker_finished")
        .expect("D#1446");
    let body = d1446["outcome"].as_str().expect("outcome");
    assert!(body.contains("未達"), "D#1446 は未達の申告を含む");
    for (case, _) in CASES {
        let [(_, req), (_, prompt), _] = committed(case);
        let req: Value = serde_json::from_str(&req).expect("request");
        let d = &req["parent_checkpoint"]["children"][3];
        assert!(
            d["title"]
                .as_str()
                .unwrap_or_default()
                .contains("アーキv4 D")
        );
        assert!(
            d["reports"]
                .as_array()
                .is_some_and(|rs| rs.iter().any(|r| r["seq"] == 1446 && r["text"] == body)),
            "{case} の request の D に D#1446 の本文が無い"
        );
        let first_line = body.lines().next().unwrap_or(body);
        assert!(
            prompt.contains(&format!("> {first_line}")),
            "{case} の prompt に D#1446 の本文の引用が無い"
        );
    }
}

/// D8（WU eval-seed）: commit した E-mid・D-done の request と prompt に、承認済み Roadmap の Outcome（goal.md の共通受入条件の文。
/// D6 の到達は偽）と、cutoff 以前に `reviewer_passed` のある子の criteria（全部 pass）が入り、「（Outcome の記録は無い）」が出ない。
/// review の記録の無い子は criteria を持たない（slice に無い条件を作らない）。
#[test]
fn parent_checkpoint_blind_request_has_outcome_and_criteria() {
    for (case, _) in CASES {
        let (rows, goal, _) = slice(case);
        let spec = outcome_spec(&goal);
        assert_eq!(spec.acceptance.len(), 6, "{case}: 共通受入条件の文の数");
        let [(_, req), (_, prompt), _] = committed(case);
        assert!(!prompt.contains(ABSENT), "{case}.prompt.md に {ABSENT}");
        assert!(
            prompt.contains(&format!(
                "- outcome `{OUTCOME_ID}`: {}（reached=false）\n",
                spec.title
            )),
            "{case}.prompt.md に Outcome の行が無い"
        );
        for a in &spec.acceptance {
            assert!(
                goal.contains(a.as_str()),
                "acceptance が goal.md の文でない: {a}"
            );
            assert!(
                prompt.contains(&format!("  - acceptance: {a}\n")),
                "{case}.prompt.md に acceptance が無い: {a}"
            );
        }
        let req: Value = serde_json::from_str(&req).expect("request");
        let pc = &req["parent_checkpoint"];
        let outcomes = pc["goal"]["outcomes"].as_array().expect("outcomes");
        assert_eq!(outcomes.len(), 1, "{case}: Outcome の数");
        assert_eq!(outcomes[0]["id"], OUTCOME_ID);
        assert_eq!(
            outcomes[0]["reached"], false,
            "{case}: cutoff 以前に到達の記録は無い"
        );
        let children = pc["children"].as_array().expect("children");
        for (key, child) in CHILDREN.iter().zip(children) {
            let criteria = child["criteria"].as_array().expect("criteria");
            let passed = rows_of(&rows, key)
                .iter()
                .any(|r| r["type"] == "reviewer_passed");
            assert_eq!(
                !criteria.is_empty(),
                passed,
                "{case} {key}: criteria の有無"
            );
            for c in criteria {
                assert_eq!(c["result"], "pass", "{case} {key}: {c}");
                assert_eq!(c["text"], NO_TEXT);
            }
        }
        let a: Vec<&str> = children[0]["criteria"]
            .as_array()
            .expect("A")
            .iter()
            .filter_map(|c| c["kind"].as_str())
            .collect();
        assert_eq!(
            a,
            [
                "reviewer",
                "reviewer",
                "reviewer",
                "command",
                "artifact_exists"
            ],
            "{case}: A の条件の種類"
        );
        assert!(
            prompt.contains(&format!("  - [command] pass: {NO_TEXT}\n")),
            "{case}.prompt.md に子の criteria の行が無い"
        );
    }
}

/// D8.3: request・prompt・slice（reports.json と slice.json）・ADR・この WU の進捗に answer key の印・理由の文・期待の組が無く、
/// request・prompt・reports に時点より後の seq・時刻・本文と、未来にだけ現れる語（`first_seen` が時点より後の sentinel）が無い。
#[test]
fn parent_checkpoint_blind_no_answer_key_or_future() {
    let key: Value = serde_json::from_str(ANSWER_KEY).expect("key");
    let whys: Vec<&str> = key["checkpoints"]
        .as_object()
        .expect("checkpoints")
        .values()
        .filter_map(|c| c["why"].as_str())
        .collect();
    let verdicts = ["on_track", "at_risk", "off_track", "unknown"];
    let nexts = [
        "continue",
        "discriminating_experiment",
        "pivot",
        "ask_human",
    ];
    let pairs: Vec<String> = verdicts
        .iter()
        .flat_map(|v| nexts.iter().map(move |n| format!("{v}・{n}")))
        .collect();
    let sentinels: Vec<(String, OffsetDateTime)> = serde_json::from_str::<Value>(SENTINELS)
        .expect("sentinels")["sentinels"]
        .as_array()
        .expect("sentinels")
        .iter()
        .map(|s| {
            (
                s["text"].as_str().expect("text").to_string(),
                ts(s["first_seen"].as_str().expect("first_seen")),
            )
        })
        .collect();
    assert!(!sentinels.is_empty());
    let all: Vec<Value> = std::fs::read_to_string(Path::new(SHADOW).join("G-done/events.jsonl"))
        .expect("all")
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    for (case, _) in CASES {
        let (_, _, cutoff) = slice(case);
        let cut = ts(&cutoff);
        let slice_json = std::fs::read_to_string(Path::new(SHADOW).join(case).join("slice.json"))
            .expect("slice");
        let files = committed(case);
        for (name, text) in files
            .iter()
            .chain([&(format!("{case}/slice.json"), slice_json)])
        {
            for mark in ANSWER_MARKS {
                assert!(!text.contains(mark), "{name} に answer key の印 {mark}");
            }
            for why in &whys {
                assert!(!text.contains(why), "{name} に answer key の理由の文");
            }
            for pair in &pairs {
                assert!(!text.contains(pair.as_str()), "{name} に期待の組 {pair}");
            }
        }
        for s in sentinels.iter().filter(|(_, first)| *first > cut) {
            for (name, text) in &files {
                assert!(
                    !text.contains(s.0.as_str()),
                    "{name} に未来にだけ現れる語 {}",
                    s.0
                );
            }
        }
        let (req, _) = request(case, "-");
        for r in req.children.iter().flat_map(|c| &c.reports) {
            assert!(ts(&r.at) <= cut, "{case}: 時点より後の報告 seq {}", r.seq);
        }
        for row in all
            .iter()
            .filter(|r| ts(r["ts"].as_str().unwrap_or_default()) > cut)
        {
            let at = row["ts"].as_str().unwrap_or_default();
            for (name, text) in &files {
                assert!(!text.contains(at), "{name} に時点より後の時刻 {at}");
                if let Some(body) = row["outcome"].as_str().filter(|b| b.len() > 40) {
                    assert!(
                        !text.contains(body),
                        "{name} に時点より後の報告 {}#{}",
                        row["task"],
                        row["seq"]
                    );
                }
            }
        }
    }
    for (name, text) in [("ADR", ADR), ("進捗 blind-eval-input.md", PROGRESS)] {
        assert!(
            !text.contains("answer key の理想"),
            "{name} に answer key の列"
        );
        for pair in &pairs {
            assert!(!text.contains(pair.as_str()), "{name} に期待の組 {pair}");
        }
    }
}

/// D3.2・D4: commit した prompt に出力の欄と axes の定義があり、prompt の例の評価は規則の source を読まずに
/// `check_evaluation` の形の検査を通る。
#[test]
fn parent_checkpoint_blind_prompt_has_output_fields_and_axes() {
    let fields = [
        "celeris.parent-evaluation/1",
        "`schema`",
        "`verdict`",
        "`next`",
        "`axes`",
        "`measured`",
        "`learned`",
        "`refuted`",
        "`uncertainty`",
        "`reason`",
        "`claim`",
        "`metric`",
        "`before`",
        "`after`",
        "`references`",
        "`hypothesis`",
        "`statement`",
        "`resolve_by`",
    ];
    let axes = [
        "`axes.execution`",
        "`axes.epistemic`",
        "`axes.goal`",
        "`succeeded`",
        "`partial`",
        "`failed`",
        "`advanced`",
        "`none`",
        "`met`",
        "`not_met`",
        "終端の子が 0 なら `unknown`",
        "支持された `learned` か `refuted` が 1 件以上あれば `advanced`",
        "`reached` だけが決める",
    ];
    for (case, _) in CASES {
        let [_, (_, prompt), _] = committed(case);
        for f in fields.iter().chain(&axes) {
            assert!(prompt.contains(f), "{case}.prompt.md に {f} が無い");
        }
        let example = prompt
            .lines()
            .find_map(|l| {
                let inner = l.strip_prefix('`')?.strip_suffix('`')?;
                serde_json::from_str::<Value>(inner).ok()
            })
            .expect("prompt の例の JSON");
        let checked = pc::check_evaluation(
            example.get("parent_evaluation"),
            &pc::EvaluationFacts {
                resolved: Vec::new(),
                goal_reached: false,
                children: pc::ChildOutcomes::default(),
            },
        );
        assert_eq!(
            checked.status,
            pc::RecordStatus::Checked,
            "{case}: {:?}",
            checked.violations
        );
    }
}

/// D3.1.1: 本文の長い報告は 1 件の上限で切れ、合計の上限を超えない（E-mid の実データで上限の値を確かめる）。
#[test]
fn parent_checkpoint_blind_reports_capped() {
    for (case, trigger) in CASES {
        let (req, _) = request(case, trigger);
        let mut total = 0;
        for c in &req.children {
            assert!(c.reports.len() <= pc::REPORTS_PER_CHILD_MAX);
            for r in &c.reports {
                assert!(r.text.len() <= pc::REPORT_MAX_BYTES);
                total += r.text.len();
            }
        }
        assert!(total <= pc::REPORTS_MAX_BYTES, "{case}: {total}");
    }
}

/// live-evaluator-sample の評価を検査する（`#[ignore]`: sample の file が無い間は走らせない）。
#[test]
#[ignore = "live-evaluator-sample の E-mid.evaluation.json を検査する（手で走らせる）"]
fn parent_checkpoint_live_sample_checks() {
    let raw: Value = serde_json::from_slice(
        &std::fs::read(Path::new(LIVE).join("E-mid.evaluation.json"))
            .expect("E-mid.evaluation.json"),
    )
    .expect("evaluation json");
    let (rows, _, cutoff) = slice("E-mid");
    let cut = ts(&cutoff);
    let (req, ids) = request("E-mid", "children:2");
    let known: Vec<String> = ids
        .values()
        .map(|id| id.to_string())
        .chain([ROOT_ID.to_string()])
        .collect();
    let refs = pc::references_to_resolve(&raw);
    let mut outside = Vec::new();
    let mut resolved = Vec::new();
    for r in refs {
        let inside = match &r {
            Reference::Task { task_id } => known.contains(task_id),
            Reference::Artifact { name, .. } => rows.iter().any(|row| {
                row["type"] == "artifact_produced"
                    && row.to_string().contains(name.as_str())
                    && ts(row["ts"].as_str().unwrap_or_default()) <= cut
            }),
            Reference::Commit { sha, .. } => rows
                .iter()
                .any(|row| row.to_string().contains(sha.as_str())),
            _ => false,
        };
        if inside {
            resolved.push(r);
        } else {
            outside.push(format!("{r:?}"));
        }
    }
    let children = pc::ChildOutcomes {
        done_passed: req
            .children
            .iter()
            .filter(|c| c.status == Status::Done)
            .count() as u32,
        ..Default::default()
    };
    let checked = pc::check_evaluation(
        Some(&raw),
        &pc::EvaluationFacts {
            resolved,
            goal_reached: false,
            children,
        },
    );
    let key: Value = serde_json::from_str(ANSWER_KEY).expect("key");
    let ideal = &key["checkpoints"]["E-mid"];
    let v = serde_json::to_value(checked.effective.verdict).expect("v");
    let n = serde_json::to_value(checked.effective.next).expect("n");
    println!(
        "sample status={:?} effective={}/{} unsupported={} violations={} refs_outside_slice={outside:?} answer_key={}/{} match={}",
        checked.status,
        v,
        n,
        checked.effective.unsupported,
        checked.violations.len(),
        ideal["ideal_verdict"],
        ideal["ideal_action"],
        v == ideal["ideal_verdict"] && n == ideal["ideal_action"],
    );
    assert_eq!(
        checked.status,
        pc::RecordStatus::Checked,
        "{:?}",
        checked.violations
    );
    assert!(
        outside.is_empty(),
        "slice の外・cutoff の後の参照: {outside:?}"
    );
}

// ---- 本番の起動口での盲検の評価（WU blind-eval-runs。`scripts/dev/parent-eval-blind.sh`） ----

/// `scripts/dev/parent-eval-blind/driver.py` の seed: 使い捨ての一時 DB（`PARENT_EVAL_BLIND_SEED` の spec の `db`）に、時点
/// （`point`）の slice から案件・承認済み Roadmap・strategy root・子を [`seed`] で置く（`#[ignore]`: driver だけが走らせる）。
///
/// - root と子の作業場所は spec の `repo`（slice だけの seed の git repo、mode shared）。
/// - spec の `out` に `{project_id, root_id, children: {letter: id}}` を書く。本番の DB には触れない（spec の db が無い path なら作る）。
#[test]
#[ignore = "scripts/dev/parent-eval-blind/driver.py が一時 DB を seed するときだけ走らせる"]
fn parent_checkpoint_blind_seed() {
    let spec_path = std::env::var("PARENT_EVAL_BLIND_SEED").expect("PARENT_EVAL_BLIND_SEED");
    let spec: Value =
        serde_json::from_slice(&std::fs::read(&spec_path).expect("spec")).expect("spec json");
    let field = |k: &str| spec[k].as_str().expect(k).to_string();
    let (point, repo) = (field("point"), std::path::PathBuf::from(field("repo")));
    assert!(
        CASES.iter().chain(&PILOT_POINTS).any(|(c, _)| *c == point),
        "point {point} は盲検の時点（E-mid・C-mid・D-done・F-done）のどれでもない"
    );
    let store = SqliteStore::open(Path::new(&field("db"))).expect("store");
    let seeded = seed(&store, &point, &repo);
    let out = serde_json::json!({
        "project_id": seeded.project_id.to_string(),
        "root_id": seeded.root_id.to_string(),
        "children": seeded.children.iter().map(|(k, v)| (k.clone(), v.to_string())).collect::<BTreeMap<_, _>>(),
    });
    std::fs::write(field("out"), serde_json::to_vec_pretty(&out).expect("out")).expect("write out");
}

/// `scripts/dev/parent-eval-blind/driver.py` の prompt の写し: fake の harness（stub）は prompt を書かないので、run の dir の
/// `request.json`（`PARENT_EVAL_BLIND_RENDER` の spec の `request`）の `context.review` から、claude-code と同じ本物の関数
/// （`parent_checkpoint_section`・`result_section`）で評価の prompt を描き、spec の `out` に書く（`#[ignore]`: driver だけが走らせる）。
#[test]
#[ignore = "scripts/dev/parent-eval-blind/driver.py が stub の run の prompt を描くときだけ走らせる"]
fn parent_checkpoint_blind_render() {
    let spec_path = std::env::var("PARENT_EVAL_BLIND_RENDER").expect("PARENT_EVAL_BLIND_RENDER");
    let spec: Value =
        serde_json::from_slice(&std::fs::read(&spec_path).expect("spec")).expect("spec json");
    let field = |k: &str| spec[k].as_str().expect(k).to_string();
    let run: Value = serde_json::from_slice(&std::fs::read(field("request")).expect("request"))
        .expect("request json");
    let review: ReviewRequest =
        serde_json::from_value(run["context"]["review"].clone()).expect("context.review");
    assert!(
        review.parent_checkpoint.is_some(),
        "run の request に parent_checkpoint が無い"
    );
    let artifacts = run["artifacts_dir"].as_str().unwrap_or(ARTIFACTS);
    let prompt = task_worker::change_rules::review::parent_checkpoint_section(Some(&review))
        + &task_worker::change_rules::review::result_section(Some(&review), artifacts);
    std::fs::write(field("out"), prompt).expect("write prompt");
}

/// 盲検の評価の出力（`PARENT_CHECKPOINT_BLIND_DIR`、無ければ `fixtures/parent_checkpoint_live/blind/`）の
/// `replay/<時点>/<方式>/`（driver の出力の配置。ADR 2026-11-24-strategy-pilot-ablation）のうち、評価器を起こす方式 b・c の記録を検査する。
///
/// 守る不変条件: 本番の起動口の評価（`output.json` の `record` の最終の記録）が `check_evaluation` を `checked` で通り、全参照が
/// slice の中（その run の案件の root・子の id、cutoff 以前の replay の成果物の name・commit）にある。`output.json` の verdict・next
/// は記録の effective と同じ。commit した既定の dir では stub の出力（`run.json` の `harness` が `stub`）を拒む（LLM の評価だけを
/// 証拠に数える）。記録が 1 件も無ければ落ちる。
/// 既存の試験で検出できない理由: `parent_checkpoint_live_sample_checks` は WU の run が手で書いた E-mid の 1 件だけを見て、本番の起動口
/// （controller の evaluate → `start_side_review`）の出力も D-done も見なかった。
#[test]
#[ignore = "人が流した実 LLM の評価（または PARENT_CHECKPOINT_BLIND_DIR の stub の出力）を検査する（手で走らせる）"]
fn parent_checkpoint_blind_live_checks() {
    let env_dir = std::env::var_os("PARENT_CHECKPOINT_BLIND_DIR");
    let dir = env_dir
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(LIVE).join("blind"));
    let mut seen: Vec<&str> = Vec::new();
    for (case, trigger) in CASES.iter().chain(&PILOT_POINTS) {
        for arm in ["b", "c"] {
            let at = dir.join("replay").join(case).join(arm);
            if seen.contains(case) || !at.join("run.json").is_file() {
                continue;
            }
            blind_live_check(&at, case, trigger, env_dir.is_none());
        }
        seen.push(case);
    }
    let checked = CASES
        .iter()
        .chain(&PILOT_POINTS)
        .flat_map(|(case, _)| ["b", "c"].map(|arm| dir.join("replay").join(case).join(arm)))
        .filter(|at| at.join("run.json").is_file())
        .count();
    assert!(
        checked > 0,
        "{}: replay/<時点>/<b|c>/ の記録が無い",
        dir.display()
    );
}

/// 1 時点・1 方式の記録（`output.json`・`run.json`）を検査する（[`parent_checkpoint_blind_live_checks`]）。
fn blind_live_check(at: &Path, case: &str, trigger: &str, committed: bool) {
    {
        let read = |name: &str| -> Value {
            let p = at.join(name);
            serde_json::from_slice(
                &std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
            )
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        };
        let (output, run) = (read("output.json"), read("run.json"));
        let record = output["record"].clone();
        assert_eq!(
            (&output["verdict"], &output["next"]),
            (
                &record["effective"]["verdict"],
                &record["effective"]["next"]
            ),
            "{case}: output.json の verdict・next が記録の effective と違う"
        );
        if committed {
            assert_ne!(
                run["harness"], "stub",
                "{case}: 既定の dir に stub の出力がある（LLM の評価だけを置く）"
            );
        }
        assert_eq!(
            record["schema"],
            pc::RECORD_SCHEMA,
            "{case}: 最終の記録でない"
        );
        let raw = record["evaluation"].clone();
        let (rows, _, cutoff) = slice(case);
        let cut = ts(&cutoff);
        let before: Vec<&Value> = rows
            .iter()
            .filter(|r| ts(r["ts"].as_str().unwrap_or_default()) <= cut)
            .collect();
        let known: Vec<String> = run["children"]
            .as_object()
            .map(|m| {
                m.values()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .chain(run["root_id"].as_str().map(String::from))
            .collect();
        assert!(known.len() > 1, "{case}: run.json に root と子の id が無い");
        let mut outside = Vec::new();
        let mut resolved = Vec::new();
        for r in pc::references_to_resolve(&raw) {
            let inside = match &r {
                Reference::Task { task_id } => known.contains(task_id),
                Reference::Artifact { name, .. } => before.iter().any(|row| {
                    row["type"] == "artifact_produced" && row.to_string().contains(name.as_str())
                }),
                Reference::Commit { sha, .. } => before
                    .iter()
                    .any(|row| sha.len() >= 7 && row.to_string().contains(sha.as_str())),
                _ => false,
            };
            if inside {
                resolved.push(r);
            } else {
                outside.push(format!("{r:?}"));
            }
        }
        let (req, _) = request(case, trigger);
        let children = pc::ChildOutcomes {
            done_passed: req
                .children
                .iter()
                .filter(|c| c.status == Status::Done)
                .count() as u32,
            ..Default::default()
        };
        let checked = pc::check_evaluation(
            Some(&raw),
            &pc::EvaluationFacts {
                resolved,
                goal_reached: false,
                children,
            },
        );
        println!(
            "{case} harness={} status={:?} effective={}/{} record={}/{} violations={} refs_outside_slice={outside:?}",
            run["harness"],
            checked.status,
            serde_json::to_value(checked.effective.verdict).expect("v"),
            serde_json::to_value(checked.effective.next).expect("n"),
            record["effective"]["verdict"],
            record["effective"]["next"],
            checked.violations.len(),
        );
        assert_eq!(
            record["status"], "checked",
            "{case}: 本番の起動口の記録が checked でない"
        );
        assert_eq!(
            checked.status,
            pc::RecordStatus::Checked,
            "{case}: {:?}",
            checked.violations
        );
        assert!(
            outside.is_empty(),
            "{case}: slice の外・cutoff の後の参照: {outside:?}"
        );
    }
}
