//! ADR 2026-11-22-strategy-review-critic D7.3: 子 0 の replay の E-mid（cutoff 2026-11-18T20:36:35Z、子 A〜C が done）までの記録だけを
//! 使う shadow ケース。`POST /api/v1/cos/operations` の `task.create` を CoS の run の bearer で送り、本物の gate（`cos::strategy_gate`）と
//! store を通す。
//!
//! 守る不変条件:
//! - 入力は E-mid の `goal.md`・`events.jsonl`・`slice.json` の 3 file だけで、各 file は子 0 の `manifest.sha256` の E-mid の行と byte 単位で同じ、
//!   event の ts はすべて cutoff 以下（未来の記録を混ぜない）。cutoff より後の行を混ぜた入力は拒否する。
//! - 初回に大きな固定計画（子 A〜H の compound）を起票できず、open の候補ごとに 1 件の atomic の識別実験だけが通る。
//! - 実験の結果（E-mid の slice の中の文）を証拠に引き、固定計画の候補を反証で却下して別の候補を選んだ版だけが本実装（compound）を通す。
//! - 再評価の版は、却下した候補を選べず、前の版の終端の実験 task（失敗を含む）を全部証拠で参照しないと通らない。
//!
//! 既存の試験で検出できない理由: gate の `strategy_gate_review_` は試験用の短い文書で規則を 1 つずつ確かめ、task-core の
//! `strategy_review_contract_` は純関数だけを見る。どちらも「アーキv4 の途中の記録から 3 候補を立て、実験 → 選択 → 本実装 → 再評価と
//! 版を重ねる一続きの流れ」と、そのとき起票される task の数を見ない。Critic の LLM の判定はここでは動かさない（文書の task を done に置く）。

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::Router;
use common::cos_ops::{cos_run_bearer, post_op};
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::chat::ChatOpenThreadRequest;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::model::{ArtifactRef, Event, Task, TaskId};
use task_core::strategy;
use task_core::strategy_critic::critic_subjects;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// 子 0 の `replay.py build` が作った E-mid の slice の写し。
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/strategy_review_shadow"
);
/// 子 0 の slice の file の sha256 の正本。
const MANIFEST: &str =
    include_str!("../../../agent-docs/progress/2026-11-21-strategy-v1-0/replay/manifest.sha256");
const SLICE_ID: &str = "E-mid";
const FILES: [&str; 3] = ["events.jsonl", "goal.md", "slice.json"];
const CUTOFF: &str = "2026-11-18T20:36:35.314647904Z";

/// 実験の結果の証拠に引く E-mid の slice の中の文 `(task, seq, 欄, 文)`。
/// D の worker の報告（D#1446）と、B の後の循環の値（B#1119 の reviewer の判定。C の進捗の「17・198・756・1,827」と同じ値で、
/// C の進捗の file は 3 file の外の snapshot にあるので、events.jsonl の中のこの文を引く）。
const QUOTE_BOUNDARY: (&str, u64, &str, &str) = (
    "D",
    1446,
    "outcome",
    "未達/要判断として書いたもの: handlers ⇄ cos",
);
const QUOTE_CYCLES: (&str, u64, &str, &str) = ("B", 1119, "reason", "新 17 件・198・756・1,827");

const STRATEGY_ID: &str = "arch-v4-emid";
const FIXED: &str = "fixed-plan-a-to-h";
const CUT: &str = "cut-boundary-edges";
const RECOUNT: &str = "recount-baseline";
const PHASE: &str = "strategy_experiment_phase";
const REVISIT: &str = "strategy_revisit_rejected";
const UNREAD: &str = "strategy_failure_unread";
const APPROVER: &str = "human:shadow";

// ---- 入力（fixture）の検査 ----

fn manifest_sha(file: &str) -> Option<&'static str> {
    let want = format!("{SLICE_ID}/{file}");
    MANIFEST.lines().find_map(|line| {
        let (sha, path) = line.split_once("  ")?;
        (path == want).then_some(sha)
    })
}

fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn parse_ts(raw: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(raw, &Rfc3339).ok()
}

/// cutoff より後（または ts の読めない）行の `(task, seq, ts)`。
fn lines_after_cutoff(events: &str, cutoff: OffsetDateTime) -> Vec<(String, u64, String)> {
    events
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let row: Value = serde_json::from_str(line).unwrap_or(Value::Null);
            let ts = row["ts"].as_str().unwrap_or_default().to_string();
            let late = parse_ts(&ts).is_none_or(|t| t > cutoff);
            late.then(|| {
                (
                    row["task"].as_str().unwrap_or_default().to_string(),
                    row["seq"].as_u64().unwrap_or(u64::MAX),
                    ts,
                )
            })
        })
        .collect()
}

