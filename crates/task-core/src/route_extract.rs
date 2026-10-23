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
        ROUTE_OUTCOME_SCHEMA, RouteOutcome, TaskFeatures, WriteSetGap, build_task_features,
    },
    store::{SqliteStore, StoreError},
    write_set::WritePathHint,
};

type SubjectRow = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);
type OutcomeRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

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

/// Git-derived outcome values calculated by task-ops from a persisted write set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangeCount {
    pub changed_lines: u64,
    pub changed_files_n: u64,
    pub changed_crates_n: u64,
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
            "SELECT t.json,t.parent_id,t.kind,t.created_at,w.json,w.depends_on_json,t.project_id FROM tasks t LEFT JOIN work_units w ON w.id=?2 AND w.task_id=t.id WHERE t.id=?1",
            rusqlite::params![if subject.kind == "work_unit" { let x: Option<String> = c.query_row("SELECT task_id FROM work_units WHERE id=?1",[&subject.id],|r|r.get(0)).optional()?; x.unwrap_or_else(|| subject.id.clone()) } else {subject.id.clone()}, if subject.kind == "work_unit" {Some(subject.id.clone())} else {None}],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
        let Some((task_json,parent_id,task_kind,_,wu_json,depends_json,project_id))=row else { return Err(StoreError::Invalid(format!("routing feature subject not found: {}", subject.id))); };
        let task: Value=serde_json::from_str(&task_json)?;
        let wu=wu_json.as_deref().map(serde_json::from_str::<Value>).transpose()?.unwrap_or(Value::Null);
        let plan_id: Option<String> = if subject.kind=="work_unit" { c.query_row("SELECT plan_id FROM work_units WHERE id=?1",[&subject.id],|r|r.get(0)).optional()? } else {None};
        let plan: Option<String> = plan_id.as_ref().and_then(|id| c.query_row("SELECT json FROM execution_plans WHERE id=?1",[id],|r|r.get::<_,String>(0)).optional().ok().flatten());
        let project_name: Option<String> = project_id
            .as_deref()
            .map(|id| c.query_row("SELECT title FROM projects WHERE id=?1", [id], |r| r.get(0)).optional())
            .transpose()?
            .flatten();
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
        let retries:Option<u64>=if subject.kind=="work_unit" {c.query_row("SELECT retries FROM work_units WHERE id=?1",[&subject.id],|r|r.get(0)).optional()?}else{Some(prior.saturating_sub(1))};
        let repairs:Option<u64>=Some(if subject.kind=="work_unit" {c.query_row("SELECT COUNT(*) FROM work_units WHERE task_id=(SELECT task_id FROM work_units WHERE id=?1) AND kind='repair'",[&subject.id],|r|r.get(0))?}else{c.query_row("SELECT COUNT(*) FROM work_units WHERE task_id=?1 AND kind='repair'",[&subject.id],|r|r.get(0))?});
        let task_budget=task.get("budget").unwrap_or(&Value::Null); let wu_budget=wu.get("budget").unwrap_or(&Value::Null);
        let budget_turns=wu_budget["max_turns"].as_u64().or(task_budget["max_turns"].as_u64());
        let budget_wall_s=wu_budget["max_wall_secs"].as_u64().or(task_budget["max_wall_secs"].as_u64());
        let paths=arr_strings(&wu["context"]["paths"]); let repos=arr_strings(&task["repos"]);
        let deps_n = if subject.kind == "work_unit" {
            depends_json
                .as_deref()
                .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
                .map(|v| v.len() as u64)
        } else {
            task.get("depends_on")
                .and_then(Value::as_array)
                .map(|v| v.len() as u64)
        };
        let mut input=FeaturesInput {title,project_name,objective,acceptance_n:acceptance.as_ref().map(|v|v.len() as u64),check_command_n:Some(command_n),check_artifact_n:Some(artifact_n),check_reviewer_n:Some(reviewer_n),check_human_n:Some(human_n),acceptance,checks,done_when:arr_strings(&wu["done_when"]),outputs:arr_strings(&wu["outputs"]),paths:paths.clone(),repos:repos.clone(),genre:str(&task,"genre"),kind:if subject.kind=="work_unit"{str(&wu,"kind")}else{Some("task".into())},harness:ctx.harness.or(old_harness),lane:ctx.lane.or(old_lane),depth:Some(depth),deps_n,prior_runs_n:Some(prior),prior_failed_n:Some(failed),prior_runs:Some(prior_runs),retries_n:retries,repairs_n:repairs,budget_turns,budget_wall_s,task_features_v69:v69,..Default::default()};
        let (write_set, write_set_gaps) = prior_write_set(c, &subject, &as_of)?;
        input.write_set = write_set;
        input.write_set_gaps = write_set_gaps;
        input.context_paths_n=paths.as_ref().map(|v|v.len()); input.repos_n=repos.as_ref().map(|v|v.len());
        input.crates_n=paths.as_ref().map(|v|v.iter().filter_map(|s|s.strip_prefix("crates/").and_then(|s|s.split('/').next())).collect::<std::collections::HashSet<_>>().len());
        input.phase_index=plan.as_deref().and_then(|p|serde_json::from_str::<Value>(p).ok()).and_then(|p|p["phases"].as_array().and_then(|a|a.iter().position(|x|x["key"]==wu["phase"]).map(|i|i as u64)));
        let _=(task_kind,); Ok(build_task_features(&input))
    })
}

