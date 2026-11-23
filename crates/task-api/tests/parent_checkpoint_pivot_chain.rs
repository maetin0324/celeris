//! ADR 2026-11-22-parent-checkpoint-evaluator D5.2・D7 と子 5（ADR 2026-11-22-strategy-review-critic D2.4・D4）: D-done の
//! 途中評価の pivot から、新しい戦略の版と子 5 の gate の通過までを 1 つの試験でつなぐ（接頭辞 `parent_checkpoint_pivot_chain_`）。
//!
//! 段: (1) 第二ゲートの shadow の slice（`tests/fixtures/parent_checkpoint_shadow/`）の D-done までの子 A〜H の遷移を一時 DB に流し、
//! 本物の snapshot（`task_ops::project_snapshot::build`・`checkpoints`）→ 規則 → 本物の Rust の検証（`project_proposal::parse_and_validate`）で
//! `evaluate` を出し、評価の記録を dispatcher（`task-dispatch` の `review/parent_evaluation.rs` の `write_record`）と同じ置き場と形で root に書き、
//! 次の wake で `replan{checkpoint_pivot}` に写す。(2) 案件 CoS の LLM の代わりに、CoS の skill（`config/skills/cos-operator/strategy-review.md`
//! の「checkpoint の replan を受けたとき」）の決まった操作を CoS の run の bearer で流す: replan の参照から評価の記録を API で読み、
//! 新しい版の文書の task を `/cos/operations` の `task.create` で起票し、その run が `celeris.strategy/1` の v2（反証された仮説の候補を
//! 却下し、評価の記録の `ArtifactProduced` を反証の証拠に引く）を書く。(3) 人の承認の後に子 5 の gate（`cos::strategy_gate`）が
//! v2 の識別実験の起票を通す。
//!
//! 守る不変条件と、既存の試験で検出できない理由:
//! - 反証された版（v1）へ戻る起票、v2 で却下した候補の起票、人の承認の無い v2（CoS の答えを含む）の起票を gate が断り、task は増えない。
//! - Project.request と承認済み Roadmap の行が鎖の前後で変わらない。
//! - `parent_checkpoint_shadow_` は replan への写しで止まり（CoS も gate も通さない）、`strategy_review_shadow_` は Strategy Review の
//!   実験の結果から版を上げる（途中評価の記録を読まない）。どちらも「評価の記録 → replan の参照 → CoS が読む → 新しい版 → gate」を
//!   1 つの流れで見ないので、段の間の参照の受け渡し（記録の path を API と gate が引けるか）が切れても落ちない。
//!
//! LLM を呼ばない。外部ネットワークに出ない。本番の DB に触れない。slice の fixture は読むだけ。

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::Router;
use common::cos_ops::{cos_run_bearer, post_op};
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::chat::{AuditContext, ChatActor, ChatOpenThreadRequest};
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::model::{ArtifactRef, Event, Status, Task, TaskId, TaskKind};
use task_core::parent_checkpoint as pc;
use task_core::project_proposal::{
    self as pp, ProjectAction, ProjectProposal, ProjectSnapshot, SnapshotController,
    SnapshotDecision, SnapshotLimits,
};
use task_core::strategy;
use task_core::{Project, ProjectId};
use task_ops::project_snapshot::{self as ps, CheckpointDecisionRow, SnapshotContext};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/parent_checkpoint_shadow"
);
/// 全時点の events の親集合（G-done の slice）。D-done の cutoff で切って使う。
const ALL_EVENTS: &str = "G-done/events.jsonl";
const CHILDREN: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];
const STRATEGY_ID: &str = "arch-v4-cycle";
const FIXED: &str = "fixed-plan-a-to-h";
const CUT: &str = "cut-boundary-edges";
const H_FIXED: &str = "h-fixed-plan";
const H_CUT: &str = "h-boundary-edges";
const RECOUNT: &str = "recount-baseline";
const H_RECOUNT: &str = "h-measure-first";
const CUT_HOURS: f64 = 4.0;
const APPROVER: &str = "human:shadow";

const REQUEST: &str = "アーキv4 の親の目的（shadow）: 責務境界と疎結合性を実測で改善する。\n\
```celeris-outcomes\n\
[{\"id\":\"o1\",\"title\":\"循環の削減\",\"acceptance\":[\"既存の module 循環の数が base より減る\"]}]\n\
```\n";

fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn ts(raw: &str) -> OffsetDateTime {
    OffsetDateTime::parse(raw, &Rfc3339).expect("rfc3339")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(FIXTURE).join(rel)).expect("fixture file")
}

fn cutoff(slice: &str) -> OffsetDateTime {
    let v: Value = serde_json::from_str(&read(&format!("{slice}/slice.json"))).expect("slice");
    ts(v["cutoff"].as_str().expect("cutoff"))
}

/// 作った task の workspace（gate の `Reader::file` と同じ `task_ops::workspace::local_dir`）。
fn workspace(env: &TestEnv, task: &Task) -> PathBuf {
    let dir = task_ops::workspace::local_dir(&env.store, &env.view_context().workspace_roots, task)
        .expect("workspace");
    std::fs::create_dir_all(&dir).expect("workspace dir");
    dir
}

/// worker の run の成果物（`task_worker::artifact` と同じ workspace 相対の path）。sha256 を返す。
fn produce(env: &TestEnv, task: &Task, run: &str, name: &str, bytes: &[u8]) -> String {
    let path = format!("artifacts/{name}");
    let file = workspace(env, task).join(&path);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
    std::fs::write(&file, bytes).expect("write");
    let sha256 = sha_hex(bytes);
    env.store
        .append_event(
            task.id,
            &Event::ArtifactProduced {
                run_id: run.into(),
                artifact: ArtifactRef {
                    name: name.into(),
                    path,
                    sha256: sha256.clone(),
                    kind: "file".into(),
                    declared: true,
                },
            },
        )
        .expect("artifact");
    sha256
}

fn set_status(env: &TestEnv, task: &Task, status: &str) {
    rusqlite::Connection::open(&env.db_path)
        .expect("db")
        .execute(
            "UPDATE tasks SET status = ?1, json = json_set(json, '$.status', ?1) WHERE id = ?2",
            rusqlite::params![status, task.id.to_string()],
        )
        .expect("status");
}

fn tasks(env: &TestEnv) -> i64 {
    count(env, "SELECT COUNT(*) FROM tasks")
}