/// `dir` が E-mid の入力そのものでない理由（空なら入力として使える）: file の集合、manifest の sha256、
/// `slice.json` の id・cutoff・event の数、cutoff より後の行。
fn fixture_problems(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let names: BTreeSet<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let want: BTreeSet<String> = FILES.iter().map(|f| f.to_string()).collect();
    if names != want {
        out.push(format!("file の集合が {names:?} で {want:?} でない"));
    }
    for file in FILES {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
        match manifest_sha(file) {
            Some(sha) if sha == sha_hex(&bytes) => {}
            Some(_) => out.push(format!(
                "{file} の sha256 が manifest の {SLICE_ID}/{file} と違う"
            )),
            None => out.push(format!("manifest に {SLICE_ID}/{file} の行が無い")),
        }
    }
    let slice: Value = std::fs::read(dir.join("slice.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    if slice["id"] != SLICE_ID || slice["cutoff"] != CUTOFF {
        out.push(format!(
            "slice.json の id・cutoff が {SLICE_ID}・{CUTOFF} でない: {slice}"
        ));
    }
    let events = std::fs::read_to_string(dir.join("events.jsonl")).unwrap_or_default();
    let lines = events.lines().filter(|l| !l.trim().is_empty()).count();
    if slice["events"].as_u64() != Some(lines as u64) {
        out.push(format!(
            "events.jsonl の行が {lines} で slice.json の events と違う"
        ));
    }
    if let Some(cutoff) = parse_ts(CUTOFF) {
        for (task, seq, ts) in lines_after_cutoff(&events, cutoff) {
            out.push(format!("cutoff より後の行: {task}#{seq} {ts}"));
        }
    }
    out
}

/// 検査を通った E-mid の入力。
struct Slice {
    goal: String,
    events: Vec<Value>,
}

impl Slice {
    fn load() -> Self {
        let problems = fixture_problems(Path::new(FIXTURE));
        assert!(problems.is_empty(), "{problems:#?}");
        let read = |f: &str| std::fs::read_to_string(Path::new(FIXTURE).join(f)).expect(f);
        let events = read("events.jsonl")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("event"))
            .collect();
        Self {
            goal: read("goal.md"),
            events,
        }
    }

    /// `(task, seq)` の event の `field` に `text` があることを確かめて `text` を返す（証拠が slice の中の文を引く）。
    fn quote(&self, (task, seq, field, text): (&str, u64, &str, &'static str)) -> &'static str {
        let event = self
            .events
            .iter()
            .find(|e| e["task"] == task && e["seq"] == seq)
            .unwrap_or_else(|| panic!("{task}#{seq} が E-mid の events.jsonl に無い"));
        let body = event[field].as_str().unwrap_or_default();
        assert!(
            body.contains(text),
            "{task}#{seq}.{field} に「{text}」が無い: {body}"
        );
        text
    }

    /// 親の作成から 60 秒以内に作られた子（1 文字の task 名）の数。アーキv4 の初回の起票の数。
    fn children_filed_at_start(&self) -> usize {
        let created = |e: &&Value| e["type"] == "created";
        let ts = |e: &Value| parse_ts(e["ts"].as_str().unwrap_or_default()).expect("ts");
        let parent = self
            .events
            .iter()
            .filter(created)
            .find(|e| e["task"] == "P")
            .map(ts)
            .expect("parent created");
        self.events
            .iter()
            .filter(created)
            .filter(|e| {
                let name = e["task"].as_str().unwrap_or_default();
                name.len() == 1 && name != "P" && ts(e) <= parent + time::Duration::seconds(60)
            })
            .count()
    }
}

// ---- 案件・CoS の run・成果物 ----

struct Shadow {
    env: TestEnv,
    app: Router,
    bearer: String,
    project: task_core::Project,
    /// 子 0 の replay の E-mid の 3 file を成果物に持つ task（証拠の `artifact` の参照の先）。
    replay: Task,
    slice: Slice,
}

/// 作った task の workspace（無ければ作る）。gate の `Reader::file` と同じ `task_ops::workspace::local_dir` で解く。
fn workspace(env: &TestEnv, task: &Task) -> PathBuf {
    let dir = task_ops::workspace::local_dir(&env.store, &env.view_context().workspace_roots, task)
        .expect("workspace");
    std::fs::create_dir_all(&dir).expect("workspace dir");
    dir
}

/// `task` の workspace に file を書き、`ArtifactProduced` を積む。sha256 を返す。
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

fn applied_creates(env: &TestEnv) -> i64 {
    count(
        env,
        "SELECT COUNT(*) FROM cos_operations WHERE action = 'task.create' AND state = 'applied'",
    )
}

impl Shadow {
    fn new() -> Self {
        let slice = Slice::load();
        let env = admin_env();
        let app = env.router();
        let now = OffsetDateTime::now_utc();
        // 案件の request は E-mid の goal.md（アーキv4 の親の作成時の目的の全文）。
        let project = task_core::Project {
            id: task_core::ProjectId::new(),
            title: "agent-platform（shadow: アーキv4 E-mid）".into(),
            request: slice.goal.clone(),
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
        let bearer = cos_run_bearer(&env, &thread.id, "shadow");
        let mut replay = new_task(task_core::TaskKind::Execute, task_core::Status::Done);
        replay.title = "子 0 の replay の E-mid（入力）".into();
        replay.project_id = Some(project.id);
        env.seed(&replay);
        for file in FILES {
            let bytes = std::fs::read(Path::new(FIXTURE).join(file)).expect("fixture");
            let sha = produce(&env, &replay, "run-replay", &replay_name(file), &bytes);
            assert_eq!(Some(sha.as_str()), manifest_sha(file), "{file}");
        }
        Self {
            env,
            app,
            bearer,
            project,
            replay,
            slice,
        }
    }

    fn task(&self, id: &str) -> Task {
        let id: TaskId = id.parse().expect("task id");
        self.env.store.get(id).expect("get").expect("task exists")
    }

    /// 入力の file の成果物の参照（sha256 は manifest の E-mid の行）。
    fn replay_ref(&self, file: &str) -> Value {
        json!({"kind": "artifact", "task_id": self.replay.id.to_string(),
               "name": replay_name(file), "sha256": manifest_sha(file).expect("manifest")})
    }

    async fn send(&self, key: &str, body: Value, strategy: Option<&Value>) -> (u16, Value) {
        let mut op = json!({
            "idempotency_key": key,
            "expected_revision": null,
            "reason": "shadow: アーキv4 の E-mid からの Strategy Review",
            "policy_version": "1",
            "request": {"method": "POST", "path": "/api/v1/tasks", "body": body},
        });
        if let Some(s) = strategy {
            op["strategy"] = s.clone();
        }
        let resp = post_op(&self.app, &self.bearer, &op).await;
        (resp.status.as_u16(), resp.json())
    }

    /// 通り、作った task を返す。
    async fn passes(&self, key: &str, body: Value, strategy: Option<&Value>) -> Task {
        let (status, resp) = self.send(key, body, strategy).await;
        assert_eq!(status, 200, "{key}: {resp}");
        self.task(resp["operation"]["target_id"].as_str().expect("target_id"))
    }

    /// 422 で code の集合がちょうど `[want]`、task は作られない。
    async fn refused(&self, key: &str, body: Value, strategy: &Value, want: &str) {
        let before = tasks(&self.env);
        let (status, resp) = self.send(key, body, Some(strategy)).await;
        assert_eq!(status, 422, "{key}: {resp}");
        assert_eq!(resp["code"], want, "{key}: {resp}");
        assert_eq!(resp["codes"], json!([want]), "{key}: only {want}: {resp}");
        assert_eq!(tasks(&self.env), before, "{key}: a refusal created a task");
    }

    fn body(&self, title: &str, execution: Option<&str>, max_wall_secs: Option<u64>) -> Value {
        let mut b = json!({
            "title": title,
            "objective": format!("{title}（shadow: アーキv4 の E-mid の記録だけを使う）"),
            "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
            "project_id": self.project.id.to_string(),
        });
        if let Some(e) = execution {
            b["execution"] = json!(e);
        }
        if let Some(s) = max_wall_secs {
            b["max_wall_secs"] = json!(s);
        }
        b
    }

    /// CoS が文書の task を strategy 無しで起票し（D6 の手順 1）、その run が文書を書いて人が承認し、final review を通って
    /// done になった（D3.7。Critic の LLM の判定はこの試験では動かさない）。文書の参照 `{task_id, name, sha256}` を返す。
    async fn reviewed(&self, key: &str, doc: impl FnOnce(&str) -> Value) -> (Value, Value) {
        let version = key.trim_start_matches('v');
        let source = self
            .passes(
                &format!("doc-{key}"),
                self.body(
                    &format!("Strategy Review {key}: アーキv4 の戦略候補"),
                    None,
                    None,
                ),
                None,
            )
            .await;
        let run = format!("01M7SHADOWRUN{version:0>13}");
        let doc = doc(&run);
        let bytes = serde_json::to_vec_pretty(&doc).expect("json");
        let version = u32::try_from(doc["version"].as_u64().expect("version")).expect("u32");
        let name = strategy::artifact_name(STRATEGY_ID, version);
        let sha256 = produce(&self.env, &source, &run, &name, &bytes);
        let pointer = json!({"task_id": source.id.to_string(), "name": name, "sha256": sha256});
        approve(&self.env, &source, version, &sha256);
        set_status(&self.env, &source, "done");
        let subjects = critic_subjects(&self.env.store.events_for(source.id).expect("events"));
        assert_eq!(subjects.len(), 1, "{subjects:?}");
        assert!(subjects[0].name == name && !subjects[0].conflict);
        (doc, pointer)
    }

    /// 実験 task の結果を成果物に書き、終端にする。
    fn finish(&self, task: &Task, status: &str, result: &str) -> Value {
        let sha = produce(
            &self.env,
            task,
            &format!("run-{}", task.id),
            "result.md",
            result.as_bytes(),
        );
        set_status(&self.env, task, status);
        json!({"kind": "artifact", "task_id": task.id.to_string(), "name": "result.md", "sha256": sha})
    }
}

fn replay_name(file: &str) -> String {
    format!("replay/{SLICE_ID}/{file}")
}

fn with_candidate(pointer: &Value, candidate: Option<&str>) -> Value {
    let mut s = pointer.clone();
    if let Some(c) = candidate {
        s["candidate"] = json!(c);
    }
    s
}

fn task_ref(task: &Task) -> Value {
    json!({"kind": "task", "task_id": task.id.to_string()})
}

/// 人（`human:`）が文書の task の決定 `strategy-approve-v<n>` に approve と答えた（子 2 D2.2）。
fn approve(env: &TestEnv, source: &Task, version: u32, sha256: &str) {
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
                id,
                option: strategy::APPROVE_OPTION.into(),
                note: None,
                by: APPROVER.into(),
            },
        )
        .expect("answer");
}

// ---- 文書 ----

fn run_source() -> Value {
    json!({"kind": "celeris_task_run", "label": "Strategy Review の run"})
}

fn evidence(id: &str, stance: &str, hypothesis: &str, statement: &str, refs: Vec<Value>) -> Value {
    json!({
        "id": id, "stance": stance, "hypothesis": hypothesis, "statement": statement,
        "observed_at": "2026-11-18T20:36:35Z",
        "source": {"kind": "celeris_task_run", "label": "子 0 の replay の E-mid と識別実験"},
        "references": refs,
    })
}

/// 候補 `id`（open、selected 無し）。
#[allow(clippy::too_many_arguments)]
fn candidate(
    id: &str,
    title: &str,
    approach: &str,
    cost: (f64, f64),
    hypothesis: &str,
    experiment: &str,
    budget: (f64, f64),
    value: &str,
    stop_when: &str,
) -> Value {
    json!({
        "id": id, "title": title, "approach": approach,
        "decision_method": format!("識別実験の結果で {hypothesis} が支持されれば採り、反証されれば捨てる"),
        "cost": {"usd_max": cost.0, "wall_hours_max": cost.1},
        "risks": ["結果を待つ間に親の予算を使う"],
        "reevaluate_when": ["識別実験が終わった"],
        "hypotheses": [hypothesis],
        "selected": false,
        "state": "open",
        "experiment": {"content": experiment, "discriminates": [hypothesis],
                       "budget": {"usd_max": budget.0, "wall_hours_max": budget.1}},
        "expected_information_value": value,
        "stop_when": [stop_when],
    })
}

/// 実験の予算（時間）。起票の `max_wall_secs` はこれ以下（D2.4 の 4）。
const EXPERIMENT_HOURS: [(&str, f64); 3] = [(FIXED, 2.0), (CUT, 4.0), (RECOUNT, 3.0)];

/// v1: 候補 3 件（子 A〜H の固定計画と、識別実験を先に行う 2 案）。selected 無し（実験段階）。
fn document_v1(sh: &Shadow, run: &str) -> Value {
    let hours = |id: &str| {
        EXPERIMENT_HOURS
            .iter()
            .find(|(c, _)| *c == id)
            .map(|(_, h)| *h)
            .expect("hours")
    };
    json!({
        "schema": strategy::SCHEMA,
        "project_id": sh.project.id.to_string(),
        "strategy_id": STRATEGY_ID,
        "version": 1,
        "supersedes": null,
        "idempotency_key": strategy::idempotency_key(STRATEGY_ID, 1),
        "source": run_source(),
        "actor": {"kind": "worker", "id": run},
        "source_revision": {"request_sha256": sha_hex(sh.project.request.as_bytes()), "roadmap_version": null},
        "timestamp": "2026-11-18T20:40:00Z",
        "acl": {"classification": "internal", "outbound": false},
        "decision_state": "proposed",
        "research_question": {
            "text": "アーキv4 の親の目的（goal.md の共通受入条件 (1)〜(6)）に、子 A〜H の固定の計画で届くか。子 A〜C が done の E-mid の記録から、どの戦略なら循環を減らせるか",
            "references": [sh.replay_ref("goal.md")],
        },
        "candidates": [
            candidate(FIXED, "子 A〜H の固定の計画を続ける",
                "D〜H を計画どおり順に進める（アーキv4 が実際に採った計画）", (400.0, 80.0), "h-fixed-plan",
                "子 A〜C の記録（循環の件数・単位・組と D の報告）だけを読み、計画を続けたとき (1) が減る兆しがあるかを確かめる",
                (10.0, hours(FIXED)),
                "循環の件数が減る兆しが無ければ h-fixed-plan を反証し、この候補を捨てる。兆しがあれば E を続ける",
                "子 A〜C の後に循環の件数が 17 のまま"),
            candidate(CUT, "handlers ⇄ cos の境界の辺を先に切る",
                "D が残した境界の辺だけを切る pilot を 1 つ走らせ、循環の単位を測る", (60.0, 12.0), "h-boundary-edges",
                "D の報告と進捗から境界の辺を特定し、切る候補の辺の数を数える", (20.0, hours(CUT)),
                "辺が少数に絞れれば h-boundary-edges を支持し、この候補を選ぶ",
                "切る候補の辺が 10 本を超える"),
            candidate(RECOUNT, "A の command で循環を数え直してから決める",
                "A の §7 の command で A〜C の循環を数え直し、数え方の違いを除いてから計画を決める", (30.0, 6.0), "h-measure-first",
                "A の §7 の command で A〜C の循環を数え直す", (10.0, hours(RECOUNT)),
                "数え直しで値が変われば h-measure-first を支持する。同じなら捨てる",
                "数え直しが予算の時間で終わらない"),
        ],
        "held_assumptions": [{"id": "a-count-rule", "statement": "循環の数え方は A の §7 の command のまま変わっていない",
                              "revisit_when": "B〜H が A と違う数え方を書いた"}],
        "hypotheses": [
            {"id": "h-fixed-plan", "statement": "子 A〜H の固定の計画を順に進めれば、共通受入条件 (1) の循環の件数と単位が減る",
             "confidence": "estimated", "falsified_by": "子 A〜C の後に循環の件数が 17 のままで、D が境界の循環を未達と書く",
             "evidence": ["e-plan-filed"]},
            {"id": "h-boundary-edges", "statement": "handlers ⇄ cos のような少数の境界の辺が循環の大半を作っている",
             "confidence": "estimated", "falsified_by": "境界の辺を切る pilot の後も循環の単位が減らない",
             "evidence": ["e-boundary-target"]},
            {"id": "h-measure-first", "statement": "子ごとに循環の数え方が違い、改善が見えていない",
             "confidence": "needs_decision", "falsified_by": "A の command で数え直しても値が同じ",
             "evidence": ["e-measure-rule"]},
        ],
        "evidence": [
            evidence("e-plan-filed", "unknown", "h-fixed-plan",
                "親は子 A〜H を作成の直後に全部起票した。計画の効果は E-mid の時点で未測定",
                vec![sh.replay_ref("events.jsonl")]),
            evidence("e-boundary-target", "unknown", "h-boundary-edges",
                "D の対象は handlers/org.rs と cos/ops/admin.rs の循環（D の作成の event の題名）",
                vec![sh.replay_ref("events.jsonl")]),
            evidence("e-measure-rule", "unknown", "h-measure-first",
                "B〜H は A の計測コマンドと規則をそのまま使う（goal.md の共通の規則）",
                vec![sh.replay_ref("goal.md")]),
        ],
    })
}

/// 版 `prev` の次の版の骨組み（版・idempotency・supersedes・change_reason）。
fn next_version(prev: &Value, pointer: &Value, run: &str, reason: &str) -> Value {
    let version = prev["version"].as_u64().expect("version") + 1;
    let mut doc = prev.clone();
    doc["version"] = json!(version);
    doc["idempotency_key"] = json!(strategy::idempotency_key(
        STRATEGY_ID,
        u32::try_from(version).expect("u32")
    ));
    doc["supersedes"] = pointer.clone();
    doc["change_reason"] = json!(reason);
    doc["actor"]["id"] = json!(run);
    doc
}

fn candidate_mut<'a>(doc: &'a mut Value, id: &str) -> &'a mut Value {
    doc["candidates"]
        .as_array_mut()
        .expect("candidates")
        .iter_mut()
        .find(|c| c["id"] == id)
        .expect("candidate")
}

