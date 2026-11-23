//! ADR 2026-11-22-parent-checkpoint-evaluator D8: 第二ゲートの shadow ケース（接頭辞 `parent_checkpoint_shadow_`）。
//!
//! 子 0 の `replay.py build` の slice（アーキv4 の親 P と子 A〜H の events）を写した fixture
//! （`tests/fixtures/parent_checkpoint_shadow/`）を、時点ごとに一時 DB の store へ流し、本物の
//! snapshot（`task_ops::project_snapshot::build`・`checkpoints`）→ proposal → 本物の Rust の検証
//! （`project_proposal::parse_and_validate`）→ 効果（shadow は `suppressed`）を通す。
//!
//! - task-api は task-dispatch・TS の host に依存しないので、TS の規則 `pc/checkpoint/evaluate` は D5.1 の 1 のとおりに
//!   この file の `rule` が書く（TS の規則そのものは `control-ts` の `project_controller_checkpoint_` と celeris の e2e が確かめる）。
//!   判断の記録と revision は harness が持つ（`crates/celeris/tests/project_controller_e2e.rs` と同じ形）。
//! - LLM を呼ばない。外部ネットワークに出ない。本番の DB に触れない。
//! - answer key（`answer_key.json`）は fixture に写さない。比べる側だけが子 0 の `replay/` から読む。
//!
//! 守る不変条件と、既存の試験で検出できない理由:
//! - fixture の各 file は子 0 の `manifest.sha256` と byte 単位で同じで、各時点の event は cutoff 以下だけ。
//! - N = 2 の既定で、アーキv4 の replay の 12 時点のうち C-mid・D-done・F-done の 3 点だけで `children:<k>` が発火し、
//!   shadow の `suppressed` の判断は発火済みに数えられて同じ trigger が 2 回出ない。
//!   task-core の `parent_checkpoint_contract_` は純関数に手で事実を与え、task-dispatch の `project_controller_checkpoint_` は
//!   合成の子 2 件だけを見る。どちらも本物の replay の時系列（`review_fail → ready` の再試行・孫 task の終端を含む）を流さない。

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::chat::{AuditContext, ChatActor};
use task_core::parent_checkpoint as pc;
use task_core::project_proposal::{
    self as pp, ProjectAction, ProjectProposal, ProjectSnapshot, SnapshotController,
    SnapshotDecision, SnapshotLimits,
};
use task_core::{Event, ProjectId, Status, Task, TaskId, TaskKind};
use task_ops::project_snapshot::{self as ps, CheckpointDecisionRow, SnapshotContext};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/parent_checkpoint_shadow"
);
const MANIFEST: &str =
    include_str!("../../../agent-docs/progress/2026-11-21-strategy-v1-0/replay/manifest.sha256");
/// 比べる側だけが読む answer key（fixture にも評価器の入力にも入れない）。
const ANSWER_KEY: &str =
    include_str!("../../../agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json");
/// 12 時点（cutoff の順）。D8.1 の表の順。
const SLICES: [&str; 12] = [
    "A-mid", "A-done", "C-mid", "C-done", "D-mid", "E-mid", "D-done", "E-done", "F-mid", "F-done",
    "G-mid", "G-done",
];
/// 全時点の events の親集合（G-done の slice）。各時点の events はこれを cutoff で切ったもの。
const ALL_EVENTS: &str = "G-done/events.jsonl";
/// アーキv4 の親の直下の子。
const CHILDREN: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];
/// 史実の親の最初の評価（D8.1）。
const HISTORICAL_FIRST_EVALUATION: &str = "2026-11-20T14:12:10Z";
const STRATEGY_ID: &str = "arch-v4-cycle";
const STRATEGY_NAME: &str = "strategy/arch-v4-cycle/v1.json";
/// 子 2 の戦略の文書の fixture（反証の trigger の合成に使う）。
const DOC_V1: &str = include_str!("../../task-core/src/strategy/fixtures/baseline-v1.json");
/// 評価器の入力に入ってはならない answer key の印（子 0 の否定対照 N3 と同じ）。
const ANSWER_MARKS: [&str; 3] = ["ideal_verdict", "ideal_action", "answer_key"];

const REQUEST: &str = "アーキv4 の親の目的（shadow）: 責務境界と疎結合性を実測で改善する。\n\
```celeris-outcomes\n\
[{\"id\":\"o1\",\"title\":\"循環の削減\",\"acceptance\":[\"既存の module 循環の数が base より減る\"]}]\n\
```\n";

// ---- fixture ----

fn manifest_sha(path: &str) -> Option<&'static str> {
    MANIFEST.lines().find_map(|line| {
        let (sha, p) = line.split_once("  ")?;
        (p == path).then_some(sha)
    })
}

fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn ts(raw: &str) -> OffsetDateTime {
    OffsetDateTime::parse(raw, &Rfc3339).expect("rfc3339")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(FIXTURE).join(rel)).expect("fixture file")
}

fn slice_json(slice: &str) -> Value {
    serde_json::from_str(&read(&format!("{slice}/slice.json"))).expect("slice.json")
}

fn cutoff(slice: &str) -> OffsetDateTime {
    ts(slice_json(slice)["cutoff"].as_str().expect("cutoff"))
}

