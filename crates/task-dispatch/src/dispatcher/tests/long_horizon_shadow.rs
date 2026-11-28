//! ADR 2026-11-27-r1-stall-shadow D4・D5: long-horizon bench の腕 `shadow`（接頭辞 `long_horizon_shadow_`）。
//!
//! 案件 mode `shadow`、本物の control-ts の bundle、`old.proactive` と同じ台本の CoS（起票は CoS）。R1 の
//! `parent_checkpoint::stall` は strategy root の直下の子しか数えず、Outcome の到達は案件の root（`parent_id` の無い task）
//! しか根拠にしない。台本の root は案件の root なので、腕 `shadow` は試行の始めに strategy root を 1 件置き
//! （`cos_operations` の `task.create` に strategy の参照を持つ行）、台本の root 1 件ごとに strategy root の直下の子
//! （写し）を 1 件作って、root が終端になった仮想時刻に同じ終端へ遷移させる。これで R1 の写像（案件 = 親、root = 子。
//! r1-bench と同じ）を本物の snapshot → TS → Rust の門に通す。
//!
//! 試験が確かめること:
//! - 効果 0: daemon が起票した root・controller の問い（`project_controller:` の受信箱の源）・CoS 入力・効果のある判断が 0 件で、
//!   判断は記録され（1 件以上）効果は抑えられる（`suppressed` が 1 件以上）。台本の木・人の操作・受信箱の通知の数は同じ課題の
//!   `old.proactive` と一致する。
//! - 同じ seed の 2 回で記録の bytes が一致する。
//!
//! `LONG_HORIZON_BENCH_RAW_DIR` があれば `shadow/<case>.json`（`celeris.long-horizon-shadow/1`: 判断の rule id と時点、
//! root の遷移の時刻、Outcome ごとの到達の時刻、TS に渡った `stall`、効果の数）を書く。集計は
//! `agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.py` が行う。

use super::*;
use task_core::chat::{AuditContext, ChatActor};

/// strategy root の参照の name（`strategy_critic::strategy_artifact` が読める形）。
const STRATEGY_NAME: &str = "strategy/shadow/v1.json";

pub(super) struct Shadow {
    strategy_root: TaskId,
    /// 台本の root → strategy root の直下の子（写し）。
    mirrors: BTreeMap<TaskId, TaskId>,
    /// 台本の root ごとの状態と時刻（試行の始めからの秒）。
    status_at: BTreeMap<TaskId, Vec<(String, i64)>>,
    /// Outcome ごとの最初の到達（台本の木の判定。秒）。
    reached_at: BTreeMap<String, i64>,
    /// TS に渡った strategy root の `stall`（値が変わった時点だけ）。
    stalls: Vec<Value>,
}