fn evidence_mut<'a>(doc: &'a mut Value, id: &str) -> &'a mut Value {
    doc["evidence"]
        .as_array_mut()
        .expect("evidence")
        .iter_mut()
        .find(|e| e["id"] == id)
        .expect("evidence")
}

/// 3 件の識別実験の task と結果の成果物の参照。
struct Experiments {
    fixed: (Task, Value),
    cut: (Task, Value),
    recount: (Task, Value),
}

/// v2: 実験の結果を読み、固定計画の候補を反証（E-mid の文を引く）で却下し、境界の辺を切る候補を選ぶ。
/// 終端の実験 task 3 件（失敗を含む）を全部証拠で参照する。
fn document_v2(sh: &Shadow, v1: &Value, p1: &Value, run: &str, x: &Experiments) -> Value {
    let boundary = sh.slice.quote(QUOTE_BOUNDARY);
    let cycles = sh.slice.quote(QUOTE_CYCLES);
    let mut doc = next_version(
        v1,
        p1,
        run,
        "識別実験 3 件の結果を読み、固定の計画を反証で却下して境界の辺を切る候補を選んだ",
    );
    let fixed = candidate_mut(&mut doc, FIXED);
    fixed["state"] = json!("rejected");
    fixed["rejected_by"] = json!(["e-fixed-refuted"]);
    candidate_mut(&mut doc, CUT)["selected"] = json!(true);
    let hypotheses = doc["hypotheses"].as_array_mut().expect("hypotheses");
    hypotheses[0]["confidence"] = json!("contradicted");
    hypotheses[0]["evidence"] = json!(["e-plan-filed", "e-fixed-refuted"]);
    hypotheses[1]["evidence"] = json!(["e-boundary-target", "e-boundary-named"]);
    hypotheses[2]["evidence"] = json!(["e-measure-rule", "e-recount-failed"]);
    let list = doc["evidence"].as_array_mut().expect("evidence");
    list.push(evidence(
        "e-fixed-refuted",
        "refute",
        "h-fixed-plan",
        &format!(
            "子 A〜C の後の循環は「{cycles}」（B#1119）で件数が 17 のまま単位と組が増え、D は「{boundary}」（D#1446）と書いた。固定の計画を続けても (1) は減らない"
        ),
        vec![
            task_ref(&x.fixed.0),
            x.fixed.1.clone(),
            sh.replay_ref("events.jsonl"),
        ],
    ));
    list.push(evidence(
        "e-boundary-named",
        "support",
        "h-boundary-edges",
        &format!("D#1446 が残りの循環を「{boundary}」と名指しした。切る辺の候補が決まった"),
        vec![
            task_ref(&x.cut.0),
            x.cut.1.clone(),
            sh.replay_ref("events.jsonl"),
        ],
    ));
    list.push(evidence(
        "e-recount-failed",
        "unknown",
        "h-measure-first",
        "数え直しの実験 task は予算の時間で終わらず失敗した。数え方の前提 a-count-rule は保留のまま",
        vec![task_ref(&x.recount.0), x.recount.1.clone()],
    ));
    doc
}

