//! Database-backed feature and outcome extraction for routing evaluation.
//!
//! These APIs read only structured task, work-unit, run, plan, and selected event
//! metadata. They never read prompts, comments, logs, tool output, or credentials.
//! `AsOf` can pin a snapshot to an event sequence or timestamp for replaying the
//! inputs that were available when a routing decision was made.
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    execution_plan::WorkUnitCheck,
    model::Criterion,
    route_record::{
        AcceptanceInput, CheckInput, FailedCriterionInput, FeaturesInput, PriorRunInput,
        ROUTE_OUTCOME_SCHEMA, RouteOutcome, TaskFeatures, build_task_features,
    },
    store::{SqliteStore, StoreError},
};

type SubjectRow = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
);
type OutcomeRow = (String, Option<String>, Option<String>, Option<String>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub kind: String,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AsOf {
    EventSeq(i64),
    Timestamp(String),
}

/// Routing values already known by dispatch. When absent, historical values are
/// recovered from the latest routing_decided event at the requested snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteFeatureContext {
    pub lane: Option<String>,
    pub harness: Option<String>,
    pub task_features_v69: Option<crate::model_policy::TaskFeatures>,
}

/// Read a deterministic `celeris.task-features/1` snapshot for a WU or leaf task.
/// `as_of` limits included runs and events; pass the sequence or timestamp captured
/// at the routing decision to build evaluation manifests without future information.
pub fn features_for_subject(
    store: &SqliteStore,
    subject: Subject,
    as_of: Option<AsOf>,
    context: Option<RouteFeatureContext>,
) -> Result<TaskFeatures, StoreError> {
    store.with_read_conn(|c| {
        let row: Option<SubjectRow> = c.query_row(
            "SELECT t.json,t.parent_id,t.kind,t.created_at,w.json,w.depends_on_json FROM tasks t LEFT JOIN work_units w ON w.id=?2 AND w.task_id=t.id WHERE t.id=?1",
            rusqlite::params![if subject.kind == "work_unit" { let x: Option<String> = c.query_row("SELECT task_id FROM work_units WHERE id=?1",[&subject.id],|r|r.get(0)).optional()?; x.unwrap_or_else(|| subject.id.clone()) } else {subject.id.clone()}, if subject.kind == "work_unit" {Some(subject.id.clone())} else {None}],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
        let Some((task_json,parent_id,task_kind,_,wu_json,depends_json))=row else { return Err(StoreError::Invalid(format!("routing feature subject not found: {}", subject.id))); };
        let task: Value=serde_json::from_str(&task_json)?;
        let wu=wu_json.as_deref().map(serde_json::from_str::<Value>).transpose()?.unwrap_or(Value::Null);
        let plan_id: Option<String> = if subject.kind=="work_unit" { c.query_row("SELECT plan_id FROM work_units WHERE id=?1",[&subject.id],|r|r.get(0)).optional()? } else {None};
        let plan: Option<String> = plan_id.as_ref().and_then(|id| c.query_row("SELECT json FROM execution_plans WHERE id=?1",[id],|r|r.get::<_,String>(0)).optional().ok().flatten());
        let title=str(&task,"title"); let objective=if subject.kind=="work_unit" {str(&wu,"objective")} else {str(&task,"objective")};
        let acceptance=arr_values(&task["acceptance"]).map(|v|v.into_iter().filter_map(|x|serde_json::from_value::<Criterion>(x).ok()).map(|c|AcceptanceInput::from(&c)).collect::<Vec<_>>());
        let checks=if subject.kind=="work_unit" { arr_values(&wu["checks"]).map(|v|v.into_iter().filter_map(|x|serde_json::from_value::<WorkUnitCheck>(x).ok()).map(|c|CheckInput::Command{cmd:c.cmd}).collect::<Vec<_>>()) } else {None};
        let mut depth=0u64; let mut ancestor=parent_id.clone(); let mut seen=std::collections::HashSet::new();
        while let Some(id)=ancestor { if !seen.insert(id.clone()) {break;} depth+=1; ancestor=c.query_row("SELECT parent_id FROM tasks WHERE id=?1",[id],|r|r.get(0)).optional()?.flatten(); }
        let mut runs_stmt=c.prepare("SELECT run_id,status,started_at,finished_at,usage_json,metrics_json FROM runs WHERE (?1 IS NULL OR work_unit_id=?1) AND (?2 IS NULL OR task_id=?2) AND (?3 IS NULL OR started_at<=?3) AND (?4 IS NULL OR EXISTS (SELECT 1 FROM events e WHERE e.task_id=runs.task_id AND e.seq<=?4 AND json_extract(e.json,'$.run_id')=runs.run_id)) ORDER BY started_at,run_id")?;
        let boundary=match &as_of {Some(AsOf::Timestamp(s))=>Some(s.as_str()), _=>None};
        let seq_boundary=match as_of {Some(AsOf::EventSeq(n))=>Some(n),_=>None};
        let mut run_rows=runs_stmt.query(rusqlite::params![if subject.kind=="work_unit" {Some(subject.id.as_str())}else{None},if subject.kind=="task"{Some(subject.id.as_str())}else{None},boundary,seq_boundary])?;
        let(mut prior,mut failed)=(0u64,0u64); let mut prior_data=Vec::new(); while let Some(r)=run_rows.next()? {let run_id:String=r.get(0)?;let status:String=r.get(1)?;let metrics:Option<String>=r.get(5)?;prior+=1;if status=="failed"||status=="budget_exhausted"{failed+=1;} let wall=metrics.as_deref().and_then(|s|serde_json::from_str::<Value>(s).ok()).and_then(|v|v["wall_ms"].as_f64()).map(|v|(v/1000.0) as u64); prior_data.push((run_id,status,wall));}
        let mut event_sql="SELECT json FROM events WHERE task_id=?1".to_owned();
        if matches!(as_of,Some(AsOf::EventSeq(_))) {event_sql.push_str(" AND seq<=?2");}
        if matches!(as_of,Some(AsOf::Timestamp(_))) {event_sql.push_str(" AND ts<=?2");}
        event_sql.push_str(" ORDER BY seq");
        let mut ev_stmt=c.prepare(&event_sql)?; let mut ev_rows=match as_of {Some(AsOf::EventSeq(n))=>ev_stmt.query(rusqlite::params![if subject.kind=="work_unit" {c.query_row("SELECT task_id FROM work_units WHERE id=?1",[&subject.id],|r|r.get::<_,String>(0))?}else{subject.id.clone()},n])?,Some(AsOf::Timestamp(ref s))=>ev_stmt.query(rusqlite::params![if subject.kind=="work_unit" {c.query_row("SELECT task_id FROM work_units WHERE id=?1",[&subject.id],|r|r.get::<_,String>(0))?}else{subject.id.clone()},s])?,None=>ev_stmt.query([if subject.kind=="work_unit" {c.query_row("SELECT task_id FROM work_units WHERE id=?1",[&subject.id],|r|r.get::<_,String>(0))?}else{subject.id.clone()}])?};
        let mut v69=None; let mut old_lane=None; let mut old_harness=None; let mut models=std::collections::HashMap::new(); let mut failed_by_run=std::collections::HashMap::<String,Vec<FailedCriterionInput>>::new(); while let Some(r)=ev_rows.next()? {let raw:String=r.get(0)?;let e:Value=serde_json::from_str(&raw)?; if e["type"]=="routing_decided" {let rec=e.get("record").unwrap_or(&e); let f=rec.pointer("/decision/features").cloned();if let Some(f)=f.and_then(|f|serde_json::from_value(f).ok()){v69=Some(f);} old_lane=rec.pointer("/decision/lane").and_then(Value::as_str).map(str::to_owned); old_harness=rec.get("harness").and_then(Value::as_str).map(str::to_owned); } else if e["type"]=="worker_started" {if let (Some(id),Some(model))=(e["run_id"].as_str(),e["model"].as_str()){models.insert(id.to_owned(),model.to_owned());}} else if e["type"]=="review_verdict" && e["pass"].as_bool()==Some(false) && let Some(id)=e["run_id"].as_str(){let idx=e["criterion_idx"].as_u64().unwrap_or(u64::MAX) as usize;let text=acceptance.as_ref().and_then(|a|a.get(idx).map(|x|x.text.clone())).or_else(||checks.as_ref().and_then(|a|a.get(idx.saturating_sub(acceptance.as_ref().map_or(0,Vec::len))).map(|x|match x {CheckInput::Command{cmd}=>cmd.clone(),_=>String::new()}))).unwrap_or_default();failed_by_run.entry(id.to_owned()).or_default().push(FailedCriterionInput{text,exit:None});} }
        let prior_runs=prior_data.into_iter().map(|(id,status,wall_s)|PriorRunInput{status,model:models.remove(&id),wall_s,failed_criteria:failed_by_run.remove(&id).unwrap_or_default()}).collect::<Vec<_>>();
        let ctx=context.unwrap_or_default();
        if ctx.task_features_v69.is_some(){v69=ctx.task_features_v69;}
        let check_types=acceptance.as_deref().unwrap_or(&[]).iter().map(|x|&x.check).chain(checks.as_deref().unwrap_or(&[]).iter());
        let (mut command_n,mut artifact_n,mut reviewer_n,mut human_n)=(0u64,0u64,0u64,0u64);
        for check in check_types { match check {CheckInput::Command{..}=>command_n+=1,CheckInput::ArtifactExists|CheckInput::KnowledgePage=>artifact_n+=1,CheckInput::Reviewer=>reviewer_n+=1,CheckInput::Human=>human_n+=1} }
        let retries:Option<u64>=if subject.kind=="work_unit" {c.query_row("SELECT retries FROM work_units WHERE id=?1",[&subject.id],|r|r.get(0)).optional()?}else{None};
        let repairs:Option<u64>=if subject.kind=="work_unit" {Some(c.query_row("SELECT COUNT(*) FROM work_units WHERE task_id=(SELECT task_id FROM work_units WHERE id=?1) AND kind='repair'",[&subject.id],|r|r.get(0))?)}else{None};
        let task_budget=task.get("budget").unwrap_or(&Value::Null); let wu_budget=wu.get("budget").unwrap_or(&Value::Null);
        let budget_turns=wu_budget["max_turns"].as_u64().or(task_budget["max_turns"].as_u64());
        let budget_wall_s=wu_budget["max_wall_secs"].as_u64().or(task_budget["max_wall_secs"].as_u64());
        let paths=arr_strings(&wu["context"]["paths"]); let repos=arr_strings(&task["repos"]);
        let mut input=FeaturesInput {title,objective,acceptance_n:acceptance.as_ref().map(|v|v.len() as u64),check_command_n:Some(command_n),check_artifact_n:Some(artifact_n),check_reviewer_n:Some(reviewer_n),check_human_n:Some(human_n),acceptance,checks,done_when:arr_strings(&wu["done_when"]),outputs:arr_strings(&wu["outputs"]),paths:paths.clone(),repos:repos.clone(),genre:str(&task,"genre"),kind:if subject.kind=="work_unit"{str(&wu,"kind")}else{Some("task".into())},harness:ctx.harness.or(old_harness),lane:ctx.lane.or(old_lane),depth:Some(depth),deps_n:depends_json.as_deref().and_then(|s|serde_json::from_str::<Vec<Value>>(s).ok()).map(|v|v.len() as u64),prior_runs_n:Some(prior),prior_failed_n:Some(failed),prior_runs:Some(prior_runs),retries_n:retries,repairs_n:repairs,budget_turns,budget_wall_s,task_features_v69:v69,..Default::default()};
        input.context_paths_n=paths.as_ref().map(|v|v.len()); input.repos_n=repos.as_ref().map(|v|v.len());
        input.crates_n=paths.as_ref().map(|v|v.iter().filter_map(|s|s.strip_prefix("crates/").and_then(|s|s.split('/').next())).collect::<std::collections::HashSet<_>>().len());
        input.phase_index=plan.as_deref().and_then(|p|serde_json::from_str::<Value>(p).ok()).and_then(|p|p["phases"].as_array().and_then(|a|a.iter().position(|x|x["key"]==wu["phase"]).map(|i|i as u64)));
        let _=(task_kind,); Ok(build_task_features(&input))
    })
}

/// Extract a run's `celeris.route-outcome/1` labels for an evaluation/export runner.
/// Unavailable metrics (including git-derived change counts and quota) are null and
/// listed in `missing`; this function performs no git operations.
pub fn outcome_labels_for_run(
    store: &SqliteStore,
    run_id: &str,
) -> Result<RouteOutcome, StoreError> {
    store.with_read_conn(|c| {
        let row: Option<OutcomeRow>=c.query_row("SELECT status,usage_json,metrics_json,work_unit_id FROM runs WHERE run_id=?1",[run_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let Some((status,usage,metrics,wu))=row else{return Err(StoreError::Invalid(format!("run not found: {run_id}")));};
        let usage:Value=usage.as_deref().and_then(|s|serde_json::from_str(s).ok()).unwrap_or(Value::Null);let metrics:Value=metrics.as_deref().and_then(|s|serde_json::from_str(s).ok()).unwrap_or(Value::Null);
        let mut verdict_stmt=c.prepare("SELECT json FROM events WHERE json_extract(json,'$.type')='review_verdict' AND (?1 IS NULL OR task_id=(SELECT task_id FROM runs WHERE run_id=?2)) ORDER BY seq")?;
        let rows=verdict_stmt.query_map(rusqlite::params![wu,run_id],|r|r.get::<_,String>(0))?;
        let(mut passed,mut total,mut review)=(0u64,0u64,None);
        for item in rows {let e:Value=serde_json::from_str(&item?)?;if let Some(items)=e["verdict"]["criteria"].as_array().or(e["criteria"].as_array()){for x in items{total+=1;if x["pass"].as_bool()==Some(true){passed+=1;}}}
            if let Some(v)=e.pointer("/verdict/pass").and_then(Value::as_bool).or(e["pass"].as_bool()){review=Some(v);}}
        let mut missing=Vec::new(); let wall=metrics["wall_ms"].as_f64().map(|n|n/1000.0); if wall.is_none(){missing.push("wall_s".to_owned());}
        if usage["cost_usd_list"].is_null(){missing.push("cost_usd_list".into());}
        if usage["tokens_in"].is_null(){missing.push("tokens_in".into());}
        if usage["tokens_out"].is_null(){missing.push("tokens_out".into());}
        missing.extend(["quota_used","changed_lines","changed_files_n","changed_crates_n","retries_after","repaired"].map(str::to_owned));
        Ok(RouteOutcome{schema:ROUTE_OUTCOME_SCHEMA.into(),labels:serde_json::json!({"run_status":status,"checks_passed":if total>0{Some(passed)}else{None},"checks_total":if total>0{Some(total)}else{None},"review_pass":review,"wall_s":wall,"cost_usd_list":usage.get("cost_usd_list"),"tokens_in":usage.get("tokens_in"),"tokens_out":usage.get("tokens_out"),"quota_used":null,"changed_lines":null,"changed_files_n":null,"changed_crates_n":null,"retries_after":null,"repaired":null,"missing":missing})})
    })
}

fn str(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}
fn arr_strings(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}
fn arr_values(v: &Value) -> Option<Vec<Value>> {
    v.as_array().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SqliteStore;

    fn fixture() -> SqliteStore {
        let s = SqliteStore::open_in_memory().unwrap();
        let c = s.lock().unwrap();
        c.execute("INSERT INTO tasks(id,status,kind,parent_id,priority,created_at,json) VALUES('task-1','running','task',NULL,0,'2026-01-01T00:00:00Z',?1)",[
            r#"{"title":"A task","objective":"Build it","genre":"coding","budget":{"max_turns":22,"max_wall_secs":600,"max_retries":3},"acceptance":[{"text":"works","check":{"type":"command","cmd":"cargo test -p task-core","expect_exit":0}},{"text":"docs","check":{"type":"artifact_exists","name":"docs.md"}},{"text":"review","check":{"type":"reviewer"}},{"text":"human","check":{"type":"human"}}],"repos":["repo"]}"#]).unwrap();
        c.execute("INSERT INTO execution_plans(id,task_id,version,origin,status,json,created_at) VALUES('p1','task-1',1,'fixture','active','{}','2026-01-01T00:00:00Z')",[]).unwrap();
        c.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,retries,json,created_at,updated_at) VALUES('wu-1','task-1','p1','leaf',1,'implement','ready','[]',2,?1,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[r#"{"objective":"Do work","budget":{"max_turns":30,"max_wall_secs":900},"checks":[{"cmd":"cargo clippy --workspace","expect_exit":0,"scope":true}],"context":{"paths":["crates/task-core/src/lib.rs"]}}"#]).unwrap();
        c.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,retries,json,created_at,updated_at) VALUES('wu-repair','task-1','p1','repair',2,'repair','ready','[]',0,'{}','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[]).unwrap();
        c.execute("INSERT INTO runs(run_id,task_id,work_unit_id,role,seq,status,usage_json,metrics_json,started_at,finished_at) VALUES('r1','task-1','wu-1','worker',1,'completed','{}','{}','2026-01-01T00:01:00Z','2026-01-01T00:02:00Z')",[]).unwrap();
        c.execute("INSERT INTO runs(run_id,task_id,work_unit_id,role,seq,status,started_at) VALUES('r2','task-1','wu-1','worker',2,'failed','2026-01-01T00:03:00Z')",[]).unwrap();
        c.execute(
            "INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',1,'2026-01-01T00:00:01Z',?1)",
            [r#"{"type":"comment","text":"prompt sk_live_abcdefghijk and private instruction"}"#],
        )
        .unwrap();
        c.execute("INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',2,'2026-01-01T00:01:00Z','{\"type\":\"worker_started\",\"run_id\":\"r1\",\"model\":\"model-one\"}')",[]).unwrap();
        c.execute("INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',3,'2026-01-01T00:03:00Z','{\"type\":\"worker_started\",\"run_id\":\"r2\",\"model\":\"model-two\"}')",[]).unwrap();
        c.execute("INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',5,'2026-01-01T00:05:00Z','{\"type\":\"review_verdict\",\"run_id\":\"r2\",\"criterion_idx\":0,\"pass\":false,\"reason\":\"stdout sk_live_supersecret\"}')",[]).unwrap();
        c.execute("INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',4,'2026-01-01T00:04:00Z',?1)",[r#"{"type":"routing_decided","record":{"harness":"old-harness","decision":{"lane":"cheap","features":{"judgment":"low","ambiguity":"low","verifiability":"high","reversibility":"high","consequence":"low","context_size":"low","tool_intensity":"low","expected_length":"low","cross_cutting":"low"}}}}"#]).unwrap();
        drop(c);
        s
    }

    #[test]
    fn route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs() {
        let s = fixture();
        let wu = Subject {
            kind: "work_unit".into(),
            id: "wu-1".into(),
        };
        let a = features_for_subject(
            &s,
            wu.clone(),
            Some(AsOf::Timestamp("2026-01-01T00:02:30Z".into())),
            None,
        )
        .unwrap();
        let b = features_for_subject(
            &s,
            wu,
            Some(AsOf::Timestamp("2026-01-01T00:02:30Z".into())),
            None,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap()
        );
        assert_eq!(a.aux.prior_runs_n, Some(1));
        assert_eq!(a.aux.acceptance_n, Some(4));
        assert_eq!(a.aux.check_command_n, Some(2));
        assert_eq!(a.aux.check_artifact_n, Some(1));
        assert_eq!(a.aux.check_reviewer_n, Some(1));
        assert_eq!(a.aux.check_human_n, Some(1));
        assert_eq!(a.aux.retries_n, Some(2));
        assert_eq!(a.aux.repairs_n, Some(1));
        assert_eq!(a.aux.budget_turns, Some(30));
        assert_eq!(a.aux.budget_wall_s, Some(900));
        assert!(a.brief.contains("works") && !a.brief.contains("決定的な検査なし"));
        let feature = crate::model_policy::TaskFeatures {
            judgment: crate::model_policy::Level::Medium,
            ambiguity: crate::model_policy::Level::Medium,
            verifiability: crate::model_policy::Level::High,
            reversibility: crate::model_policy::Level::High,
            consequence: crate::model_policy::Level::Low,
            context_size: crate::model_policy::Level::Medium,
            tool_intensity: crate::model_policy::Level::High,
            expected_length: crate::model_policy::Level::Medium,
            cross_cutting: crate::model_policy::Level::Low,
        };
        let ctx = RouteFeatureContext {
            lane: Some("standard".into()),
            harness: Some("claude-code".into()),
            task_features_v69: Some(feature),
        };
        let with_ctx = features_for_subject(
            &s,
            Subject {
                kind: "work_unit".into(),
                id: "wu-1".into(),
            },
            Some(AsOf::EventSeq(5)),
            Some(ctx),
        )
        .unwrap();
        assert_eq!(with_ctx.aux.lane.as_deref(), Some("standard"));
        assert_eq!(with_ctx.aux.harness.as_deref(), Some("claude-code"));
        assert!(with_ctx.aux.task_features_v69.is_some());
        assert!(with_ctx.brief.contains("model-two") && with_ctx.brief.contains("works"));
        assert!(!with_ctx.brief.contains("stdout") && !with_ctx.brief.contains("supersecret"));
        assert!(!with_ctx.aux.missing.iter().any(|m| {
            [
                "acceptance_n",
                "check_command_n",
                "check_artifact_n",
                "check_reviewer_n",
                "check_human_n",
                "retries_n",
                "repairs_n",
                "budget_turns",
                "budget_wall_s",
                "lane",
                "harness",
            ]
            .contains(&m.as_str())
        }));
        let by_seq = features_for_subject(
            &s,
            Subject {
                kind: "work_unit".into(),
                id: "wu-1".into(),
            },
            Some(AsOf::EventSeq(2)),
            None,
        )
        .unwrap();
        assert_eq!(by_seq.aux.prior_runs_n, Some(1));
        assert!(!a.brief.contains("private instruction") && !a.brief.contains("sk_live_"));
        assert!(
            features_for_subject(
                &s,
                Subject {
                    kind: "task".into(),
                    id: "task-1".into()
                },
                None,
                None,
            )
            .unwrap()
            .brief
            .contains("A task")
        );
    }

    #[test]
    fn route_extract_outcome_reports_missing_metrics_without_git() {
        let s = fixture();
        let outcome = outcome_labels_for_run(&s, "r1").unwrap();
        assert_eq!(outcome.schema, ROUTE_OUTCOME_SCHEMA);
        assert!(
            outcome.labels["missing"]
                .as_array()
                .unwrap()
                .contains(&Value::String("changed_lines".into()))
        );
    }
}