/// Extract a run's `celeris.route-outcome/1` labels for an evaluation/export runner.
/// Git-derived change counts are supplied by task-ops; DB-backed labels are extracted here.
pub fn outcome_labels_for_run(
    store: &SqliteStore,
    run_id: &str,
    changes: Option<ChangeCount>,
) -> Result<RouteOutcome, StoreError> {
    store.with_read_conn(|c| {
        let row: Option<OutcomeRow>=c.query_row("SELECT status,usage_json,metrics_json,work_unit_id,task_id,started_at FROM runs WHERE run_id=?1",[run_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
        let Some((status,usage,metrics,wu,task_id,started_at))=row else{return Err(StoreError::Invalid(format!("run not found: {run_id}")));};
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
        let quota: Option<Value> = {
            let mut st=c.prepare("SELECT json FROM events WHERE task_id=?1 AND json_extract(json,'$.type')='quota_estimated' AND json_extract(json,'$.run_id')=?2 ORDER BY seq DESC")?;
            let mut rows=st.query(rusqlite::params![task_id,run_id])?;
            let mut result=None;
            if let Some(r)=rows.next()? { let event:Value=serde_json::from_str(&r.get::<_,String>(0)?)?; let mut map=serde_json::Map::new();
                if let Some(windows)=event["windows"].as_array(){for w in windows {if let Some(pct)=w["used_pct"].as_f64() && let Some(name)=w["window"].as_str(){map.insert(name.to_owned(),serde_json::json!(pct));}}}
                if !map.is_empty(){result=Some(Value::Object(map));}
            } result
        };
        let retries_after: Option<u64> = if let Some(start)=started_at.as_deref() {
            let count:i64=if let Some(wu_id)=wu.as_deref(){c.query_row("SELECT COUNT(*) FROM runs WHERE work_unit_id=?1 AND role='worker' AND started_at>?2",rusqlite::params![wu_id,start],|r|r.get(0))?}
            else {c.query_row("SELECT COUNT(*) FROM runs WHERE task_id=?1 AND work_unit_id IS NULL AND role='worker' AND started_at>?2",rusqlite::params![task_id,start],|r|r.get(0))?}; Some(count as u64)
        } else {None};
        let repaired: Option<bool> = started_at.as_deref().map(|start| c.query_row("SELECT EXISTS(SELECT 1 FROM work_units WHERE task_id=?1 AND kind='repair' AND created_at>?2)",rusqlite::params![task_id,start],|r|r.get::<_,bool>(0))).transpose()?;
        if quota.is_none() { missing.push("quota_used".into()); }
        if retries_after.is_none() { missing.push("retries_after".into()); }
        if repaired.is_none() { missing.push("repaired".into()); }
        let changed_lines=changes.map(|v|v.changed_lines);
        let changed_files_n=changes.map(|v|v.changed_files_n);
        let changed_crates_n=changes.map(|v|v.changed_crates_n);
        if changes.is_none() { missing.extend(["changed_lines","changed_files_n","changed_crates_n"].map(str::to_owned)); }
        Ok(RouteOutcome{schema:ROUTE_OUTCOME_SCHEMA.into(),labels:serde_json::json!({"run_status":status,"checks_passed":if total>0{Some(passed)}else{None},"checks_total":if total>0{Some(total)}else{None},"review_pass":review,"wall_s":wall,"cost_usd_list":usage.get("cost_usd_list"),"tokens_in":usage.get("tokens_in"),"tokens_out":usage.get("tokens_out"),"quota_used":quota,"changed_lines":changed_lines,"changed_files_n":changed_files_n,"changed_crates_n":changed_crates_n,"retries_after":retries_after,"repaired":repaired,"missing":missing})})
    })
}

/// D2「触る範囲」の書き込み集合の予測。task は `run_write_sets`（task_id 一致）、WU は
/// `run_write_sets`（work_unit_id 一致）と `work_unit_write_sets`。判断時点より後の行は読まない:
/// `AsOf::Timestamp` は `recorded_at` と run の `started_at`、`AsOf::EventSeq` は runs と同じく
/// その run を指す event の `seq` と、境界 seq までの event の最大 `ts` 以前の `recorded_at`。
/// path は `<repo>:<path>` に正規化（先頭 `./` と末尾 `/` を落とす、不正な path は捨てる）し、
/// 重複を除いて整列する。`incomplete`／`unavailable` の行は `WriteSetGap` に残す。
/// 行が無い（または complete の行が無く path も無い）ときは `None`（欠測）。
fn prior_write_set(
    c: &rusqlite::Connection,
    subject: &Subject,
    as_of: &Option<AsOf>,
) -> Result<(Option<Vec<String>>, Vec<WriteSetGap>), StoreError> {
    let ts = match as_of {
        Some(AsOf::Timestamp(s)) => Some(s.as_str()),
        _ => None,
    };
    let seq = match as_of {
        Some(AsOf::EventSeq(n)) => Some(*n),
        _ => None,
    };
    let is_wu = subject.kind == "work_unit";
    let owner_column = if is_wu { "work_unit_id" } else { "task_id" };
    let mut sql = format!(
        "SELECT 'run_write_sets',ws.run_id,ws.repo_id,ws.paths_json,ws.status FROM run_write_sets ws \
         JOIN runs r ON r.run_id=ws.run_id WHERE ws.{owner_column}=?1 \
         AND (?2 IS NULL OR (ws.recorded_at<=?2 AND r.started_at<=?2)) \
         AND (?3 IS NULL OR (EXISTS (SELECT 1 FROM events e WHERE e.task_id=r.task_id AND e.seq<=?3 AND json_extract(e.json,'$.run_id')=r.run_id) \
         AND ws.recorded_at<=(SELECT MAX(e.ts) FROM events e WHERE e.task_id=ws.task_id AND e.seq<=?3)))"
    );
    if is_wu {
        sql.push_str(
            " UNION ALL SELECT 'work_unit_write_sets',w.work_unit_id,w.repo_id,w.paths_json,w.status FROM work_unit_write_sets w \
             WHERE w.work_unit_id=?1 AND (?2 IS NULL OR w.recorded_at<=?2) \
             AND (?3 IS NULL OR w.recorded_at<=(SELECT MAX(e.ts) FROM events e WHERE e.task_id=w.task_id AND e.seq<=?3))",
        );
    }
    sql.push_str(" ORDER BY 1,2,3");
    let mut stmt = c.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params![subject.id, ts, seq])?;
    let mut paths = std::collections::BTreeSet::new();
    let mut gaps = std::collections::BTreeSet::new();
    let mut complete = false;
    while let Some(r) = rows.next()? {
        let source: String = r.get(0)?;
        let owner_id: String = r.get(1)?;
        let repo: String = r.get(2)?;
        let paths_json: String = r.get(3)?;
        let status: String = r.get(4)?;
        let raw: Vec<String> = serde_json::from_str(&paths_json).unwrap_or_default();
        for p in raw {
            let p = p.trim();
            let p = p.strip_prefix("./").unwrap_or(p);
            if let Ok(hint) = WritePathHint::parse(p) {
                paths.insert(format!("{repo}:{}", hint.as_str()));
            }
        }
        if status == "complete" {
            complete = true;
        } else {
            gaps.insert(WriteSetGap {
                source,
                owner_id,
                repo,
                status,
            });
        }
    }
    let write_set = (complete || !paths.is_empty()).then(|| paths.into_iter().collect());
    Ok((write_set, gaps.into_iter().collect()))
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
#[path = "route_extract/tests.rs"]
mod tests;