/// 戦略を選んで本実装を起票した状態。
struct Selected {
    sh: Shadow,
    v2: Value,
    p2: Value,
    x: Experiments,
    implementation: Task,
}

/// D7.3 の筋書きの v1 → 実験 → v2 → 本実装。各段の結果を確かめ、数を出す。
async fn select_after_experiments() -> Selected {
    let sh = Shadow::new();
    let a_to_h = sh.slice.children_filed_at_start();
    // v1（候補 3 件、selected 無し）を人が承認し、文書の task が final review を通って done。
    let (v1, p1) = sh.reviewed("v1", |run| document_v1(&sh, run)).await;
    // 初回に子 A〜H の固定計画（compound）を起票しようとすると、実験段階で拒否される（候補の指定の有無によらない）。
    let plan = sh.body(
        "[アーキv4 A〜H] 固定の計画を全面起票",
        Some("compound"),
        None,
    );
    sh.refused(
        "a-to-h",
        plan.clone(),
        &with_candidate(&p1, Some(FIXED)),
        PHASE,
    )
    .await;
    sh.refused("a-to-h-bare", plan, &p1, PHASE).await;
    // open の候補ごとに atomic の識別実験 1 件だけが通る。同じ候補の 2 件目は拒否。
    let mut started = Vec::new();
    for (id, hours) in EXPERIMENT_HOURS {
        let secs = (hours * 3600.0) as u64;
        let body = sh.body(&format!("識別実験: {id}"), Some("atomic"), Some(secs));
        started.push(
            sh.passes(
                &format!("x-{id}"),
                body,
                Some(&with_candidate(&p1, Some(id))),
            )
            .await,
        );
    }
    sh.refused(
        "x-fixed-again",
        sh.body("識別実験: 2 件目", Some("atomic"), None),
        &with_candidate(&p1, Some(FIXED)),
        PHASE,
    )
    .await;
    // 実験の結果。固定計画と境界の辺の実験は done、数え直しは失敗（失敗の記録も次の版が読む）。
    let boundary = sh.slice.quote(QUOTE_BOUNDARY);
    let cycles = sh.slice.quote(QUOTE_CYCLES);
    let [fixed, cut, recount]: [Task; 3] = started.try_into().expect("3 experiments");
    let x = Experiments {
        fixed: (
            fixed.clone(),
            sh.finish(
                &fixed,
                "done",
                &format!("B#1119: {cycles}\nD#1446: {boundary}\n"),
            ),
        ),
        cut: (
            cut.clone(),
            sh.finish(
                &cut,
                "done",
                &format!("D#1446: {boundary}\n切る辺の候補: handlers ⇄ cos\n"),
            ),
        ),
        recount: (
            recount.clone(),
            sh.finish(&recount, "failed", "数え直しは予算の時間で終わらなかった\n"),
        ),
    };
    // v2: 固定計画を rejected_by 付きで却下し、境界の辺の候補を selected にして実験 task を全部参照。
    let (v2, p2) = sh
        .reviewed("v2", |run| document_v2(&sh, &v1, &p1, run, &x))
        .await;
    let before_selection = applied_creates(&sh.env);
    println!(
        "strategy_review_shadow: 戦略を選ぶまでに起票した task = {before_selection}（文書の task 2・識別実験 3）。アーキv4 の初回の起票 = 子 {a_to_h} 件"
    );
    assert_eq!(before_selection, 5);
    assert_eq!(a_to_h, 8);
    // 選んだ版では、却下した候補の起票は拒否、compound の本実装（候補は省略）は通る。
    let main = sh.body(
        "本実装: handlers ⇄ cos の境界の辺を切る",
        Some("compound"),
        None,
    );
    sh.refused(
        "impl-fixed",
        main.clone(),
        &with_candidate(&p2, Some(FIXED)),
        REVISIT,
    )
    .await;
    let implementation = sh.passes("impl", main, Some(&p2)).await;
    let rejected = count(
        &sh.env,
        "SELECT COUNT(*) FROM cos_operations WHERE state = 'rejected'",
    );
    println!(
        "strategy_review_shadow: 拒否 {rejected} 件（実験段階 3・却下した候補 1）、本実装を含む起票 {}",
        applied_creates(&sh.env)
    );
    assert_eq!(rejected, 4);
    assert_eq!(applied_creates(&sh.env), 6);
    Selected {
        sh,
        v2,
        p2,
        x,
        implementation,
    }
}