impl Trial<'_> {
    /// 試行の始め（mode を置いた後、daemon の前）に strategy root を置く。
    pub(super) fn shadow_start(&mut self) {
        let now = self.env.now();
        let mut s = new_task(self.env.dir.path(), Check::Reviewer, 0);
        s.project_id = Some(self.p.id);
        s.title = format!("{}: strategy root (shadow)", self.p.title);
        s.acceptance = vec![Criterion {
            text: "R1 の shadow の strategy root".into(),
            check: Check::Reviewer,
        }];
        s.status = Status::Ready;
        self.store.create_task(&s, vec![]).unwrap();
        let op = ulid::Ulid::new().to_string();
        let ctx = AuditContext {
            actor: ChatActor::Cos,
            thread_id: "bench".into(),
            run_id: "bench".into(),
            operation_id: op.clone(),
            reason: "long-horizon shadow の strategy root".into(),
            policy_version: "v1".into(),
        };
        let payload = json!({
            "method": "POST", "path": "/api/v1/tasks", "body": {"title": s.title},
            "strategy": {"task_id": s.id.to_string(), "name": STRATEGY_NAME,
                         "sha256": format!("sha256:{}", sha(STRATEGY_NAME)), "candidate": null}
        });
        self.store
            .cos_operation_apply_at(
                &ctx,
                &op,
                "bench",
                "task",
                &s.id.to_string(),
                None,
                "task.create",
                &payload,
                now,
                |_, _| Ok(json!({})),
            )
            .unwrap();
        if let Some(ts) = &self.ts {
            ts.checkpoints.lock().unwrap().clear();
        }
        self.shadow = Some(Shadow {
            strategy_root: s.id,
            mirrors: BTreeMap::new(),
            status_at: BTreeMap::new(),
            reached_at: BTreeMap::new(),
            stalls: Vec::new(),
        });
    }

    /// strategy root か写しの子か（台本の root として拾わない）。
    pub(super) fn shadow_owns(&self, id: TaskId) -> bool {
        self.shadow
            .as_ref()
            .is_some_and(|s| s.strategy_root == id || s.mirrors.values().any(|m| *m == id))
    }

    /// 台本の root を記録した直後: 写しの子を作る。
    pub(super) fn shadow_track(&mut self, id: TaskId) {
        let Some(parent) = self.shadow.as_ref().map(|s| s.strategy_root) else {
            return;
        };
        let mut c = new_task(self.env.dir.path(), Check::Reviewer, 0);
        c.parent_id = Some(parent);
        c.project_id = Some(self.p.id);
        c.title = format!("mirror of {}", self.roots[&id].logical);
        c.status = Status::Ready;
        self.store.create_task(&c, vec![]).unwrap();
        let now = self.secs();
        let sh = self.shadow.as_mut().unwrap();
        sh.mirrors.insert(id, c.id);
        sh.status_at.insert(id, vec![("ready".into(), now)]);
    }

    /// 台本の root の遷移: 時刻を残し、終端なら写しの子を同じ仮想時刻に同じ終端へ遷移させる。
    pub(super) fn shadow_status(&mut self, id: TaskId, to: Status) {
        let now = self.secs();
        let at = self.env.now();
        let Some(sh) = self.shadow.as_mut() else {
            return;
        };
        let Some(mirror) = sh.mirrors.get(&id).copied() else {
            return;
        };
        let s = serde_json::to_value(to).unwrap();
        let s = s.as_str().unwrap().to_owned();
        sh.status_at.get_mut(&id).unwrap().push((s.clone(), now));
        if !to.is_terminal() {
            return;
        }
        let conn = rusqlite::Connection::open(self.env.db()).unwrap();
        conn.execute(
            "UPDATE tasks SET status = ?1, json = json_set(json, '$.status', ?1) WHERE id = ?2",
            rusqlite::params![s, mirror.to_string()],
        )
        .unwrap();
        drop(conn);
        self.store
            .write_tx::<_, task_core::StoreError, _>(|tx| {
                SqliteStore::append_event_tx_at(
                    tx,
                    mirror,
                    &Event::Transitioned {
                        from: Status::Ready,
                        to,
                        reason: "bench_mirror".into(),
                    },
                    at,
                )
            })
            .unwrap();
    }

    /// tick の後: TS に渡った snapshot の strategy root の `stall` を読み出す（値が変わったときだけ残す）。
    pub(super) fn shadow_observe(&mut self) {
        let (Some(sh), Some(ts)) = (self.shadow.as_mut(), self.ts.as_ref()) else {
            return;
        };
        let now = (self.env.now() - self.t0).whole_seconds();
        let calls: Vec<Value> = ts.checkpoints.lock().unwrap().drain(..).collect();
        let root = sh.strategy_root.to_string();
        for call in calls {
            let stall = call
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["root"] == root.as_str()))
                .map(|c| c["stall"].clone())
                .unwrap_or(Value::Null);
            // `at` は大域の event id で試行の外では意味が無いので落とす。
            let stall = match stall {
                Value::Object(mut o) => {
                    o.remove("at");
                    Value::Object(o)
                }
                other => other,
            };
            if sh.stalls.last().map(|s| &s["stall"]) != Some(&stall) {
                sh.stalls.push(json!({"at": now, "stall": stall}));
            }
        }
    }

    /// step の終わり: Outcome ごとの最初の到達（台本の木）を残す。
    pub(super) fn shadow_reach(&mut self) {
        if self.shadow.is_none() {
            return;
        }
        let now = self.secs();
        let ids: Vec<String> = self.sc.outcomes.iter().map(|o| o.id.clone()).collect();
        for id in ids {
            if self.reached(&id) {
                self.shadow
                    .as_mut()
                    .unwrap()
                    .reached_at
                    .entry(id)
                    .or_insert(now);
            }
        }
    }

    /// 判断の行（記録の順）: 時点・rule id（行の `validation`。TS の規則・Rust の guard・門の置き換えのどれでも最終の id）・
    /// TS の規則 id（TS を呼んだ行だけ）・guard の名前・行動の種類・key・効果の状態。id は logical に置き換える。
    fn shadow_decisions(&self) -> Vec<Value> {
        let sh = self.shadow.as_ref().unwrap();
        let conn = rusqlite::Connection::open(self.env.db()).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT created_at, validation, proposal_json, effect_json, effect_state \
                 FROM project_controller_decisions WHERE project_id = ?1 ORDER BY rowid",
            )
            .unwrap();
        let rows: Vec<(String, String, String, String, String)> = stmt
            .query_map([self.p.id.to_string()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let logical = |text: &str| {
            let mut t = text.replace(&sh.strategy_root.to_string(), "S");
            for (id, r) in &self.roots {
                t = t.replace(&id.to_string(), &r.logical);
            }
            t.replace(&self.p.id.to_string(), "<project>")
        };
        rows.into_iter()
            .map(|(created_at, rule, proposal, effect_json, effect)| {
                let at = OffsetDateTime::parse(&created_at, &Rfc3339).unwrap();
                let rec: Value = serde_json::from_str(&proposal).unwrap_or(Value::Null);
                let action = &rec["proposal"]["action"];
                let fx: Value = serde_json::from_str(&effect_json).unwrap_or(Value::Null);
                let fx_kind = match &fx {
                    Value::String(k) => Some(k.clone()),
                    Value::Object(o) => o
                        .get("kind")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| o.keys().next().cloned()),
                    _ => None,
                };
                let key = action["question_key"]
                    .as_str()
                    .or(action["outcome_id"].as_str())
                    .or(action["root"]["key"].as_str())
                    .map(logical);
                json!({
                    "at": (at - self.t0).whole_seconds(),
                    "rule_id": logical(&rule),
                    "ts_rule_id": rec["proposal"]["explanation"]["rule_id"].as_str(),
                    "guard": rec["guard"].as_str(),
                    "kind": action["kind"].as_str().map(str::to_owned).or(fx_kind),
                    "key": key,
                    "effect_state": effect,
                })
            })
            .collect()
    }

    /// 受信箱の通知の数（腕 `old.proactive` と比べる）。
    fn feed_notices(&self) -> i64 {
        self.env.count("SELECT COUNT(*) FROM feed_notices")
    }

    /// `celeris.long-horizon-shadow/1` の記録。
    fn shadow_record(&self) -> Value {
        let sh = self.shadow.as_ref().unwrap();
        let roots: Vec<Value> = self
            .order
            .iter()
            .map(|id| {
                let r = &self.roots[id];
                let statuses: Vec<Value> = sh.status_at[id]
                    .iter()
                    .map(|(s, at)| json!({"status": s, "at": at}))
                    .collect();
                json!({
                    "logical_id": r.logical, "outcome": r.outcome, "created_by": r.created_by,
                    "created_at": r.created_at, "statuses": statuses,
                })
            })
            .collect();
        let outcomes: Vec<Value> = self
            .sc
            .outcomes
            .iter()
            .map(|o| {
                json!({
                    "id": o.id,
                    "required": o.required && self.current_outcomes.contains(&o.id),
                    "reached_at": sh.reached_at.get(&o.id),
                })
            })
            .collect();
        let decisions = self.shadow_decisions();
        let with_effect = decisions
            .iter()
            .filter(|d| !matches!(d["effect_state"].as_str(), Some("none" | "suppressed")))
            .count();
        let suppressed = decisions
            .iter()
            .filter(|d| d["effect_state"] == "suppressed")
            .count();
        let pid = self.p.id.to_string();
        json!({
            "schema": "celeris.long-horizon-shadow/1",
            "case": self.sc.case,
            "seed": 1,
            "horizon_secs": self.sc.horizon_secs,
            "stall_k": task_core::parent_checkpoint::STALL_K,
            "roots": roots,
            "outcomes": outcomes,
            "achieved_at": self.achieved_at,
            "decisions": decisions,
            "stalls": sh.stalls,
            "effects": {
                "daemon_roots": self.roots.values().filter(|r| r.created_by == "daemon").count(),
                "controller_questions": self.seen_escalations.len(),
                "controller_feed_sources": self.env.count(&format!(
                    "SELECT COUNT(*) FROM feed_sources WHERE source_key LIKE 'project_controller:%{pid}%'"
                )),
                "cos_inputs": self.env.count(
                    "SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%'"
                ),
                "decisions": decisions.len(),
                "suppressed": suppressed,
                "decisions_with_effect": with_effect,
            },
        })
    }
}

