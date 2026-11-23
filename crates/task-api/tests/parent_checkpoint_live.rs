//! ADR 2026-11-22-parent-checkpoint-evaluator D8.3: live の sample（接頭辞 `parent_checkpoint_live_`）。
//!
//! - `parent_checkpoint_live_request_renders`: E-mid の slice（`tests/fixtures/parent_checkpoint_shadow/` の写し。
//!   manifest の sha256 と cutoff を確かめる）から `ParentCheckpointRequest` を作り、task-worker の本物の型と prompt の節
//!   （`change_rules::review::parent_checkpoint_section`）で request と prompt を出し、commit した
//!   `fixtures/parent_checkpoint_live/E-mid.request.json`・`E-mid.prompt.md` と byte 単位で一致することを確かめる。
//!   request の欄は dispatcher の `review::parent_evaluation::request`（task-api から呼べない private の関数）と同じ出どころ
//!   （子の title・cutoff の時点の status・reviewer の合格・終端の数、outcomes と criteria は空、固定の rubric）で、id は
//!   fixture で固定の値を使う。`PARENT_CHECKPOINT_LIVE_WRITE=1` のときだけ 2 file を書き直す。
//! - `parent_checkpoint_live_sample_checks`（`#[ignore]`）: live-evaluator-sample の WU が書く `E-mid.evaluation.json` を
//!   `check_evaluation` に通し、形・規則・全参照が slice の中で cutoff 以前かを検査し、answer key との一致を出力する。
//!
//! 守る不変条件: 評価器の入力（request・prompt）に answer key の印と cutoff より後の記録が入らない。
//! 既存の試験で検出できない理由: task-worker の `parent_evaluation_wire_` は合成の request で wire だけを見て、
//! E-mid の実際の slice から作った入力に answer key や未来の記録が混ざらないことを見ない。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};
use task_core::parent_checkpoint as pc;
use task_core::strategy::Reference;
use task_core::strategy_ref::StrategyRef;
use task_core::{Status, TaskId};
use task_worker::protocol::{
    ParentCheckpointChild, ParentCheckpointGoal, ParentCheckpointMetrics, ParentCheckpointRequest,
    ReviewRequest,
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
const ANSWER_MARKS: [&str; 3] = ["ideal_verdict", "ideal_action", "answer_key"];
const CHILDREN: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];
/// fixture の固定の id（root はアーキv4 の親の実 id、子は letter の順の固定 ULID）。
const ROOT_ID: &str = "01M7MZT8KX547464YYS76QNP4G";
const DECISION_ID: &str = "e-mid-sample";

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

/// E-mid の slice: cutoff 以下の events（G-done を cutoff で切る。manifest の sha256 と一致を確かめる）と goal.md。
fn e_mid() -> (Vec<Value>, String, OffsetDateTime) {
    let slice: Value = serde_json::from_slice(
        &std::fs::read(Path::new(SHADOW).join("E-mid/slice.json")).expect("slice"),
    )
    .expect("slice json");
    let cut = ts(slice["cutoff"].as_str().expect("cutoff"));
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
        manifest_sha("E-mid/events.jsonl"),
        Some(sha_hex(text.as_bytes()).as_str()),
        "E-mid の events が子 0 の slice と違う"
    );
    let goal = std::fs::read_to_string(Path::new(SHADOW).join("E-mid/goal.md")).expect("goal");
    assert_eq!(
        manifest_sha("E-mid/goal.md"),
        Some(sha_hex(goal.as_bytes()).as_str())
    );
    (rows, goal, cut)
}

/// D3.1 の入力を E-mid の slice から作る（dispatcher の `request` と同じ出どころ）。
fn request() -> (ParentCheckpointRequest, BTreeMap<String, TaskId>) {
    let (rows, goal, _) = e_mid();
    let ids: BTreeMap<String, TaskId> = CHILDREN
        .iter()
        .enumerate()
        .map(|(i, k)| (k.to_string(), child_id(i + 1)))
        .collect();
    let mut children = Vec::new();
    for key in CHILDREN {
        let mine = || rows.iter().filter(|r| r["task"] == key);
        let Some(title) = mine().find_map(|r| r["title"].as_str()) else {
            continue;
        };
        let status = mine()
            .filter(|r| r["type"] == "transitioned")
            .filter_map(|r| serde_json::from_value::<Status>(r["to"].clone()).ok())
            .last()
            .unwrap_or(Status::Draft);
        children.push(ParentCheckpointChild {
            task_id: ids[key],
            title: title.to_string(),
            status,
            reviewer_passed: mine().any(|r| r["type"] == "reviewer_passed"),
            criteria: Vec::new(),
        });
    }
    let metrics = ParentCheckpointMetrics {
        children_terminal: children.iter().filter(|c| c.status.is_terminal()).count() as u32,
        children_total: children.len() as u32,
        tokens_used: 0,
        max_tokens: None,
    };
    let req = ParentCheckpointRequest {
        decision_id: DECISION_ID.into(),
        root_id: ROOT_ID.parse().expect("root id"),
        triggers: vec!["children:2".into()],
        goal: ParentCheckpointGoal {
            request_sha256: format!("sha256:{}", sha_hex(goal.as_bytes())),
            outcomes: Vec::new(),
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

fn rendered() -> (String, String) {
    let (req, _) = request();
    let review = ReviewRequest {
        parent_checkpoint: Some(Box::new(req.clone())),
        ..ReviewRequest::default()
    };
    let json = serde_json::to_string_pretty(&review).expect("request json") + "\n";
    let prompt = task_worker::change_rules::review::parent_checkpoint_section(Some(&review));
    (json, prompt)
}

#[test]
fn parent_checkpoint_live_request_renders() {
    let (json, prompt) = rendered();
    let req_path = Path::new(LIVE).join("E-mid.request.json");
    let prompt_path = Path::new(LIVE).join("E-mid.prompt.md");
    if std::env::var_os("PARENT_CHECKPOINT_LIVE_WRITE").is_some() {
        std::fs::create_dir_all(LIVE).expect("dir");
        std::fs::write(&req_path, &json).expect("write request");
        std::fs::write(&prompt_path, &prompt).expect("write prompt");
    }
    assert!(!prompt.is_empty(), "節のある review は prompt に節を出す");
    for text in [&json, &prompt] {
        for mark in ANSWER_MARKS {
            assert!(
                !text.contains(mark),
                "評価器の入力に answer key の印 {mark}"
            );
        }
        // answer key の E-mid の理由の文も入っていない。
        let key: Value = serde_json::from_str(ANSWER_KEY).expect("key");
        let why = key["checkpoints"]["E-mid"]["why"].as_str().unwrap_or("-");
        assert!(!text.contains(why));
    }
    let committed_req = std::fs::read_to_string(&req_path).expect("committed request");
    let committed_prompt = std::fs::read_to_string(&prompt_path).expect("committed prompt");
    assert_eq!(
        json, committed_req,
        "E-mid.request.json が本物の型の出力と違う"
    );
    assert_eq!(
        prompt, committed_prompt,
        "E-mid.prompt.md が本物の prompt と違う"
    );
    let (req, _) = request();
    println!(
        "e_mid_request children_terminal={} children_total={} bytes_request={} bytes_prompt={}",
        req.metrics.children_terminal,
        req.metrics.children_total,
        json.len(),
        prompt.len()
    );
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
    let (rows, _, cut) = e_mid();
    let (req, ids) = request();
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