/// 入力の検査: fixture の dir はちょうど E-mid の 3 file で、各 file の sha256 は子 0 の manifest の行と同じ、event の ts は
/// すべて cutoff 以下。証拠に引く文が slice の中に実在し、アーキv4 の初回の起票は子 8 件（親の作成から 11 秒以内）。
#[test]
fn strategy_review_shadow_fixture_is_e_mid_only() {
    assert!(fixture_problems(Path::new(FIXTURE)).is_empty());
    let slice = Slice::load();
    assert_eq!(slice.events.len(), 870);
    let cutoff = parse_ts(CUTOFF).expect("cutoff");
    assert!(
        slice
            .events
            .iter()
            .all(|e| parse_ts(e["ts"].as_str().unwrap_or_default()).is_some_and(|t| t <= cutoff))
    );
    slice.quote(QUOTE_BOUNDARY);
    slice.quote(QUOTE_CYCLES);
    assert_eq!(slice.children_filed_at_start(), 8);
}

/// 負の試験: cutoff より後の行を 1 行混ぜた入力は、その行を名指しして拒否する（manifest の sha256 とも食い違う）。
/// cutoff ちょうどの行は拒否しない。余分な file（評価の問いなど）を置いた dir も拒否する。
#[test]
fn strategy_review_shadow_rejects_input_with_lines_after_cutoff() {
    let cutoff = parse_ts(CUTOFF).expect("cutoff");
    let events = std::fs::read_to_string(Path::new(FIXTURE).join("events.jsonl")).expect("events");
    let last: Value = serde_json::from_str(events.lines().last().expect("line")).expect("json");
    assert_eq!(last["ts"], CUTOFF, "the last line is exactly at the cutoff");
    assert!(lines_after_cutoff(&events, cutoff).is_empty());
    let mut late = last.clone();
    late["seq"] = json!(999_999);
    late["ts"] = json!("2026-11-18T20:36:35.314647905Z");
    let mixed = format!("{events}{late}\n");
    assert_eq!(
        lines_after_cutoff(&mixed, cutoff),
        vec![(
            last["task"].as_str().expect("task").to_string(),
            999_999,
            "2026-11-18T20:36:35.314647905Z".to_string()
        )]
    );
    let tmp = tempfile::tempdir().expect("tempdir");
    for file in FILES {
        std::fs::copy(Path::new(FIXTURE).join(file), tmp.path().join(file)).expect("copy");
    }
    assert!(fixture_problems(tmp.path()).is_empty());
    std::fs::write(tmp.path().join("events.jsonl"), &mixed).expect("write");
    let problems = fixture_problems(tmp.path());
    assert!(
        problems.iter().any(|p| p.contains("cutoff より後の行")
            && p.contains("#999999 2026-11-18T20:36:35.314647905Z")),
        "{problems:#?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("events.jsonl の sha256")),
        "{problems:#?}"
    );
    std::fs::write(tmp.path().join("events.jsonl"), &events).expect("restore");
    std::fs::write(tmp.path().join("question.md"), "評価の問い").expect("extra");
    let problems = fixture_problems(tmp.path());
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(problems[0].contains("file の集合"), "{problems:#?}");
}