/// 課題 1 件の腕 `shadow`。同じ seed で 2 回流して記録の bytes 一致を確かめ、効果 0 を `old.proactive` と比べて確かめる。
fn shadow(case: &str, test: &str) {
    let Some(ts) = real_ts(test) else {
        return;
    };
    let sc = load_scenario(case);
    let work = crate::test_support::WritableTempDir::new();
    // 対照: 同じ台本の CoS で mode `off`（controller は評価しない）。
    let mut base = Trial::new(&sc, Arm::OldProactive, None, work.path().join("base/repo"));
    base.steps();
    let base_facts = base.facts();
    let base_notices = base.feed_notices();
    drop(base);
    let mut outs = Vec::new();
    for run in 0..2 {
        let repo = work.path().join(format!("run{run}/repo"));
        let mut t = Trial::new(&sc, Arm::Shadow, Some(ts.clone()), repo);
        t.steps();
        outs.push((
            serde_json::to_string_pretty(&t.facts()).unwrap(),
            serde_json::to_string_pretty(&t.shadow_record()).unwrap(),
            t.feed_notices(),
        ));
    }
    assert_eq!(outs[0], outs[1], "{test}: two runs of the same seed differ");
    let facts: Value = serde_json::from_str(&outs[0].0).unwrap();
    let rec: Value = serde_json::from_str(&outs[0].1).unwrap();
    // 機構の不変条件（他の腕と同じ）。
    assert_eq!(facts["events_append_only"], true, "{test}");
    assert_eq!(facts["running_untracked"], 0, "{test}");
    // 効果 0: 判断は記録され効果は抑えられる。
    let e = &rec["effects"];
    for k in [
        "daemon_roots",
        "controller_questions",
        "controller_feed_sources",
        "cos_inputs",
        "decisions_with_effect",
    ] {
        assert_eq!(e[k], 0, "{test}: shadow had an effect ({k}): {e}");
    }
    assert!(
        e["decisions"].as_u64() > Some(0),
        "{test}: no decision: {e}"
    );
    assert!(
        e["suppressed"].as_u64() > Some(0),
        "{test}: nothing suppressed: {e}"
    );
    // 台本の木・人の操作・受信箱は同じ台本の `old.proactive` と同じ（controller の効果が台本に届いていない）。
    for k in [
        "roots",
        "effects",
        "human_ops",
        "faults",
        "invocations",
        "actors",
        "statements",
        "questions",
        "cos_launch_rejected",
        "achieved_at",
    ] {
        assert_eq!(
            facts[k], base_facts[k],
            "{test}: {k} differs from old.proactive"
        );
    }
    assert_eq!(
        outs[0].2, base_notices,
        "{test}: inbox notices differ from old.proactive"
    );
    let Some(dir) = std::env::var_os("LONG_HORIZON_BENCH_RAW_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir).join("shadow");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{case}.json")), format!("{}\n", outs[0].1)).unwrap();
}

macro_rules! shadow_case {
    ($case:literal, $name:ident) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() {
            shadow($case, stringify!($name));
        }
    };
}

shadow_case!("C01", long_horizon_shadow_c01_stale_event);
shadow_case!("C02", long_horizon_shadow_c02_partial_result);
shadow_case!("C03", long_horizon_shadow_c03_slow_worker);
shadow_case!("C04", long_horizon_shadow_c04_child_failed);
shadow_case!("C05", long_horizon_shadow_c05_plan_invalid);
shadow_case!("C06", long_horizon_shadow_c06_auth_wait);
shadow_case!("C07", long_horizon_shadow_c07_daemon_restart);
shadow_case!("C08", long_horizon_shadow_c08_multi_root);
shadow_case!("C09", long_horizon_shadow_c09_consult_short_continue);
shadow_case!("C10", long_horizon_shadow_c10_repeat_failure);