/// cutoff 以下の行（子 0 の slice の events.jsonl と byte 単位で同じになる）。
fn events_until(cut: OffsetDateTime) -> String {
    let mut out = String::new();
    for line in read(ALL_EVENTS).lines().filter(|l| !l.trim().is_empty()) {
        let row: Value = serde_json::from_str(line).expect("event row");
        if ts(row["ts"].as_str().expect("ts")) <= cut {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// fixture が子 0 の slice の写しでない理由（空なら使える）。
fn fixture_problems() -> Vec<String> {
    let mut out = Vec::new();
    for slice in SLICES {
        let rel = format!("{slice}/slice.json");
        let bytes = std::fs::read(Path::new(FIXTURE).join(&rel)).unwrap_or_default();
        if manifest_sha(&rel) != Some(sha_hex(&bytes).as_str()) {
            out.push(format!("{rel}: sha256 が manifest と違う"));
        }
        let derived = events_until(cutoff(slice));
        let rel_events = format!("{slice}/events.jsonl");
        if manifest_sha(&rel_events) != Some(sha_hex(derived.as_bytes()).as_str()) {
            out.push(format!(
                "{rel_events}: cutoff で切った events が manifest と違う"
            ));
        }
        let n = derived.lines().count() as u64;
        if slice_json(slice)["events"].as_u64() != Some(n) {
            out.push(format!("{slice}: event の数 {n} が slice.json と違う"));
        }
    }
    let goal = std::fs::read(Path::new(FIXTURE).join("E-mid/goal.md")).unwrap_or_default();
    if manifest_sha("E-mid/goal.md") != Some(sha_hex(&goal).as_str()) {
        out.push("E-mid/goal.md: sha256 が manifest と違う".into());
    }
    out
}

// ---- harness ----

/// アーキv4 の親を strategy root とみなした案件（一時 DB）。判断の記録と revision は harness が持つ。
struct Shadow {
    env: TestEnv,
    project: ProjectId,
    root: Task,
    children: BTreeMap<String, Task>,
    controller: SnapshotController,
    decisions: Vec<(SnapshotDecision, ProjectProposal)>,
    settings: pc::CheckpointSettings,
    /// 流した events の数（G-done の行の先頭からの位置）。
    applied: usize,
    /// root の木の使った token（予算の trigger の合成）。
    tokens_used: u64,
    /// 評価器（スクリプトの評価）を起こした回数 = 評価の reviewer run の数。
    evaluator_runs: u32,
}

impl Shadow {
    fn new(mode: &str) -> Self {
        Self::with_budget(mode, None)
    }

    fn with_budget(mode: &str, max_tokens: Option<u64>) -> Self {
        let env = admin_env();
        let now = OffsetDateTime::now_utc();
        let project = task_core::Project {
            id: ProjectId::new(),
            title: "shadow:arch-v4".into(),
            request: REQUEST.into(),
            status: task_core::ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            slug: None,
            created_at: now,
            updated_at: now,
        };
        env.store.project_create(&project).expect("project");
        let mut root = new_task(TaskKind::Execute, Status::Running);
        root.project_id = Some(project.id);
        root.title = "[アーキv4 親]（shadow）".into();
        root.budget.max_tokens = max_tokens;
        env.store.create_task(&root, vec![]).expect("root");
        let mut children = BTreeMap::new();
        for key in CHILDREN {
            let mut t = new_task(TaskKind::Execute, Status::Ready);
            t.project_id = Some(project.id);
            t.parent_id = Some(root.id);
            t.title = format!("[アーキv4 {key}]");
            env.store.create_task(&t, vec![]).expect("child");
            children.insert(key.to_string(), t);
        }
        strategy_op(&env, &root);
        Self {
            env,
            project: project.id,
            root,
            children,
            controller: SnapshotController {
                mode: mode.into(),
                phase: "active".into(),
                revision: 0,
                human_epoch: 0,
                retry_count: 0,
                progress_epoch: 0,
            },
            decisions: Vec::new(),
            settings: pc::CheckpointSettings::default(),
            applied: 0,
            tokens_used: 0,
            evaluator_runs: 0,
        }
    }

    /// cutoff までの子 A〜H の遷移を store に流す（孫 `sub:` と親 P の行は root の直下でないので流さない）。
    fn advance_to(&mut self, cut: OffsetDateTime) {
        let all = read(ALL_EVENTS);
        let lines: Vec<&str> = all.lines().filter(|l| !l.trim().is_empty()).collect();
        while self.applied < lines.len() {
            let row: Value = serde_json::from_str(lines[self.applied]).expect("row");
            if ts(row["ts"].as_str().expect("ts")) > cut {
                break;
            }
            self.applied += 1;
            let Some(task) = row["task"].as_str().and_then(|k| self.children.get(k)) else {
                continue;
            };
            if row["type"] != "transitioned" {
                continue;
            }
            let status =
                |v: &Value| -> Status { serde_json::from_value(v.clone()).expect("status") };
            self.env
                .store
                .append_event(
                    task.id,
                    &Event::Transitioned {
                        from: status(&row["from"]),
                        to: status(&row["to"]),
                        reason: row["reason"].as_str().unwrap_or_default().to_string(),
                    },
                )
                .expect("append");
        }
    }

    fn snapshot(&self) -> ProjectSnapshot {
        let mut s = ps::build(
            &self.env.store,
            self.project,
            SnapshotContext {
                controller: self.controller.clone(),
                decisions: self.decisions.iter().map(|(d, _)| d.clone()).collect(),
                wakes: Vec::new(),
                limits: SnapshotLimits::default(),
                now: OffsetDateTime::now_utc(),
            },
        )
        .expect("snapshot");
        let rows: Vec<CheckpointDecisionRow<'_>> = self
            .decisions
            .iter()
            .map(|(d, p)| CheckpointDecisionRow {
                id: &d.id,
                effect_state: &d.effect_state,
                proposal: p,
            })
            .collect();
        s.checkpoints = ps::checkpoints(
            &self.env.store,
            &self.env.view_context().workspace_roots,
            &s.roots,
            &rows,
            &self.settings,
            &mut |_| Ok(self.tokens_used),
        )
        .expect("checkpoints");
        s
    }

    /// 検証して記録する。shadow の `evaluate` は `suppressed`（D9.1）。`Err` は検証の違反の code。
    fn apply(&mut self, s: &ProjectSnapshot, raw: &Value) -> Result<String, Vec<String>> {
        let p = pp::parse_and_validate(s, raw)
            .map_err(|v| v.into_iter().map(|v| v.code).collect::<Vec<_>>())?;
        let effect = match (&p.action, self.controller.mode.as_str()) {
            (ProjectAction::Evaluate { .. }, "shadow") => "suppressed",
            (ProjectAction::Wait { .. } | ProjectAction::Continue { .. }, _) => "none",
            _ => "applied",
        };
        let id = format!("dec-{:03}", self.decisions.len() + 1);
        let d = SnapshotDecision {
            id: id.clone(),
            kind: p.action.kind().to_owned(),
            rule_id: p.explanation.rule_id.clone(),
            effect_state: effect.into(),
            root_key: None,
            reason_code: None,
            outcome_id: None,
            request_sha256: s.project.request_sha256.clone(),
            progress_epoch: p.gap.progress_epoch,
            at: s.now.clone(),
        };
        self.decisions.push((d, p));
        self.controller.revision += 1;
        Ok(id)
    }

    fn evaluate_count(&self) -> usize {
        self.decisions
            .iter()
            .filter(|(d, _)| d.kind == "evaluate")
            .count()
    }
}

/// strategy root の正本（D1.1）: 戦略の文書を参照する `task.create` の CoS 操作。
fn strategy_op(env: &TestEnv, root: &Task) {
    let op = ulid::Ulid::new().to_string();
    let ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: "thread".into(),
        run_id: "cos-run".into(),
        operation_id: op.clone(),
        reason: "second-gate shadow".into(),
        policy_version: "v1".into(),
    };
    let payload = json!({
        "method": "POST", "path": "/api/v1/tasks", "body": {"title": "root"},
        "strategy": {"task_id": root.id.to_string(), "name": STRATEGY_NAME,
                     "sha256": format!("sha256:{}", "a".repeat(64)), "candidate": null}
    });
    env.store
        .cos_operation_apply_at(
            &ctx,
            &op,
            "hash",
            "task",
            &root.id.to_string(),
            None,
            "task.create",
            &payload,
            OffsetDateTime::now_utc(),
            |_, _| Ok(json!({})),
        )
        .expect("cos operation");
}

fn proposal(s: &ProjectSnapshot, action: Value, rule: &str) -> Value {
    proposal_with(s, action, rule, &[])
}

fn proposal_with(
    s: &ProjectSnapshot,
    action: Value,
    rule: &str,
    signals: &[(&str, &str)],
) -> Value {
    json!({
        "action": action,
        "gap": {"reached": [], "open": s.outcomes.iter().map(|o| o.id.clone()).collect::<Vec<_>>(),
                "unknown": [], "progress_epoch": s.controller.progress_epoch},
        "explanation": {"rule_id": rule, "signals": signals.iter().map(|(n, d)| json!({"name": n, "detail": d})).collect::<Vec<_>>()},
        "based_on": {"revision": s.controller.revision, "human_epoch": s.controller.human_epoch,
                     "request_sha256": s.project.request_sha256},
    })
}

/// D5.1 の 1（`pc/checkpoint/evaluate`）: due の trigger があり実行中の判断の無い strategy root を 1 つ評価に出す。
fn rule(s: &ProjectSnapshot) -> Option<Value> {
    let c = s
        .checkpoints
        .iter()
        .find(|c| c.running.is_none() && !c.due.is_empty())?;
    let triggers: Vec<&str> = c.due.iter().map(|t| t.key.as_str()).collect();
    Some(proposal(
        s,
        json!({"kind": "evaluate", "root_id": c.root_id, "triggers": triggers}),
        "pc/checkpoint/evaluate",
    ))
}

fn answer_key() -> Value {
    serde_json::from_str(ANSWER_KEY).expect("answer key")
}

// ---- 試験 ----

#[test]
fn parent_checkpoint_shadow_fixture_matches_manifest() {
    let problems = fixture_problems();
    assert!(problems.is_empty(), "{problems:#?}");
    // fixture に answer key の file も印も無い。
    for entry in walk(Path::new(FIXTURE)) {
        let name = entry.to_string_lossy().into_owned();
        assert!(
            !name.contains("answer_key") && !name.contains("sentinels"),
            "{name}"
        );
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        for mark in ANSWER_MARKS {
            assert!(!text.contains(mark), "{name} に {mark}");
        }
    }
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).expect("dir").filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// D8.1: 12 時点で checkpoint が C-mid・D-done・F-done だけで発火し、shadow の `suppressed` の判断で同じ trigger が
/// 2 回出ない。発火の時点と、史実の親の最初の評価との差を出力する。answer key との比較は出力だけ（合否は人の rubric）。
#[test]
fn parent_checkpoint_shadow_fires_at_c_mid_d_done_f_done() {
    assert!(fixture_problems().is_empty(), "{:#?}", fixture_problems());
    let key = answer_key();
    let mut h = Shadow::new("shadow");
    let mut fired = Vec::new();
    let historical = ts(HISTORICAL_FIRST_EVALUATION);
    for slice in SLICES {
        let cut = cutoff(slice);
        h.advance_to(cut);
        let s = h.snapshot();
        assert_eq!(s.checkpoints.len(), 1, "{slice}: strategy root は 1 つ");
        let cp = &s.checkpoints[0];
        assert_eq!(cp.root_id, h.root.id.to_string());
        let due: Vec<String> = cp.due.iter().map(|t| t.key.clone()).collect();
        match rule(&s) {
            Some(raw) => {
                h.apply(&s, &raw)
                    .unwrap_or_else(|v| panic!("{slice}: {v:?}"));
                let ideal = &key["checkpoints"][slice];
                let hours = (historical - cut).as_seconds_f64() / 3600.0;
                println!(
                    "fire slice={slice} cutoff={} triggers={due:?} effect=suppressed \
                     earlier_than_historical_h={hours:.1} answer_key={}/{}",
                    slice_json(slice)["cutoff"].as_str().unwrap_or_default(),
                    ideal["ideal_verdict"].as_str().unwrap_or("-"),
                    ideal["ideal_action"].as_str().unwrap_or("-"),
                );
                fired.push((slice, due));
            }
            None => println!("no-fire slice={slice}"),
        }
        // 同じ時点でもう一度判断しても、suppressed の判断が発火済みに数えられて 2 回目は出ない。
        assert!(rule(&h.snapshot()).is_none(), "{slice}: 2 回目の発火");
    }
    let want = vec![
        ("C-mid", vec!["children:2".to_string()]),
        ("D-done", vec!["children:4".to_string()]),
        ("F-done", vec!["children:6".to_string()]),
    ];
    assert_eq!(fired, want);
    assert_eq!(h.evaluate_count(), 3);
    // D8.1: 最初の off_track の checkpoint（D-done）は史実の最初の評価より約 41.5 時間早い。
    let lead = (historical - cutoff("D-done")).as_seconds_f64() / 3600.0;
    println!("d_done_lead_hours={lead:.2}");
    assert!((41.0..42.0).contains(&lead), "{lead}");
}

/// D9.1: shadow の案件の `evaluate` は `suppressed` で、評価の記録（`parent-evaluation/`）は root に 1 件も書かれない。
#[test]
fn parent_checkpoint_shadow_suppressed_writes_no_evaluation() {
    let mut h = Shadow::new("shadow");
    h.advance_to(cutoff("G-done"));
    let s = h.snapshot();
    let raw = rule(&s).expect("due at G-done");
    h.apply(&s, &raw).expect("valid");
    let artifacts = h
        .env
        .store
        .events_for(h.root.id)
        .expect("events")
        .into_iter()
        .filter(|e| {
            matches!(e, (_, Event::ArtifactProduced { artifact, .. })
            if artifact.name.starts_with(pc::EVALUATION_ARTIFACT_PREFIX))
        })
        .count();
    assert_eq!(artifacts, 0);
    assert_eq!(h.decisions[0].0.effect_state, "suppressed");
    let _: TaskId = h.root.id;
}

// ---- enabled の流れ（評価器はスクリプトの出力。LLM ではない） ----

fn scripted(name: &str, h: &Shadow) -> Value {
    let all: Value =
        serde_json::from_str(&read("scripted-evaluations.json")).expect("scripted evaluations");
    let mut text = all[name].to_string();
    for (k, t) in &h.children {
        text = text.replace(&format!("@{k}"), &t.id.to_string());
    }
    serde_json::from_str(&text).expect("scripted")
}

/// D5.1 の 2（`pc/checkpoint/act`）: 写していない評価があれば、D5.2 の表の行動で写す。
fn act(s: &ProjectSnapshot) -> Option<Value> {
    let (cp, e) = s
        .checkpoints
        .iter()
        .find_map(|c| c.unmapped.first().map(|e| (c, e)))?;
    let want = match (e.status.as_str(), e.effective["next"].as_str()) {
        ("checked", Some("continue")) => "continue",
        ("checked", Some("discriminating_experiment" | "pivot")) => "replan",
        _ => "escalate",
    };
    let a = e.allowed.iter().find(|a| a.kind == want)?;
    let action = match want {
        "continue" => {
            json!({"kind": "continue", "watch_root_ids": [cp.root_id], "next_wake_after_secs": 3600})
        }
        "replan" => json!({"kind": "replan", "reason_code": a.reason_code,
            "question": "checkpoint の評価に沿って戦略の次の版を作る",
            "context_refs": [format!("task:{}", cp.root_id), e.record_ref, cp.strategy_ref]}),
        _ => {
            json!({"kind": "escalate", "category": "explicit_human", "question_key": a.question_key,
            "summary": "checkpoint の評価が人の判断を求める", "options": [], "evidence_refs": []})
        }
    };
    Some(proposal_with(
        s,
        action,
        pp::CHECKPOINT_ACT_RULE,
        &[(pp::CHECKPOINT_SIGNAL, &e.decision_id)],
    ))
}

fn guard_escalate(s: &ProjectSnapshot) -> Value {
    let key = format!("no_progress:{}", s.controller.progress_epoch);
    proposal(
        s,
        json!({"kind": "escalate", "category": "explicit_human", "question_key": key,
               "summary": "評価の後も進まない", "options": [], "evidence_refs": []}),
        "pc/no-progress",
    )
}

fn launch_root(s: &ProjectSnapshot) -> Value {
    proposal(
        s,
        json!({"kind": "launch_root", "root": {
            "key": "outcome:o1:1", "title": "循環の削減", "objective": "途中目標 o1 を達成する",
            "acceptance": [{"text": "既存の module 循環の数が base より減る", "check": {"type": "reviewer"}}],
            "stages_hint": [], "continues_root_ids": [], "outcome_ids": ["o1"]}}),
        "pc/outcome-open",
    )
}

/// 1 回の判断の記録（表に出す行）。
#[derive(Debug)]
struct Step {
    slice: &'static str,
    kind: String,
    rule: String,
    detail: String,
    result: Result<String, Vec<String>>,
}

impl Shadow {
    fn last_status(&self, task: TaskId) -> Option<Status> {
        self.env
            .store
            .events_for(task)
            .expect("events")
            .into_iter()
            .filter_map(|(_, e)| match e {
                Event::Transitioned { to, .. } => Some(to),
                _ => None,
            })
            .last()
    }

    /// D4 の事実（dispatcher の `settle` と同じ意味）: 参照の解決は同じ案件の task の実在、子は終端の数。
    fn facts(&self, raw: &Value) -> pc::EvaluationFacts {
        let resolved = pc::references_to_resolve(raw)
            .into_iter()
            .filter(|r| match r {
                task_core::strategy::Reference::Task { task_id } => task_id
                    .parse::<TaskId>()
                    .ok()
                    .and_then(|id| self.env.store.get(id).ok().flatten())
                    .is_some_and(|t| t.project_id == Some(self.project)),
                _ => false,
            })
            .collect();
        let mut children = pc::ChildOutcomes::default();
        for t in self.children.values() {
            match self.last_status(t.id) {
                Some(Status::Done) => children.done_passed += 1,
                Some(Status::Failed) => children.failed += 1,
                Some(Status::Cancelled) => children.cancelled += 1,
                _ => {}
            }
        }
        pc::EvaluationFacts {
            resolved,
            goal_reached: false,
            children,
        }
    }

    /// 評価の reviewer run の代わり: スクリプトの出力を `check_evaluation` に通し、最終の記録を root の成果物に書く（D3.4）。
    fn run_evaluator(
        &mut self,
        decision_id: &str,
        evaluation: Value,
    ) -> pc::ParentEvaluationRecord {
        self.evaluator_runs += 1;
        let checked = pc::check_evaluation(Some(&evaluation), &self.facts(&evaluation));
        let record = pc::ParentEvaluationRecord::new(
            decision_id,
            &self.root.id.to_string(),
            Some(evaluation),
            &checked,
        );
        let bytes = serde_json::to_vec(&record).expect("record");
        let name = pc::evaluation_artifact_name(decision_id);
        let path = self.env.dir.path().join(name.replace('/', "_"));
        std::fs::write(&path, &bytes).expect("write record");
        self.env
            .store
            .append_event(
                self.root.id,
                &Event::ArtifactProduced {
                    run_id: format!("eval-{}", self.evaluator_runs),
                    artifact: task_core::model::ArtifactRef {
                        name,
                        path: path.to_string_lossy().into_owned(),
                        sha256: sha_hex(&bytes),
                        kind: pc::EVALUATION_ARTIFACT_KIND.into(),
                        declared: false,
                    },
                },
            )
            .expect("artifact");
        record
    }

    /// 1 回の wake: snapshot → 規則（act → evaluate）→ 検証 → 効果。enabled の `evaluate` はその場で評価器を 1 回起こす。
    fn tick(&mut self, slice: &'static str, script: Option<&str>) -> Option<Step> {
        let s = self.snapshot();
        let raw = act(&s).or_else(|| rule(&s))?;
        let kind = raw["action"]["kind"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let rule_id = raw["explanation"]["rule_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let detail = match kind.as_str() {
            "evaluate" => raw["action"]["triggers"].to_string(),
            "replan" => raw["action"]["reason_code"].to_string(),
            "escalate" => raw["action"]["question_key"].to_string(),
            _ => String::new(),
        };
        let result = self.apply(&s, &raw);
        if let (Ok(id), "evaluate", "enabled") =
            (&result, kind.as_str(), self.controller.mode.as_str())
            && let Some(name) = script
        {
            let ev = scripted(name, self);
            self.run_evaluator(&id.clone(), ev);
        }
        Some(Step {
            slice,
            kind,
            rule: rule_id,
            detail,
            result,
        })
    }

    fn task_count(&self) -> usize {
        self.env.store.list(None).expect("list").len()
    }
}

/// enabled の案件で replay の 12 時点を流す。`script` は発火した時点の評価器の出力の名前。
fn run_enabled_replay(h: &mut Shadow) -> Vec<Step> {
    let mut steps = Vec::new();
    for slice in SLICES {
        h.advance_to(cutoff(slice));
        let script = matches!(slice, "C-mid" | "D-done" | "F-done").then_some(slice);
        for _ in 0..4 {
            let Some(step) = h.tick(slice, script) else {
                break;
            };
            let stop = step.result.is_err();
            steps.push(step);
            if stop {
                break;
            }
        }
    }
    steps
}

/// D8.2・D5.2: enabled の replay で C-mid の評価（at_risk・continue）は continue で写され、D-done の評価
/// （off_track・pivot）は `replan{checkpoint_pivot}`、F-done の評価（off_track・ask_human）は `escalate` に写る。
/// 評価の reviewer run は発火の数（3）だけ。answer key との一致は出力する（評価はスクリプトなので評価の質の根拠ではない）。
#[test]
fn parent_checkpoint_shadow_enabled_replay_maps_c_mid_continue_d_done_pivot() {
    let mut h = Shadow::new("enabled");
    let tasks_before = h.task_count();
    let steps = run_enabled_replay(&mut h);
    let key = answer_key();
    for s in &steps {
        println!(
            "step slice={} kind={} rule={} detail={} result={:?}",
            s.slice, s.kind, s.rule, s.detail, s.result
        );
    }
    let got: Vec<(&str, &str, &str)> = steps
        .iter()
        .filter(|s| s.result.is_ok())
        .map(|s| (s.slice, s.kind.as_str(), s.detail.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("C-mid", "evaluate", "[\"children:2\"]"),
            ("C-mid", "continue", ""),
            ("D-done", "evaluate", "[\"children:4\"]"),
            ("D-done", "replan", "\"checkpoint_pivot\""),
            ("F-done", "evaluate", "[\"children:6\"]"),
            ("F-done", "escalate", "\"checkpoint:dec-005\""),
        ],
    );
    // F-done の ask_human は escalate（question_key = checkpoint:<判断 id>）。
    let last = steps.last().expect("steps");
    assert_eq!(last.kind, "escalate");
    assert!(last.detail.contains(pc::QUESTION_KEY_PREFIX), "{last:?}");
    assert!(last.result.is_ok(), "{last:?}");
    assert!(steps.iter().all(|s| s.result.is_ok()), "{steps:#?}");
    assert_eq!(h.evaluator_runs, 3, "評価の reviewer run は発火の数だけ");
    // 評価と写しは task を作らない（replan・escalate は CoS 入力・受信箱で、起票は CoS と人の承認を通る）。
    assert_eq!(h.task_count(), tasks_before);
    for (slice, record) in [("C-mid", 1usize), ("D-done", 3), ("F-done", 5)] {
        let id = format!("dec-{record:03}");
        let rec = read_record(&h, &id);
        let ideal = &key["checkpoints"][slice];
        let (v, n) = (rec.effective.verdict, rec.effective.next);
        let vs = serde_json::to_value(v).expect("v");
        let ns = serde_json::to_value(n).expect("n");
        println!(
            "compare slice={slice} effective={}/{} answer_key={}/{} match={}",
            vs.as_str().unwrap_or_default(),
            ns.as_str().unwrap_or_default(),
            ideal["ideal_verdict"].as_str().unwrap_or("-"),
            ideal["ideal_action"].as_str().unwrap_or("-"),
            vs == ideal["ideal_verdict"] && ns == ideal["ideal_action"],
        );
    }
    // D-done の評価は負の研究結果（execution 成功・epistemic 前進・goal 未達）。
    let d = read_record(&h, "dec-003");
    assert!(d.effective.negative_result, "{d:?}");
    assert_eq!(
        serde_json::to_value(&d.effective.axes).expect("axes"),
        json!({"execution": "succeeded", "epistemic": "advanced", "goal": "not_met"})
    );
}

fn read_record(h: &Shadow, decision_id: &str) -> pc::ParentEvaluationRecord {
    let name = pc::evaluation_artifact_name(decision_id);
    let path = h
        .env
        .store
        .events_for(h.root.id)
        .expect("events")
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::ArtifactProduced { artifact, .. } if artifact.name == name => {
                Some(artifact.path)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("record {name}"));
    serde_json::from_slice(&std::fs::read(path).expect("record file")).expect("record json")
}

/// D5.2（checkpoint-act-fix）: 写していない評価がある間、no-progress の `escalate` と paused の `wait(waiting)` は
/// 検証を通り、計画を進める行動（launch_root・表に無い continue）は `checkpoint_action_mismatch` で落ちる。
#[test]
fn parent_checkpoint_shadow_no_progress_escalates_while_unmapped() {
    let mut h = Shadow::new("enabled");
    h.advance_to(cutoff("D-done"));
    // D-done で初めて起こした案件: 終端の子が一度に 0 → 4 に進んだので最大の k だけ（D1.2）。
    let step = h.tick("D-done", Some("D-done")).expect("evaluate");
    assert_eq!(step.kind, "evaluate");
    assert_eq!(step.detail, "[\"children:4\"]");
    let s = h.snapshot();
    assert_eq!(
        s.checkpoints[0].unmapped.len(),
        1,
        "評価済みで写していない評価"
    );
    let codes = |raw: &Value| match pp::parse_and_validate(&s, raw) {
        Ok(_) => Vec::new(),
        Err(v) => v.into_iter().map(|v| v.code).collect::<Vec<_>>(),
    };
    assert_eq!(codes(&guard_escalate(&s)), Vec::<String>::new());
    let paused_wait = proposal(
        &s,
        json!({"kind": "wait", "waiting_on": [], "next_wake_after_secs": 3600, "phase": "waiting"}),
        "pc/project-not-active",
    );
    assert_eq!(codes(&paused_wait), Vec::<String>::new());
    assert!(codes(&launch_root(&s)).contains(&"checkpoint_action_mismatch".to_string()));
    let cont = proposal_with(
        &s,
        json!({"kind": "continue", "watch_root_ids": [h.root.id.to_string()], "next_wake_after_secs": 3600}),
        pp::CHECKPOINT_ACT_RULE,
        &[(pp::CHECKPOINT_SIGNAL, "dec-001")],
    );
    assert!(codes(&cont).contains(&"checkpoint_action_mismatch".to_string()));
    // guard の escalate を記録しても評価は写していないまま残り、次の判断で act が写す。
    h.apply(&s, &guard_escalate(&s)).expect("guard escalate");
    let s2 = h.snapshot();
    assert_eq!(s2.checkpoints[0].unmapped.len(), 1);
    let mapped = h.tick("D-done", None).expect("act");
    assert_eq!(
        (mapped.kind.as_str(), mapped.detail.as_str()),
        ("replan", "\"checkpoint_pivot\"")
    );
    assert!(h.snapshot().checkpoints[0].unmapped.is_empty());
    println!(
        "no_progress_escalate=ok evaluator_runs={}",
        h.evaluator_runs
    );
}

/// D5.3（checkpoint-act-fix）: continue の写し（効果なし = `none`）は写し済みに数え、次の起票（launch_root）は
/// `checkpoint_action_mismatch` で止まらない。写す前は止まる。
#[test]
fn parent_checkpoint_shadow_continue_mapping_unblocks_next_launch() {
    let mut h = Shadow::new("enabled");
    h.advance_to(cutoff("C-mid"));
    let ev = h.tick("C-mid", Some("C-mid")).expect("evaluate");
    assert_eq!(ev.kind, "evaluate");
    let s = h.snapshot();
    let mismatch = |s: &ProjectSnapshot| {
        pp::parse_and_validate(s, &launch_root(s))
            .err()
            .unwrap_or_default()
            .into_iter()
            .any(|v| v.code == "checkpoint_action_mismatch")
    };
    assert!(mismatch(&s), "写す前の起票は止まる");
    let cont = h.tick("C-mid", None).expect("act");
    assert_eq!(cont.kind, "continue");
    assert!(cont.result.is_ok(), "{cont:?}");
    assert_eq!(h.decisions.last().expect("decision").0.effect_state, "none");
    let after = h.snapshot();
    assert!(
        after.checkpoints[0].unmapped.is_empty(),
        "continue は写し済み"
    );
    assert!(
        !mismatch(&after),
        "写した後の起票は checkpoint で止まらない"
    );
    println!("continue_mapped=ok next_launch_unblocked=ok");
}

/// D9.1・controller-launch: shadow の案件で `suppressed` の評価の後に mode を enabled に上げると、次の due
/// （children:4）で評価の run が 1 回起き、shadow で発火済みの children:2 は評価し直さない。
#[test]
fn parent_checkpoint_shadow_mode_raised_to_enabled_evaluates_next_due_once() {
    let mut h = Shadow::new("shadow");
    h.advance_to(cutoff("C-mid"));
    let first = h.tick("C-mid", Some("C-mid")).expect("evaluate");
    assert_eq!(h.decisions[0].0.effect_state, "suppressed");
    assert_eq!(h.evaluator_runs, 0, "{first:?}");
    h.controller.mode = "enabled".into();
    h.advance_to(cutoff("C-done"));
    assert!(h.tick("C-done", None).is_none(), "children:2 は発火済み");
    h.advance_to(cutoff("D-done"));
    let second = h.tick("D-done", Some("D-done")).expect("evaluate");
    assert_eq!(second.detail, "[\"children:4\"]");
    assert_eq!(h.evaluator_runs, 1);
    println!("shadow_then_enabled evaluator_runs={}", h.evaluator_runs);
}

/// D8.4: 予算の閾値（50・80 %）で `budget:<p>` が due になる（root の `max_tokens` に対する木の使った token）。
#[test]
fn parent_checkpoint_shadow_budget_50_80() {
    let mut h = Shadow::with_budget("shadow", Some(1_000));
    h.tokens_used = 499;
    assert!(h.tick("A-mid", None).is_none());
    h.tokens_used = 500;
    let a = h.tick("A-mid", None).expect("budget:50");
    assert_eq!(a.detail, "[\"budget:50\"]");
    h.tokens_used = 800;
    let b = h.tick("A-mid", None).expect("budget:80");
    assert_eq!(b.detail, "[\"budget:80\"]");
    assert!(h.tick("A-mid", None).is_none());
    println!("budget_fired=50,80");
}

/// D8.4: root の子の `failed`（理由を問わない）で `child_failed:<id>` が due になる。
#[test]
fn parent_checkpoint_shadow_child_failed() {
    let mut h = Shadow::new("shadow");
    let child = h.children["H"].clone();
    h.env
        .store
        .append_event(
            child.id,
            &Event::Transitioned {
                from: Status::Running,
                to: Status::Failed,
                reason: "review_failed".into(),
            },
        )
        .expect("failed");
    let step = h.tick("A-mid", None).expect("child_failed");
    assert_eq!(step.detail, format!("[\"child_failed:{}\"]", child.id));
}

/// D8.4: root の subtree の task が作った新しい版の文書に、参照した版に無い `stance: refute` の証拠があると
/// `refute:<strategy>:v<n>:<id>` が due になる。
#[test]
fn parent_checkpoint_shadow_refute_in_subtree() {
    let mut h = Shadow::new("shadow");
    // root が参照する v1 の文書（文書の task = root）。v1 に既にある反証は数えない。
    let root = h.root.clone();
    produce(&h, &root, 1, DOC_V1.as_bytes());
    let mut doc: Value = serde_json::from_str(DOC_V1).expect("doc");
    assert_eq!(doc["strategy_id"], STRATEGY_ID);
    doc["version"] = json!(2);
    let mut e = doc["evidence"][0].clone();
    e["id"] = json!("e-shadow-refute");
    e["stance"] = json!("refute");
    doc["evidence"].as_array_mut().expect("evidence").push(e);
    let bytes = serde_json::to_vec(&doc).expect("doc");
    let child = h.children["D"].clone();
    produce(&h, &child, 2, &bytes);
    let step = h.tick("A-mid", None).expect("refute");
    assert_eq!(
        step.detail,
        format!("[\"refute:{STRATEGY_ID}:v2:e-shadow-refute\"]")
    );
}

/// D4 R2・R3・R4・R7: 参照の無い改善の主張（url だけ・参照 0 件）は unsupported に数え、on_track・goal met にならず、
/// next は ask_human に直り、写しの表は escalate だけ（continue は落ちる）。
#[test]
fn parent_checkpoint_shadow_unsupported_claims_never_on_track() {
    let mut h = Shadow::new("enabled");
    h.advance_to(cutoff("C-mid"));
    h.tick("C-mid", Some("unsupported")).expect("evaluate");
    let rec = read_record(&h, "dec-001");
    assert_eq!(rec.status, pc::RecordStatus::Checked);
    assert_eq!(
        serde_json::to_value(rec.effective.verdict).expect("v"),
        json!("unknown")
    );
    assert_eq!(
        serde_json::to_value(rec.effective.next).expect("n"),
        json!("ask_human")
    );
    assert!(rec.effective.unsupported >= 2, "{rec:?}");
    assert_ne!(
        serde_json::to_value(&rec.effective.axes).expect("axes")["goal"],
        json!("met")
    );
    let s = h.snapshot();
    let allowed: Vec<&str> = s.checkpoints[0].unmapped[0]
        .allowed
        .iter()
        .map(|a| a.kind.as_str())
        .collect();
    assert_eq!(allowed, vec!["escalate"]);
    let mapped = h.tick("C-mid", None).expect("act");
    assert_eq!(mapped.kind, "escalate");
    println!(
        "unsupported={} effective=unknown/ask_human",
        rec.effective.unsupported
    );
}

/// 戦略の文書 `strategy/<id>/v<n>.json` を `task` の成果物として記録する。
fn produce(h: &Shadow, task: &Task, version: u32, bytes: &[u8]) {
    let path = h.env.dir.path().join(format!("strategy-v{version}.json"));
    std::fs::write(&path, bytes).expect("write");
    h.env
        .store
        .append_event(
            task.id,
            &Event::ArtifactProduced {
                run_id: format!("run-v{version}"),
                artifact: task_core::model::ArtifactRef {
                    name: format!("strategy/{STRATEGY_ID}/v{version}.json"),
                    path: path.to_string_lossy().into_owned(),
                    sha256: sha_hex(bytes),
                    kind: "strategy".into(),
                    declared: true,
                },
            },
        )
        .expect("artifact");
}