/// 筋書き（D7.3）: v1 の承認 → A〜H の compound は `strategy_experiment_phase` → open の候補ごとの atomic の実験 3 件は通り、
/// 同じ候補の 2 件目は拒否 → 実験の結果（E-mid の文）を証拠に引いた v2 が固定計画を却下して別の候補を選ぶ → 本実装の compound が通る。
/// 戦略を選ぶまでに起票した task は 5 件（アーキv4 の初回は子 8 件）。
#[tokio::test]
async fn strategy_review_shadow_experiments_first_then_select() {
    let s = select_after_experiments().await;
    // 本実装の起票の監査に、選んだ版の参照が残る（どの戦略の実行か辿れる）。
    let payload: String = rusqlite::Connection::open(&s.sh.env.db_path)
        .expect("db")
        .query_row(
            "SELECT payload_json FROM cos_operations WHERE target_id = ?1",
            [s.implementation.id.to_string()],
            |r| r.get(0),
        )
        .expect("payload");
    let payload: Value = serde_json::from_str(&payload).expect("json");
    assert_eq!(payload["strategy"], s.p2);
    // v2 の証拠は 3 件の実験 task（失敗を含む）を全部参照し、却下の根拠は反証の証拠を指す。
    let cited: BTreeSet<String> = s.v2["evidence"]
        .as_array()
        .expect("evidence")
        .iter()
        .flat_map(|e| e["references"].as_array().cloned().unwrap_or_default())
        .filter(|r| r["kind"] == "task")
        .filter_map(|r| r["task_id"].as_str().map(str::to_string))
        .collect();
    for (task, _) in [&s.x.fixed, &s.x.cut, &s.x.recount] {
        assert!(cited.contains(&task.id.to_string()), "{cited:?}");
    }
    let fixed = s.v2["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .find(|c| c["id"] == FIXED)
        .expect("fixed");
    assert_eq!(fixed["state"], "rejected");
    assert_eq!(fixed["rejected_by"], json!(["e-fixed-refuted"]));
}

/// 再評価（本実装が失敗した後）の v3 が、却下した固定計画の候補を selected にすると `strategy_revisit_rejected` だけで拒否され、
/// task は作られない。v3 は終端の task（実験 3・本実装 1）を全部参照し、人が承認し、文書の task は done（ほかの規則には当たらない）。
#[tokio::test]
async fn strategy_review_shadow_revisit_rejected_after_failure() {
    let s = select_after_experiments().await;
    let sh = &s.sh;
    let failed = sh.finish(
        &s.implementation,
        "failed",
        "境界の辺を切っても循環の単位が減らなかった\n",
    );
    let (v3, p3) = sh
        .reviewed("v3", |run| {
            let mut doc = next_version(
                &s.v2,
                &s.p2,
                run,
                "本実装が失敗したので固定の計画に戻す（却下した候補）",
            );
            candidate_mut(&mut doc, CUT)["selected"] = json!(false);
            candidate_mut(&mut doc, FIXED)["selected"] = json!(true);
            let refs = evidence_mut(&mut doc, "e-boundary-named")["references"]
                .as_array_mut()
                .expect("refs");
            refs.push(task_ref(&s.implementation));
            refs.push(failed);
            doc
        })
        .await;
    assert_eq!(candidate_mut(&mut v3.clone(), FIXED)["state"], "rejected");
    sh.refused(
        "v3-revisit",
        sh.body("固定の計画に戻る", Some("compound"), None),
        &p3,
        REVISIT,
    )
    .await;
}

/// 再評価の v3 が、前の版の終端の実験 task（失敗した数え直し）の参照を落とすと `strategy_failure_unread` だけで拒否される。
/// 同じ v3 の形で全部を参照すれば通ることは revisit の試験と筋書きの v2 が示す。
#[tokio::test]
async fn strategy_review_shadow_dropped_terminal_experiment_is_unread() {
    let s = select_after_experiments().await;
    let sh = &s.sh;
    let done = sh.finish(
        &s.implementation,
        "done",
        "境界の辺を切り、循環の単位を測った\n",
    );
    let recount = s.x.recount.0.id.to_string();
    let (v3, p3) = sh
        .reviewed("v3", |run| {
            let mut doc = next_version(&s.v2, &s.p2, run, "本実装の結果を読み、同じ候補で続ける");
            let refs = evidence_mut(&mut doc, "e-boundary-named")["references"]
                .as_array_mut()
                .expect("refs");
            refs.push(task_ref(&s.implementation));
            refs.push(done);
            // 失敗した実験 task の参照を落とす（結果の file の参照も task の参照も）。
            evidence_mut(&mut doc, "e-recount-failed")["references"] =
                json!([sh.replay_ref("goal.md")]);
            doc
        })
        .await;
    let text = v3.to_string();
    assert!(!text.contains(&recount), "v3 still cites {recount}");
    sh.refused(
        "v3-unread",
        sh.body("本実装の続き", Some("compound"), None),
        &p3,
        UNREAD,
    )
    .await;
}