/// 文書の task に決定 `strategy-approve-v<n>` を出し、`by` が approve と答えた（子 2 D2.2）。決定の id を返す。
fn answer(env: &TestEnv, source: &Task, version: u32, sha256: &str, by: &str) -> String {
    let id = format!("d-{}", TaskId::new());
    let opt = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    let request = DecisionRequest {
        id: id.clone(),
        key: strategy::approval_decision_key(version),
        kind: DecisionKind::Choice,
        question: strategy::approval_question(STRATEGY_ID, version, sha256),
        options: vec![opt(strategy::APPROVE_OPTION), opt(strategy::REJECT_OPTION)],
        recommended: strategy::APPROVE_OPTION.into(),
        cost_of_reversal: CostOfReversal::Medium,
        cost_note: None,
        needed_before: vec!["self".into()],
        path: vec![DecisionPathEntry {
            task_id: source.id,
            title: source.title.clone(),
            stage: None,
            unit: None,
        }],
        raised_by: DecisionRaisedBy {
            task_id: source.id,
            run_id: None,
            origin: DecisionOrigin::Worker,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    env.store
        .append_event(
            source.id,
            &Event::DecisionRequested {
                decision: Box::new(request),
            },
        )
        .expect("decision");
    env.store
        .append_event(
            source.id,
            &Event::DecisionAnswered {
                id: id.clone(),
                option: strategy::APPROVE_OPTION.into(),
                note: None,
                by: by.into(),
            },
        )
        .expect("answer");
    id
}

/// 同じ決定 `id` に `by` が approve と答え直した（gate は決定の最後の答えを読む）。
fn answer_again(env: &TestEnv, source: &Task, id: &str, by: &str) {
    env.store
        .append_event(
            source.id,
            &Event::DecisionAnswered {
                id: id.into(),
                option: strategy::APPROVE_OPTION.into(),
                note: None,
                by: by.into(),
            },
        )
        .expect("answer");
}

// ---- 案件と controller の harness（判断の記録と revision は harness が持つ。`parent_checkpoint_shadow` と同じ形） ----

struct Chain {
    env: TestEnv,
    app: Router,
    bearer: String,
    project: Project,
    root: Task,
    children: BTreeMap<String, Task>,
    controller: SnapshotController,
    decisions: Vec<(SnapshotDecision, ProjectProposal)>,
    settings: pc::CheckpointSettings,
    applied: usize,
}

impl Chain {
    async fn new() -> Self {
        let env = admin_env();
        let app = env.router();
        let now = OffsetDateTime::now_utc();
        let project = Project {
            id: ProjectId::new(),
            title: "shadow:arch-v4 pivot chain".into(),
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
        let thread = env
            .store
            .cos_thread_open(
                &ChatOpenThreadRequest {
                    project_id: Some(project.id.to_string()),
                },
                now,
            )
            .expect("thread")
            .thread;
        let bearer = cos_run_bearer(&env, &thread.id, "pivot-chain");
        // 人が承認した Roadmap（版 1、Outcome o1）。
        let base = format!("/api/v1/projects/{}/roadmap", project.id);
        let p = send(
            &app,
            post_admin(
                &format!("{base}/proposals"),
                &json!({"base_version": null, "change_reason": "人の方針",
                        "outcomes": [{"id": "o1", "title": "循環の削減",
                                      "acceptance": ["既存の module 循環の数が base より減る"],
                                      "priority": 1, "depends_on": []}]}),
            ),
        )
        .await;
        assert_eq!(p.status.as_u16(), 201, "{}", p.text());
        let d = send(
            &app,
            post_admin(
                &format!("{base}/proposals/1/decide"),
                &json!({"decision": "approve", "note": "確認した"}),
            ),
        )
        .await;
        assert_eq!(d.status.as_u16(), 200, "{}", d.text());
        let mut root = new_task(TaskKind::Execute, Status::Running);
        root.project_id = Some(project.id);
        root.title = "[アーキv4 親]（shadow）".into();
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
        Self {
            env,
            app,
            bearer,
            project,
            root,
            children,
            controller: SnapshotController {
                mode: "enabled".into(),
                phase: "active".into(),
                revision: 0,
                human_epoch: 0,
                retry_count: 0,
                progress_epoch: 0,
            },
            decisions: Vec::new(),
            settings: pc::CheckpointSettings::default(),
            applied: 0,
        }
    }

    fn task(&self, id: &str) -> Task {
        let id: TaskId = id.parse().expect("task id");
        self.env.store.get(id).expect("get").expect("task")
    }

    /// cutoff までの子 A〜H の遷移を store に流す（`parent_checkpoint_shadow` の `advance_to` と同じ選別）。
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
            self.project.id,
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
            &mut |_| Ok(0),
        )
        .expect("checkpoints");
        s
    }

    /// Rust の検証を通して記録する（enabled の効果は applied、continue・wait は none）。
    fn apply(&mut self, s: &ProjectSnapshot, raw: &Value) -> (String, ProjectProposal) {
        let p = pp::parse_and_validate(s, raw).unwrap_or_else(|v| panic!("{raw}: {v:?}"));
        let effect = match p.action {
            ProjectAction::Wait { .. } | ProjectAction::Continue { .. } => "none",
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
        self.decisions.push((d, p.clone()));
        self.controller.revision += 1;
        (id, p)
    }

    /// 評価の reviewer run の代わり: 第二ゲートの slice のスクリプトの D-done の評価を `check_evaluation` に通し、
    /// 最終の記録を dispatcher の `write_record` と同じ置き場（root の `artifacts_dir_for`）と path の形で root に書く。
    fn run_evaluator(&self, decision_id: &str) -> pc::ParentEvaluationRecord {
        let all: Value =
            serde_json::from_str(&read("scripted-evaluations.json")).expect("scripted");
        let mut text = all["D-done"].to_string();
        for (k, t) in &self.children {
            text = text.replace(&format!("@{k}"), &t.id.to_string());
        }
        let evaluation: Value = serde_json::from_str(&text).expect("evaluation");
        let mut children = pc::ChildOutcomes::default();
        for t in self.children.values() {
            let last = self
                .env
                .store
                .events_for(t.id)
                .expect("events")
                .into_iter()
                .filter_map(|(_, e)| match e {
                    Event::Transitioned { to, .. } => Some(to),
                    _ => None,
                })
                .last();
            if last == Some(Status::Done) {
                children.done_passed += 1;
            }
        }
        let facts = pc::EvaluationFacts {
            resolved: pc::references_to_resolve(&evaluation),
            goal_reached: false,
            children,
        };
        let checked = pc::check_evaluation(Some(&evaluation), &facts);
        let record = pc::ParentEvaluationRecord::new(
            decision_id,
            &self.root.id.to_string(),
            Some(evaluation),
            &checked,
        );
        let text = serde_json::to_string_pretty(&record).expect("record");
        let name = pc::evaluation_artifact_name(decision_id);
        let dir = workspace(&self.env, &self.root);
        let rel = Path::new(task_core::artifacts::ARTIFACTS_DIR_NAME).join(&name);
        let file = task_core::artifacts::artifacts_dir_for(&self.root, &dir).join(&name);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        std::fs::write(&file, &text).expect("write record");
        assert_eq!(file, dir.join(&rel), "root は自分の workspace を持つ");
        self.env
            .store
            .append_event(
                self.root.id,
                &Event::ArtifactProduced {
                    run_id: format!("eval-{decision_id}"),
                    artifact: ArtifactRef {
                        name,
                        path: rel.to_string_lossy().into_owned(),
                        sha256: sha_hex(text.as_bytes()),
                        kind: pc::EVALUATION_ARTIFACT_KIND.into(),
                        declared: false,
                    },
                },
            )
            .expect("artifact");
        record
    }

    async fn send_op(&self, key: &str, body: Value, strategy: Option<&Value>) -> (u16, Value) {
        let mut op = json!({
            "idempotency_key": key,
            "expected_revision": null,
            "reason": "checkpoint の replan（checkpoint_pivot）を受けた",
            "policy_version": "1",
            "request": {"method": "POST", "path": "/api/v1/tasks", "body": body},
        });
        if let Some(s) = strategy {
            op["strategy"] = s.clone();
        }
        let resp = post_op(&self.app, &self.bearer, &op).await;
        (resp.status.as_u16(), resp.json())
    }

    async fn passes(&self, key: &str, body: Value, strategy: Option<&Value>) -> Task {
        let (status, resp) = self.send_op(key, body, strategy).await;
        assert_eq!(status, 200, "{key}: {resp}");
        self.task(resp["operation"]["target_id"].as_str().expect("target_id"))
    }

    /// 422 で code の集合に `want` があり、task は作られない。
    async fn refused(&self, key: &str, body: Value, strategy: &Value, want: &str) -> Value {
        let before = tasks(&self.env);
        let (status, resp) = self.send_op(key, body, Some(strategy)).await;
        assert_eq!(status, 422, "{key}: {resp}");
        let codes: Vec<&str> = resp["codes"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert!(codes.contains(&want), "{key}: want {want}: {resp}");
        assert_eq!(
            tasks(&self.env),
            before,
            "{key}: 断った起票が task を作った"
        );
        resp
    }

    fn body(&self, title: &str, parent: bool, atomic_secs: Option<u64>) -> Value {
        let mut b = json!({
            "title": title,
            "objective": format!("{title}（shadow: アーキv4 の D-done の記録だけを使う）"),
            "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
            "project_id": self.project.id.to_string(),
        });
        if parent {
            b["parent"] = json!(self.root.id.to_string());
        }
        if let Some(secs) = atomic_secs {
            b["execution"] = json!("atomic");
            b["max_wall_secs"] = json!(secs);
        }
        b
    }

    /// 文書の task の run が版を書き、成果物として記録する。文書の参照 `{task_id, name, sha256}` を返す。
    fn write_doc(&self, source: &Task, run: &str, doc: &Value) -> Value {
        let bytes = serde_json::to_vec_pretty(doc).expect("json");
        let version = u32::try_from(doc["version"].as_u64().expect("version")).expect("u32");
        let name = strategy::artifact_name(STRATEGY_ID, version);
        let sha256 = produce(&self.env, source, run, &name, &bytes);
        json!({"task_id": source.id.to_string(), "name": name, "sha256": sha256})
    }

    /// 承認済み Roadmap の全版と Project.request（鎖の前後で比べる）。
    fn human_owned(&self) -> (String, Value) {
        let project = self
            .env
            .store
            .project_get(self.project.id)
            .expect("project")
            .expect("exists");
        let versions = self
            .env
            .store
            .roadmap_versions(&self.project.id.to_string())
            .expect("roadmap");
        (
            project.request,
            serde_json::to_value(&versions).expect("roadmap json"),
        )
    }
}

// ---- 戦略の文書 ----

fn evidence(id: &str, stance: &str, hypothesis: &str, statement: &str, refs: Vec<Value>) -> Value {
    json!({
        "id": id, "stance": stance, "hypothesis": hypothesis, "statement": statement,
        "observed_at": "2026-11-19T12:00:00Z",
        "source": {"kind": "celeris_task_run", "label": "アーキv4 の記録と途中評価"},
        "references": refs,
    })
}

fn candidate(id: &str, title: &str, hypothesis: &str, hours: f64, selected: bool) -> Value {
    json!({
        "id": id, "title": title, "approach": format!("{title}（アーキv4 の記録から）"),
        "decision_method": format!("識別実験の結果で {hypothesis} が支持されれば採り、反証されれば捨てる"),
        "cost": {"usd_max": 60.0, "wall_hours_max": 12.0},
        "risks": ["結果を待つ間に親の予算を使う"],
        "reevaluate_when": ["識別実験が終わった", "親の途中評価が off_track"],
        "hypotheses": [hypothesis],
        "selected": selected,
        "state": "open",
        "experiment": {"content": format!("{hypothesis} を区別する最小の実験を 1 つ走らせる"),
                       "discriminates": [hypothesis],
                       "budget": {"usd_max": 20.0, "wall_hours_max": hours}},
        "expected_information_value": format!("{hypothesis} の支持か反証で候補を選ぶか捨てる"),
        "stop_when": ["予算の時間で終わらない"],
    })
}

/// v1: 史実の親が採った固定計画（selected）と、境界の辺を切る案・数え直す案。
fn document_v1(c: &Chain, run: &str) -> Value {
    let a = json!({"kind": "task", "task_id": c.children["A"].id.to_string()});
    json!({
        "schema": strategy::SCHEMA,
        "project_id": c.project.id.to_string(),
        "strategy_id": STRATEGY_ID,
        "version": 1,
        "supersedes": null,
        "idempotency_key": strategy::idempotency_key(STRATEGY_ID, 1),
        "source": {"kind": "celeris_task_run", "label": "Strategy Review の run"},
        "actor": {"kind": "worker", "id": run},
        "source_revision": {"request_sha256": sha_hex(REQUEST.as_bytes()), "roadmap_version": 1},
        "timestamp": "2026-11-17T00:00:00Z",
        "acl": {"classification": "internal", "outbound": false},
        "decision_state": "proposed",
        "research_question": {"text": "子 A〜H の固定の計画で既存の module 循環の数が減るか", "references": [a]},
        "candidates": [
            candidate(FIXED, "子 A〜H の固定の計画を続ける", H_FIXED, 2.0, true),
            candidate(CUT, "handlers ⇄ cos の境界の辺を先に切る", H_CUT, CUT_HOURS, false),
            candidate(RECOUNT, "A の command で循環を数え直してから決める", H_RECOUNT, 3.0, false),
        ],
        "held_assumptions": [{"id": "a-count-rule", "statement": "循環の数え方は A の command のまま",
                              "revisit_when": "数え方が変わった"}],
        "hypotheses": [
            {"id": H_FIXED, "statement": "固定計画 A〜H の局所の切断で循環が減る", "confidence": "estimated",
             "falsified_by": "子が done でも循環の数が 17 のまま", "evidence": ["e-plan"]},
            {"id": H_CUT, "statement": "少数の境界の辺が循環の大半を作る", "confidence": "estimated",
             "falsified_by": "境界の辺を切っても循環の単位が減らない", "evidence": ["e-boundary"]},
            {"id": H_RECOUNT, "statement": "子ごとに循環の数え方が違い、改善が見えていない", "confidence": "estimated",
             "falsified_by": "A の command で数え直しても値が同じ", "evidence": ["e-count"]},
        ],
        "evidence": [
            evidence("e-plan", "unknown", H_FIXED, "親は子 A〜H を作成の直後に全部起票した", vec![a.clone()]),
            evidence("e-boundary", "unknown", H_CUT, "A の対象は境界の循環", vec![a.clone()]),
            evidence("e-count", "unknown", H_RECOUNT, "B〜H は A の計測の command を使う", vec![a]),
        ],
    })
}

/// v2（CoS の skill の `checkpoint_pivot` の手順 2）: 評価の記録の根拠に数えた `refuted` で、反証された仮説の候補を
/// `rejected` にし、記録の成果物と評価が引いた元の task を反証の証拠に引く。selected は書かない（残る候補の識別実験の版）。
fn document_v2(
    v1: &Value,
    p1: &Value,
    run: &str,
    record_ref: &Value,
    record: &Value,
    decision_id: &str,
) -> Value {
    // 根拠に数えるのは task・artifact・commit の参照を持つ `refuted` の項目だけ（skill の手順 1、ADR D4 の R2）。
    let refuted = record["evaluation"]["refuted"]
        .as_array()
        .expect("refuted")
        .iter()
        .find(|r| {
            r["references"].as_array().is_some_and(|refs| {
                refs.iter()
                    .any(|x| matches!(x["kind"].as_str(), Some("task" | "artifact" | "commit")))
            })
        })
        .expect("根拠のある refuted");
    let hypothesis_text = refuted["hypothesis"].as_str().expect("refuted hypothesis");
    let hypothesis = v1["hypotheses"]
        .as_array()
        .expect("hypotheses")
        .iter()
        .find(|h| h["statement"] == hypothesis_text)
        .and_then(|h| h["id"].as_str())
        .expect("反証された仮説が v1 にある");
    assert_eq!(hypothesis, H_FIXED);
    let mut refs = vec![record_ref.clone()];
    refs.extend(
        refuted["references"]
            .as_array()
            .expect("refs")
            .iter()
            .cloned(),
    );
    let mut doc = v1.clone();
    doc["version"] = json!(2);
    doc["idempotency_key"] = json!(strategy::idempotency_key(STRATEGY_ID, 2));
    doc["supersedes"] = p1.clone();
    doc["actor"]["id"] = json!(run);
    doc["change_reason"] = json!(format!(
        "判断 {decision_id}: effective.verdict {} ・ next {}。実行は成功・知見は前進・当初の目標は未達",
        record["effective"]["verdict"].as_str().unwrap_or("-"),
        record["effective"]["next"].as_str().unwrap_or("-"),
    ));
    for c in doc["candidates"].as_array_mut().expect("candidates") {
        if c["id"] == FIXED {
            c["selected"] = json!(false);
            c["state"] = json!("rejected");
            c["rejected_by"] = json!(["e-checkpoint-refute"]);
        }
    }
    let h = &mut doc["hypotheses"][0];
    h["confidence"] = json!("contradicted");
    h["evidence"] = json!(["e-plan", "e-checkpoint-refute"]);
    doc["evidence"]
        .as_array_mut()
        .expect("evidence")
        .push(evidence(
            "e-checkpoint-refute",
            "refute",
            hypothesis,
            refuted["statement"].as_str().expect("statement"),
            refs,
        ));
    doc
}

// ---- 試験 ----

/// (1) D-done の評価 → `replan{checkpoint_pivot}`、(2) CoS が記録を読み v2 を propose、(3) 人の承認の後に gate が v2 の
/// 識別実験を通す。同じ流れで、v1 に戻る起票・却下した候補の起票・承認の無い v2 の起票が断られ、Project.request と
/// 承認済み Roadmap の行が変わらないことを確かめる。
#[tokio::test]
async fn parent_checkpoint_pivot_chain_d_done_pivot_to_gated_v2() {
    let mut c = Chain::new().await;
    let human_before = c.human_owned();
    assert!(human_before.1.as_array().is_some_and(|v| v.len() == 1));

    // 前提: v1 の文書（Strategy Review の run が書き、人が承認し、Critic を通って done）と、v1 を参照する strategy root。
    let v1_source = c
        .passes(
            "doc-v1",
            c.body("Strategy Review v1: アーキv4 の戦略候補", false, None),
            None,
        )
        .await;
    let run1 = "01M7PIVOTRUN0000000000000V1";
    let v1 = document_v1(&c, run1);
    let p1 = c.write_doc(&v1_source, run1, &v1);
    answer(
        &c.env,
        &v1_source,
        1,
        p1["sha256"].as_str().expect("sha"),
        APPROVER,
    );
    set_status(&c.env, &v1_source, "done");
    // 史実の親は子 5 の gate より前に起票された。gate が書くのと同じ `task.create` の操作の記録を store に置く（strategy root の正本 D1.1）。
    let op = ulid::Ulid::new().to_string();
    let mut sref = p1.clone();
    sref["candidate"] = json!(FIXED);
    c.env
        .store
        .cos_operation_apply_at(
            &AuditContext {
                actor: ChatActor::Cos,
                thread_id: "thread".into(),
                run_id: "cos-run".into(),
                operation_id: op.clone(),
                reason: "アーキv4 の親".into(),
                policy_version: "v1".into(),
            },
            &op,
            "hash",
            "task",
            &c.root.id.to_string(),
            None,
            "task.create",
            &json!({"method": "POST", "path": "/api/v1/tasks", "body": {"title": "root"}, "strategy": sref}),
            OffsetDateTime::now_utc(),
            |_, _| Ok(json!({})),
        )
        .expect("root operation");

    // (1) D-done: checkpoint `children:4` → evaluate → 評価の記録 → 次の wake で replan{checkpoint_pivot}。
    c.advance_to(cutoff("D-done"));
    let s = c.snapshot();
    assert_eq!(s.checkpoints.len(), 1, "strategy root は 1 つ");
    let cp = &s.checkpoints[0];
    assert_eq!(cp.root_id, c.root.id.to_string());
    let triggers: Vec<&str> = cp.due.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(triggers, ["children:4"]);
    let raw = proposal(
        &s,
        json!({"kind": "evaluate", "root_id": cp.root_id, "triggers": triggers}),
        "pc/checkpoint/evaluate",
        &[],
    );
    let (evaluate_id, _) = c.apply(&s, &raw);
    let record = c.run_evaluator(&evaluate_id);
    assert_eq!(record.status, pc::RecordStatus::Checked);
    let s = c.snapshot();
    let (cp, unmapped) = s
        .checkpoints
        .iter()
        .find_map(|cp| cp.unmapped.first().map(|e| (cp, e)))
        .expect("写していない評価");
    assert_eq!(unmapped.status, "checked", "snapshot が記録を読めた");
    assert_eq!(unmapped.effective["next"], "pivot");
    let allowed = unmapped
        .allowed
        .iter()
        .find(|a| a.kind == "replan")
        .expect("replan が許される");
    assert_eq!(allowed.reason_code.as_deref(), Some(pc::REASON_PIVOT));
    let raw = proposal(
        &s,
        json!({"kind": "replan", "reason_code": allowed.reason_code,
               "question": "checkpoint の評価に沿って戦略の次の版を作る",
               "context_refs": [format!("task:{}", cp.root_id), unmapped.record_ref, cp.strategy_ref]}),
        pp::CHECKPOINT_ACT_RULE,
        &[(pp::CHECKPOINT_SIGNAL, &unmapped.decision_id)],
    );
    let (replan_id, replan) = c.apply(&s, &raw);
    let ProjectAction::Replan {
        reason_code,
        context_refs,
        ..
    } = &replan.action
    else {
        panic!("replan でない: {:?}", replan.action);
    };
    assert_eq!(reason_code, pc::REASON_PIVOT);
    // 写しは task を作らない（起票は CoS と人の承認を通る）。
    let tasks_after_replan = tasks(&c.env);

    // (2) CoS: replan の参照（id だけ）から評価の記録を API で読む（skill の手順 1）。
    let record_name = context_refs
        .iter()
        .find_map(|r| r.strip_prefix(&format!("artifact:{}/", c.root.id)))
        .filter(|n| n.starts_with(pc::EVALUATION_ARTIFACT_PREFIX))
        .expect("replan の参照に評価の記録がある")
        .to_string();
    let list = send(
        &c.app,
        get_with(
            &format!("/api/v1/tasks/{}/artifacts", c.root.id),
            &[("authorization", c.bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(list.status.as_u16(), 200, "{}", list.text());
    let listed = list.json();
    let rows = listed["items"].as_array().cloned().unwrap_or_default();
    let idx = rows
        .iter()
        .position(|a| a["artifact"]["name"] == record_name.as_str())
        .expect("root の成果物の一覧に評価の記録がある");
    assert_eq!(
        rows[idx]["forbidden"], false,
        "記録の path を API が引ける: {}",
        rows[idx]
    );
    let body = send(
        &c.app,
        get_with(
            &format!("/api/v1/tasks/{}/artifacts/{idx}", c.root.id),
            &[("authorization", c.bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(body.status.as_u16(), 200, "記録の本文: {}", body.text());
    let read_back: Value = serde_json::from_str(&body.text()).expect("record json");
    assert_eq!(read_back["effective"]["next"], "pivot");
    assert_eq!(read_back["effective"]["negative_result"], true);
    let record_sha = rows[idx]["artifact"]["sha256"]
        .as_str()
        .expect("sha256")
        .to_string();
    assert_eq!(record_sha, sha_hex(body.text().as_bytes()));
    let record_ref = json!({"kind": "artifact", "task_id": c.root.id.to_string(),
                            "name": record_name, "sha256": record_sha});

    // CoS が新しい版の文書の task を root の子として起票し、その run が v2 を書く（skill の手順 2・3）。
    let v2_source = c
        .passes(
            &format!("checkpoint-{evaluate_id}-{STRATEGY_ID}-v2"),
            c.body("Strategy Review v2: checkpoint の pivot", true, None),
            None,
        )
        .await;
    assert_eq!(v2_source.parent_id, Some(c.root.id));
    let run2 = "01M7PIVOTRUN0000000000000V2";
    let v2 = document_v2(&v1, &p1, run2, &record_ref, &read_back, &evaluate_id);
    let p2 = c.write_doc(&v2_source, run2, &v2);
    let sha2 = p2["sha256"].as_str().expect("sha").to_string();
    let with = |p: &Value, candidate: &str| {
        let mut s = p.clone();
        s["candidate"] = json!(candidate);
        s
    };
    let experiment = |c: &Chain| c.body("識別実験: 境界の辺の数を測る", true, Some(3600));

    // 承認の無い v2 は起票できない: 人の答えが無いとき、CoS が approve と答えたとき（人でない）。
    set_status(&c.env, &v2_source, "done");
    c.refused(
        "v2-unapproved",
        experiment(&c),
        &with(&p2, CUT),
        "strategy_unapproved",
    )
    .await;
    let d2 = answer(&c.env, &v2_source, 2, &sha2, "cos");
    c.refused(
        "v2-cos-answer",
        experiment(&c),
        &with(&p2, CUT),
        "strategy_unapproved",
    )
    .await;

    // (3) 人が v2 を承認した後、子 5 の gate が v2 の識別実験の起票を通す。
    answer_again(&c.env, &v2_source, &d2, APPROVER);
    let x = c
        .passes("v2-experiment", experiment(&c), Some(&with(&p2, CUT)))
        .await;
    assert_eq!(x.parent_id, Some(c.root.id));
    assert_eq!(
        tasks(&c.env),
        tasks_after_replan + 2,
        "v2 の文書の task と実験の 2 件だけ"
    );

    // 反証された版（v1・固定計画）へ戻る起票と、v2 で却下した候補の起票を gate が断る。
    let fixed = || c.body("固定計画を続ける（E〜H）", true, Some(3600));
    c.refused("back-to-v1", fixed(), &with(&p1, FIXED), "strategy_stale")
        .await;
    c.refused(
        "v2-rejected",
        fixed(),
        &with(&p2, FIXED),
        "strategy_revisit_rejected",
    )
    .await;

    // Project.request と承認済み Roadmap の行は変わらない。
    assert_eq!(c.human_owned(), human_before);
    assert_eq!(
        c.env
            .store
            .roadmap_approved(&c.project.id.to_string())
            .expect("roadmap")
            .map(|v| v.version),
        Some(1)
    );
    println!(
        "chain evaluate={evaluate_id} replan={replan_id} reason={reason_code} record={} v2={} experiment={}",
        record_ref["name"].as_str().unwrap_or_default(),
        p2["name"].as_str().unwrap_or_default(),
        x.id
    );
}

fn proposal(s: &ProjectSnapshot, action: Value, rule: &str, signals: &[(&str, &str)]) -> Value {
    json!({
        "action": action,
        "gap": {"reached": [], "open": s.outcomes.iter().map(|o| o.id.clone()).collect::<Vec<_>>(),
                "unknown": [], "progress_epoch": s.controller.progress_epoch},
        "explanation": {"rule_id": rule, "signals": signals.iter().map(|(n, d)| json!({"name": n, "detail": d})).collect::<Vec<_>>()},
        "based_on": {"revision": s.controller.revision, "human_epoch": s.controller.human_epoch,
                     "request_sha256": s.project.request_sha256},
    })
}
