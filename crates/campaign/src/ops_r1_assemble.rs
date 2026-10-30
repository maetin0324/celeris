//! `ops-r1-assemble`: ops-r1 の 2 condition（A=直接 Claude Code、B=CoS 受付）の記録から、runner と同じ形の out dir と
//! decision-log/1 形の `records.jsonl` を作る（写像は `docs/ops/ops-r1-pipeline.md` §4）。
//!
//! - task API にも daemon にも触れない。入力は A の raw（`ops-r1-a-raw/1`）、B の intake・trace・attribution、
//!   両腕の成果物 dir の写しと、lock の材料（cases・grader・grade-case の root）だけ。
//! - 時刻・host・実行 path を出さない。同じ入力から 2 回作れば out と records.jsonl は byte 一致する。
//! - B の CoS run の model が manifest の `contract.conditions.B.cos_model` と違えば条件逸脱として止める。
//! - `--b-stop` に停止記録（`ops-r1-stop/1`）を渡すと、B は停止の理由で分母に残す（§4.5）。渡さなければ出力は変わらない。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    aggregate::Outcome,
    env_lock::ProgramPin,
    ledger::{LEDGER_FILE, LEDGER_SCHEMA, LedgerRow, LedgerState, ObservedUsage, TrialKey},
    lock_exec,
    manifest::{CampaignManifest, ConfigId, TokenUsage},
    ops_r1_a_raw::{self, Stop},
    ops_r1_conditions::ConditionDecl,
    record::{
        CAMPAIGN_FILE, DeliveredInput, INPUT_FILE, META_FILE, ObservedRun, REASON_CASE_BUDGET,
        REASON_NO_ARTIFACT, SNAPSHOT_FILE, TRIAL_SCHEMA, TRIALS_FILE, TrialRecord,
    },
    snapshot::{copy_tree, hash_tree},
    versions::ObservedVersions,
};

pub const EXECUTOR: &str = "ops-r1-assemble";
/// decision-log の schema（final-accounting が読む形）。
pub const RECORD_SCHEMA: &str = "celeris.decision-log/1";
/// A の exit≠0。
pub const REASON_HARNESS_EXIT: &str = "harness_exit";
/// 測定者が止めた（A の stop=operator_stop）。
pub const REASON_OPERATOR_STOP: &str = "operator_stop";
/// B の root task が failed で終わった。
pub const REASON_TASK_FAILED: &str = "task_failed";
/// B の root task が cancelled で終わった。
pub const REASON_TASK_CANCELLED: &str = "task_cancelled";
/// 測定者が条件の逸脱（runbook §5 の condition_drift）で B を止めた。
pub const REASON_CONDITION_DRIFT: &str = "condition_drift";
/// runbook §5 のほかの停止（stop_id をそのまま reason にする）。
pub const REASON_SAME_CAUSE_TWICE: &str = "same_cause_twice";
pub const REASON_LIMIT_REACHED: &str = "limit_reached";
pub const REASON_UNOBSERVED_USAGE: &str = "unobserved_usage";
pub const REASON_LEAK: &str = "leak";
/// 測定者の停止で終わった試行の reason。checker は task run が 0 件でも違反にしない（§6）。
pub const STOP_REASONS: &[&str] = &[
    REASON_OPERATOR_STOP,
    REASON_CONDITION_DRIFT,
    REASON_SAME_CAUSE_TWICE,
    REASON_LIMIT_REACHED,
    REASON_UNOBSERVED_USAGE,
    REASON_LEAK,
];
/// 停止記録の schema（runbook §5）。
pub const STOP_SCHEMA: &str = "ops-r1-stop/1";
/// records.jsonl の停止の行（final-accounting は usage・run・trial 以外を読まない）。
pub const STOP_RECORD_TYPE: &str = "trial_stop";
/// A は Claude Code を直接起動する（daemon の adapter 名ではない）。
pub const A_HARNESS: &str = "claude_code";

/// 測定者の停止で終わった試行の reason か。
pub fn is_stop_reason(reason: Option<&str>) -> bool {
    reason.is_some_and(|r| STOP_REASONS.contains(&r))
}

const USAGE: &str = "usage: celeris-campaign ops-r1-assemble --manifest <ops-r1 manifest> --input-manifest <json> \
--a-raw <ops-r1-a-raw/1> --a-artifact <dir> --b-intake <ops-r1-intake/1> --b-trace <ops-r1-trace/1> \
--b-attribution <ops-r1-attribution/2> --b-artifact <dir> --b-observed-versions <json> --b-workspace-sha256-initial <hex> \
--cases-root <dir> --grader-root <dir> --grade-case-root <dir> [--measurement-id <id>] [--b-stop <ops-r1-stop/1>] \
--out <empty dir> --records-out <file>";

/// 引数。path は読むだけで、出力には書かない。
#[derive(Clone, Debug)]
pub struct Args {
    pub manifest: PathBuf,
    pub input_manifest: PathBuf,
    pub a_raw: PathBuf,
    pub a_artifact: PathBuf,
    pub b_intake: PathBuf,
    pub b_trace: PathBuf,
    pub b_attribution: PathBuf,
    pub b_artifact: PathBuf,
    pub b_observed_versions: PathBuf,
    pub b_workspace_sha256_initial: String,
    pub cases_root: PathBuf,
    pub grader_root: PathBuf,
    pub grade_case_root: PathBuf,
    pub measurement_id: Option<String>,
    /// B を止めた停止記録（`ops-r1-stop/1`）。
    pub b_stop: Option<PathBuf>,
    pub out: PathBuf,
    pub records_out: PathBuf,
}

impl Args {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let get = |k: &str| {
            args.iter()
                .position(|a| a == k)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let need = |k: &str| get(k).ok_or_else(|| format!("{k} is required\n{USAGE}"));
        let p = |k: &str| need(k).map(PathBuf::from);
        Ok(Self {
            manifest: p("--manifest")?,
            input_manifest: p("--input-manifest")?,
            a_raw: p("--a-raw")?,
            a_artifact: p("--a-artifact")?,
            b_intake: p("--b-intake")?,
            b_trace: p("--b-trace")?,
            b_attribution: p("--b-attribution")?,
            b_artifact: p("--b-artifact")?,
            b_observed_versions: p("--b-observed-versions")?,
            b_workspace_sha256_initial: need("--b-workspace-sha256-initial")?,
            cases_root: p("--cases-root")?,
            grader_root: p("--grader-root")?,
            grade_case_root: p("--grade-case-root")?,
            measurement_id: get("--measurement-id"),
            b_stop: get("--b-stop").map(PathBuf::from),
            out: p("--out")?,
            records_out: p("--records-out")?,
        })
    }
}

fn read_json(p: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?)
        .map_err(|e| format!("{}: {e}", p.display()))
}

fn write_json(p: &Path, v: &Value) -> Result<(), String> {
    let mut s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    s.push('\n');
    fs::write(p, s).map_err(|e| format!("{}: {e}", p.display()))
}

fn ts(s: &str, what: &str) -> Result<OffsetDateTime, String> {
    OffsetDateTime::parse(s, &Rfc3339).map_err(|e| format!("{what} {s:?}: {e}"))
}

fn secs_between(from: OffsetDateTime, to: OffsetDateTime) -> u64 {
    u64::try_from((to - from).whole_seconds()).unwrap_or(0)
}

fn epoch(t: OffsetDateTime) -> Option<u64> {
    u64::try_from(t.unix_timestamp()).ok()
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// trial_id の位置（1 始まりの rep）。
fn rep_of(c: &ConditionDecl, trial_id: &str, arm: ConfigId) -> Result<u32, String> {
    c.trial_ids
        .iter()
        .position(|t| t == trial_id)
        .map(|i| i as u32 + 1)
        .ok_or_else(|| {
            format!("{arm:?}: trial {trial_id:?} is not in contract.conditions.{arm:?}.trial_ids")
        })
}

/// 4 欄が揃えば runner の TokenUsage（input・output は必須）。
fn usage4(i: Option<u64>, o: Option<u64>, cr: Option<u64>, cc: Option<u64>) -> Option<TokenUsage> {
    Some(TokenUsage {
        input: i?,
        output: o?,
        cache_read: cr,
        cache_creation: cc,
    })
}

/// 1 腕分の組み立て結果（trials.jsonl・ledger.jsonl の 1 行と records.jsonl の行）。
struct Arm {
    rec: TrialRecord,
    ledger: LedgerRow,
    records: Vec<Value>,
}

/// out の共通部（manifest hash・入力 hash・snapshot・公開入力）。
struct Ctx<'a> {
    m: &'a CampaignManifest,
    /// lock の measurement_id（停止記録と照合する）。
    measurement_id: String,
    hash: String,
    input_sha: String,
    kb_sha: String,
    inputs: BTreeMap<String, Vec<DeliveredInput>>,
}

impl Ctx<'_> {
    fn problem(&self) -> Result<&str, String> {
        match self.m.problems.as_slice() {
            [p] => Ok(p.as_str()),
            _ => Err("ops-r1 manifest must name exactly one problem".into()),
        }
    }

    fn base(&self, arm: ConfigId, rep: u32, ws_rel: &str) -> Result<TrialRecord, String> {
        let c = self
            .m
            .configs
            .iter()
            .find(|c| c.id == arm)
            .ok_or_else(|| format!("manifest has no config {arm:?}"))?;
        let pid = self.problem()?.to_string();
        Ok(TrialRecord {
            schema: TRIAL_SCHEMA.into(),
            campaign_sha256: self.hash.clone(),
            input_sha256: self.input_sha.clone(),
            split: self.m.split,
            config: arm,
            inputs: self.inputs.get(&pid).cloned().unwrap_or_default(),
            problem_id: pid,
            trial_no: 1,
            rep,
            arm_rank: if arm == ConfigId::A { 1 } else { 2 },
            outcome: Outcome::Failed,
            passed: false,
            tokens: None,
            token_usage: None,
            cost_micro_usd: None,
            wall_secs: 0,
            reason: None,
            error: None,
            routing: None,
            observed: Vec::new(),
            harness: c.harness.clone(),
            harness_version: c.harness_version.clone(),
            model: c.model.clone(),
            model_version: c.model_version.clone(),
            policy_version: c.policy_version.clone(),
            observed_versions: None,
            workspace: ws_rel.into(),
            kb: String::new(),
            workspace_sha256_initial: String::new(),
            kb_sha256_initial: self.kb_sha.clone(),
            workspace_sha256_final: String::new(),
            kb_sha256_final: self.kb_sha.clone(),
        })
    }

    fn ledger(&self, rec: &TrialRecord, started: Option<u64>) -> LedgerRow {
        let key = TrialKey {
            manifest_sha256: self.hash.clone(),
            problem_id: rec.problem_id.clone(),
            arm: rec.config,
            trial_no: rec.trial_no,
            rep: rec.rep,
        };
        LedgerRow {
            schema: LEDGER_SCHEMA.into(),
            idempotency_key: key.idempotency_key(),
            key,
            state: LedgerState::Done,
            task_id: None,
            run_id: None,
            child_task_ids: Vec::new(),
            cluster_job_ids: Vec::new(),
            routing: Value::Null,
            workspace: rec.workspace.clone(),
            submitted_at: started,
            reason: rec.reason.clone(),
            started_at: started,
            observed_usage: Some(ObservedUsage {
                tokens: rec.token_usage,
                wall_secs: rec.wall_secs,
            }),
            reserved: None,
            comm_failures: 0,
            last_error: None,
            versions: rec.observed_versions.clone(),
        }
    }

    /// 成果物 dir を out の workspace に写し、最終 hash を返す。
    fn copy_artifact(&self, out: &Path, rel: &str, src: &Path) -> Result<String, String> {
        if !src.is_dir() {
            return Err(format!("artifact dir {} is missing", src.display()));
        }
        let ws = out.join(rel);
        copy_tree(src, &ws)?;
        hash_tree(&ws)
    }
}

fn ws_rel(arm: ConfigId, problem: &str, rep: u32) -> String {
    format!("workspaces/{arm:?}/{problem}/rep{rep}")
}

/// A（直接 Claude Code）。§4.1・§4.2 の A 列。
fn arm_a(ctx: &Ctx, out: &Path, cond: &ConditionDecl, a: &Args) -> Result<Arm, String> {
    let raw: ops_r1_a_raw::Raw =
        serde_json::from_value(read_json(&a.a_raw)?).map_err(|e| format!("--a-raw: {e}"))?;
    if raw.schema != ops_r1_a_raw::SCHEMA {
        return Err(format!(
            "--a-raw: schema {} is not {}",
            raw.schema,
            ops_r1_a_raw::SCHEMA
        ));
    }
    let md = &raw.metadata;
    if md.condition_id != cond.condition_id {
        return Err(format!(
            "--a-raw: condition_id {:?} differs from contract.conditions.A ({:?})",
            md.condition_id, cond.condition_id
        ));
    }
    let rep = rep_of(cond, &md.trial_id, ConfigId::A)?;
    let pid = ctx.problem()?.to_string();
    let rel = ws_rel(ConfigId::A, &pid, rep);
    let mut rec = ctx.base(ConfigId::A, rep, &rel)?;
    let started = ts(&md.started_at, "--a-raw started_at")?;
    let ended = ts(&md.ended_at, "--a-raw ended_at")?;
    let timeout = ctx.m.termination.trial_timeout_secs;
    rec.workspace_sha256_initial = md.workspace_sha256_initial.clone();
    rec.workspace_sha256_final = ctx.copy_artifact(out, &rel, &a.a_artifact)?;
    let unchanged =
        crate::inputs::artifact_hash(&out.join(&rel), &pid)? == md.workspace_sha256_initial;
    rec.wall_secs = secs_between(started, ended);
    let (outcome, reason) = match (md.stop, md.exit_code) {
        (Stop::Exited, Some(0)) if unchanged => (Outcome::Failed, Some(REASON_NO_ARTIFACT)),
        (Stop::Exited, Some(0)) => (Outcome::Passed, None),
        (Stop::Exited, _) => (Outcome::Failed, Some(REASON_HARNESS_EXIT)),
        (Stop::Cutoff, _) => {
            rec.wall_secs = timeout;
            (Outcome::Timeout, None)
        }
        (Stop::TokenCap, _) => (Outcome::Cancelled, Some(REASON_CASE_BUDGET)),
        (Stop::OperatorStop, _) => (Outcome::Cancelled, Some(REASON_OPERATOR_STOP)),
    };
    rec.outcome = outcome;
    rec.passed = outcome == Outcome::Passed;
    rec.reason = reason.map(String::from);
    let t = &raw.token_usage;
    rec.token_usage = usage4(t.input, t.output, t.cache_read, t.cache_creation);
    rec.tokens = rec
        .token_usage
        .and_then(|u| u.count(ctx.m.budget.token_basis));
    rec.cost_micro_usd = md
        .cost_usd
        .filter(|c| c.is_finite() && *c >= 0.0)
        .map(|c| (c * 1e6).round() as u64);
    rec.observed = vec![ObservedRun {
        run_id: md.session_id.clone(),
        role: "worker".into(),
        harness: Some(A_HARNESS.into()),
        model: match raw.models.as_slice() {
            [m] => Some(m.clone()),
            _ => None,
        },
        ..ObservedRun::default()
    }];
    rec.observed_versions = Some(ObservedVersions {
        claude_code: Some(ProgramPin {
            // 実行 path は出さない（観測した版と sha256 だけ）。
            path: String::new(),
            sha256: md.claude_code.sha256.clone(),
            version: md.claude_code.version.clone(),
        }),
        ..ObservedVersions::default()
    });
    let ledger = ctx.ledger(&rec, epoch(started));
    Ok(Arm {
        rec,
        ledger,
        records: Vec::new(),
    })
}

fn stage<'a>(trace: &'a Value, name: &str) -> Option<&'a Value> {
    trace["stages"]
        .as_array()?
        .iter()
        .find(|s| s["stage"] == name)
}

/// trace の受付が指す CoS の chat run（受付が観測できたときだけ）。
fn intake_cos_run(trace: &Value) -> Option<&str> {
    let s = stage(trace, "intake");
    found_at(s).and(s)?["ref"]["cos_run_id"].as_str()
}

fn found_at(s: Option<&Value>) -> Option<&str> {
    s.filter(|s| s["reason"].is_null())
        .and_then(|s| s["at"].as_str())
}

/// 停止記録（runbook §5 の `raw/stop.json`）。assemble が使う欄だけ読む。
#[derive(Clone, Debug, serde::Deserialize)]
pub struct StopRecord {
    pub schema: String,
    pub measurement_id: String,
    pub stopped_at: String,
    pub stop_id: String,
    /// 止めた試行。試行を始める前の停止は null（assemble では B の試行を指していなければ error）。
    pub trial_id: Option<String>,
    pub decided_by: String,
    #[serde(default)]
    pub decided_by_note: Option<String>,
    #[serde(default)]
    pub what: Option<String>,
}

/// `--b-stop` を読み、schema・measurement_id・trial_id・stop_id・decided_by を検査して reason を返す。
fn load_b_stop(
    p: &Path,
    measurement_id: &str,
    trial: &str,
) -> Result<(StopRecord, &'static str), String> {
    let st: StopRecord =
        serde_json::from_value(read_json(p)?).map_err(|e| format!("--b-stop: {e}"))?;
    if st.schema != STOP_SCHEMA {
        return Err(format!(
            "--b-stop: schema {} is not {STOP_SCHEMA}",
            st.schema
        ));
    }
    if st.measurement_id != measurement_id {
        return Err(format!(
            "--b-stop: measurement_id {:?} differs from the lock's {measurement_id:?}",
            st.measurement_id
        ));
    }
    if st.trial_id.as_deref() != Some(trial) {
        return Err(format!(
            "--b-stop: trial_id {:?} is not the B trial {trial:?}",
            st.trial_id
        ));
    }
    let reason = STOP_REASONS
        .iter()
        .copied()
        .find(|r| *r == st.stop_id)
        .ok_or_else(|| format!("--b-stop: unknown stop_id {:?}", st.stop_id))?;
    if st.decided_by.trim().is_empty() {
        return Err("--b-stop: decided_by is empty".into());
    }
    Ok((st, reason))
}

/// B（CoS 受付）。§4.1・§4.2・§4.4 の B 列。停止記録があれば §4.5。
fn arm_b(ctx: &Ctx, out: &Path, cond: &ConditionDecl, a: &Args) -> Result<Arm, String> {
    let intake = read_json(&a.b_intake)?;
    let trace = read_json(&a.b_trace)?;
    let attr = read_json(&a.b_attribution)?;
    for (what, v, schema) in [
        ("--b-intake", &intake, crate::ops_r1_intake::SCHEMA),
        ("--b-trace", &trace, crate::ops_r1_trace::SCHEMA),
        ("--b-attribution", &attr, crate::ops_r1_attr::SCHEMA),
    ] {
        if v["schema"] != schema {
            return Err(format!("{what}: schema {} is not {schema}", v["schema"]));
        }
    }
    let trial = intake["trial_id"].as_str().unwrap_or_default().to_string();
    if trace["trial_id"] != trial.as_str() || attr["trial_id"] != trial.as_str() {
        return Err(format!(
            "B: intake/trace/attribution trial_id differ ({:?}, {}, {})",
            trial, trace["trial_id"], attr["trial_id"]
        ));
    }
    for (what, v) in [("--b-intake", &intake), ("--b-attribution", &attr)] {
        if v["condition_id"] != cond.condition_id.as_str() {
            return Err(format!(
                "{what}: condition_id {} differs from contract.conditions.B ({:?})",
                v["condition_id"], cond.condition_id
            ));
        }
    }
    let rep = rep_of(cond, &trial, ConfigId::B)?;
    let stop = a
        .b_stop
        .as_deref()
        .map(|p| load_b_stop(p, &ctx.measurement_id, &trial))
        .transpose()?;
    let all_rows = attr["rows"]
        .as_array()
        .ok_or("--b-attribution: rows missing")?;
    let mut rows: Vec<&Value> = all_rows
        .iter()
        .filter(|r| r["trial_id"] == trial.as_str())
        .collect();
    // 停止（§4.5）: root task が無いと attribution は CoS の chat run を試行に帰属させない（trial_id null）。
    // trace の受付が指す CoS run（intake.ref.cos_run_id）だけを共有の CoS 行として足す。
    if stop.is_some()
        && let Some(id) = intake_cos_run(&trace)
        && !rows.iter().any(|r| r["run_id"] == id)
    {
        rows.extend(
            all_rows
                .iter()
                .filter(|r| r["source"] == "chat_runs" && r["run_id"] == id),
        );
    }
    check_cos_model(cond, &rows)?;
    if !is_hex64(&a.b_workspace_sha256_initial) {
        return Err("--b-workspace-sha256-initial must be a sha256 hex".into());
    }
    let observed_versions: ObservedVersions =
        serde_json::from_value(read_json(&a.b_observed_versions)?)
            .map_err(|e| format!("--b-observed-versions: {e}"))?;
    let pid = ctx.problem()?.to_string();
    let rel = ws_rel(ConfigId::B, &pid, rep);
    let mut rec = ctx.base(ConfigId::B, rep, &rel)?;
    rec.workspace_sha256_initial = a.b_workspace_sha256_initial.to_ascii_lowercase();
    rec.workspace_sha256_final = ctx.copy_artifact(out, &rel, &a.b_artifact)?;
    rec.observed_versions = Some(observed_versions);

    // 終了理由と wall（§4.2）。outcome が無い・cutoff 後は timeout。
    let accepted = ts(
        intake["accepted_at"].as_str().unwrap_or_default(),
        "--b-intake accepted_at",
    )?;
    let timeout = ctx.m.termination.trial_timeout_secs;
    let outcome_stage = stage(&trace, "outcome");
    let to = outcome_stage.and_then(|s| s["ref"]["to"].as_str());
    let before_cutoff =
        stage(&trace, "cutoff").is_some_and(|s| s["ref"]["report_before_cutoff"] == true);
    let at = |s: Option<&str>| s.map(|x| ts(x, "--b-trace stage at")).transpose();
    let (outcome, reason, end) = match (to, found_at(outcome_stage)) {
        (Some("done"), Some(_)) if before_cutoff => (
            Outcome::Passed,
            None,
            at(found_at(stage(&trace, "report")))?,
        ),
        (Some("failed"), Some(t)) => (Outcome::Failed, Some(REASON_TASK_FAILED), at(Some(t))?),
        (Some("cancelled"), Some(t)) => (
            Outcome::Cancelled,
            Some(REASON_TASK_CANCELLED),
            at(Some(t))?,
        ),
        _ => (Outcome::Timeout, None, None),
    };
    rec.wall_secs = match (outcome, end) {
        (Outcome::Timeout, _) | (_, None) => timeout,
        (_, Some(e)) => secs_between(accepted, e).min(timeout),
    };
    rec.outcome = outcome;
    rec.reason = reason.map(String::from);

    // 消費: task の run（worker・planner・reviewer）の行の和（1 行でも欠測の欄はその欄ごと欠測）。
    // CoS の chat run は final-accounting の CoS 欄（shared_overhead）に入れ、試行の外側の値には足さない
    // （final-accounting は試行の tokens と worker・planner・reviewer の和を照合する）。
    let task_rows: Vec<&Value> = rows
        .iter()
        .copied()
        .filter(|r| r["source"] == "runs")
        .collect();
    let sum = |k: &str| -> Option<u64> {
        task_rows
            .iter()
            .map(|r| r[k].as_u64())
            .try_fold(0u64, |acc, x| x.map(|x| acc + x))
    };
    rec.token_usage = usage4(
        sum("input_tokens"),
        sum("output_tokens"),
        sum("cache_read_tokens"),
        sum("cache_write_tokens"),
    );
    rec.tokens = rec
        .token_usage
        .and_then(|u| u.count(ctx.m.budget.token_basis));
    rec.cost_micro_usd = task_rows
        .iter()
        .map(|r| {
            r["cost_usd"]
                .as_f64()
                .filter(|c| c.is_finite() && *c >= 0.0)
        })
        .try_fold(0.0f64, |acc, x| x.map(|x| acc + x))
        .filter(|_| !task_rows.is_empty())
        .map(|c| (c * 1e6).round() as u64);
    // 1 試行の上限を超えたものは case_budget で止めた扱い（timeout はそのまま）。
    if rec.outcome != Outcome::Timeout
        && rec
            .tokens
            .is_some_and(|t| t > ctx.m.budget.per_case.max_tokens)
    {
        rec.outcome = Outcome::Cancelled;
        rec.reason = Some(REASON_CASE_BUDGET.into());
    }
    // 停止（§4.5）: trace の終わり方にかかわらず、停止の reason で成功に数えない（A の operator_stop と同じ cancelled）。
    // wall は受付から停止（cutoff が先ならその時刻）までの実測。
    if let Some((st, reason)) = &stop {
        let stopped = ts(&st.stopped_at, "--b-stop stopped_at")?;
        let end = match trace["cutoff"].as_str() {
            Some(c) => stopped.min(ts(c, "--b-trace cutoff")?),
            None => stopped,
        };
        if end < accepted {
            return Err(format!(
                "--b-stop: stopped_at {} is before the intake {}",
                st.stopped_at,
                intake["accepted_at"].as_str().unwrap_or_default()
            ));
        }
        rec.outcome = Outcome::Cancelled;
        rec.reason = Some((*reason).into());
        rec.wall_secs = secs_between(accepted, end);
    }
    rec.passed = rec.outcome == Outcome::Passed;
    rec.observed = task_rows
        .iter()
        .map(|r| ObservedRun {
            run_id: r["run_id"].as_str().unwrap_or_default().into(),
            role: r["role"].as_str().unwrap_or_default().into(),
            harness: r["harness"].as_str().map(String::from),
            model: r["model"].as_str().map(String::from),
            ..ObservedRun::default()
        })
        .collect();

    // 台帳: root task と最初の worker run、子 task。
    let created = stage(&trace, "created").map(|s| &s["ref"]);
    let executed = stage(&trace, "executed").map(|s| &s["ref"]);
    let root = created
        .and_then(|r| r["root_task_id"].as_str())
        .map(String::from);
    let mut ledger = ctx.ledger(&rec, epoch(accepted));
    ledger.task_id = root.clone();
    ledger.run_id = executed
        .and_then(|e| e["runs"].as_array())
        .and_then(|runs| runs.iter().find(|r| r["role"] == "worker"))
        .and_then(|r| r["run_id"].as_str())
        .map(String::from);
    let mut children: Vec<String> = Vec::new();
    for v in [
        created.and_then(|r| r["extra_root_task_ids"].as_array()),
        executed.and_then(|e| e["task_ids"].as_array()),
    ]
    .into_iter()
    .flatten()
    .flatten()
    {
        if let Some(t) = v.as_str()
            && Some(t) != root.as_deref()
            && !children.iter().any(|c| c == t)
        {
            children.push(t.into());
        }
    }
    ledger.child_task_ids = children;

    let mut records = decision_log(ctx, &rec, &rows)?;
    if let Some((st, _)) = &stop {
        records.push(stop_record(ctx, &rec, st, &trace));
    }
    Ok(Arm {
        rec,
        ledger,
        records,
    })
}

/// CoS（chat_runs の行）の model が宣言と一致するか。欠測も逸脱として止める。
fn check_cos_model(cond: &ConditionDecl, rows: &[&Value]) -> Result<(), String> {
    let want = cond
        .cos_model
        .as_deref()
        .ok_or("contract.conditions.B.cos_model is required to check the CoS model")?;
    for r in rows.iter().filter(|r| r["source"] == "chat_runs") {
        match r["model"].as_str() {
            Some(m) if m == want => {}
            got => {
                return Err(format!(
                    "condition deviation: CoS run {} ran model {got:?}, contract.conditions.B.cos_model is {want:?}",
                    r["run_id"]
                ));
            }
        }
    }
    Ok(())
}

/// attribution の行を decision-log/1 の usage 行と run 行に写す（§4.4）。
fn decision_log(ctx: &Ctx, rec: &TrialRecord, rows: &[&Value]) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for r in rows {
        let run_id = r["run_id"]
            .as_str()
            .ok_or("--b-attribution: row without run_id")?;
        let common = json!({
            "schema": RECORD_SCHEMA,
            "campaign_id": ctx.m.campaign_id,
            "manifest_hash": ctx.hash,
            "arm": format!("{:?}", rec.config),
            "case_id": rec.problem_id,
            "repetition": rec.rep,
            "run_id": run_id,
            "task_id": r["task_id"],
            "role": r["role"],
            "dedup_key": format!("ops-r1:{run_id}"),
        });
        // CoS の chat run は task に属さない共有の消費（role cos・shared_overhead）。run 行は書かない。
        let cos = r["source"] == "chat_runs";
        let mut usage = common.clone();
        let list = if r["cost_basis"] == "pricing_estimate" {
            r["cost_usd"].clone()
        } else {
            Value::Null
        };
        if let Some(o) = usage.as_object_mut() {
            o.insert("record_type".into(), json!("usage"));
            o.insert("input_tokens".into(), r["input_tokens"].clone());
            o.insert("output_tokens".into(), r["output_tokens"].clone());
            o.insert("cache_read_tokens".into(), r["cache_read_tokens"].clone());
            o.insert(
                "cache_creation_tokens".into(),
                r["cache_write_tokens"].clone(),
            );
            o.insert("list_cost_usd".into(), list);
            o.insert("is_retry".into(), json!(false));
            o.insert("shared_overhead".into(), json!(cos));
            if cos {
                o.insert("role".into(), json!("cos"));
                o.insert("cos_role".into(), r["role"].clone());
            }
        }
        out.push(usage);
        if cos {
            continue;
        }
        let mut run = common;
        if let Some(o) = run.as_object_mut() {
            o.insert("record_type".into(), json!("run"));
        }
        out.push(run);
    }
    Ok(out)
}

/// 停止の行（§4.5）。誰が止めたか（decided_by・decided_by_note）と、trace の受付に見えた CoS の chat run を role 付きで残す。
/// CoS の run は試行の `observed`（task の run の照合先）には入れない（§4.5）。
fn stop_record(ctx: &Ctx, rec: &TrialRecord, st: &StopRecord, trace: &Value) -> Value {
    let cos_runs: Vec<Value> = intake_cos_run(trace)
        .map(|id| json!({"run_id": id, "role": "cos_project", "source": "trace.intake"}))
        .into_iter()
        .collect();
    json!({
        "schema": RECORD_SCHEMA,
        "record_type": STOP_RECORD_TYPE,
        "campaign_id": ctx.m.campaign_id,
        "manifest_hash": ctx.hash,
        "arm": format!("{:?}", rec.config),
        "case_id": rec.problem_id,
        "repetition": rec.rep,
        "trial_id": st.trial_id,
        "stop_schema": st.schema,
        "stop_id": st.stop_id,
        "reason": rec.reason,
        "stopped_at": st.stopped_at,
        "decided_by": st.decided_by,
        "decided_by_note": st.decided_by_note,
        "what": st.what,
        "observed_cos_runs": cos_runs,
        "dedup_key": format!("ops-r1-stop:{}", st.trial_id.as_deref().unwrap_or_default()),
    })
}

/// out と records.jsonl を作り、件数と出力の sha256 を返す。
pub fn assemble(a: &Args) -> Result<Value, String> {
    let m = CampaignManifest::load(&a.manifest)?;
    m.validate()?;
    if !m.is_ops_r1() {
        return Err("ops-r1-assemble needs an ops-r1 manifest (contract.conditions)".into());
    }
    let conds = m.conditions();
    let (Some(ca), Some(cb)) = (conds.get(&ConfigId::A), conds.get(&ConfigId::B)) else {
        return Err("contract.conditions must declare A and B".into());
    };
    let input = m.load_input(&a.manifest, Some(&a.input_manifest))?;
    if input.sha256 != m.input.sha256 {
        return Err(format!(
            "--input-manifest sha256 {} differs from the manifest's input pin {}",
            input.sha256, m.input.sha256
        ));
    }
    let lock = lock_exec::expected_lock(
        &lock_exec::LockSpec {
            cases_root: &a.cases_root,
            grader_root: &a.grader_root,
            grade_cases_root: &a.grade_case_root,
            measurement_id: a.measurement_id.as_deref(),
        },
        &m,
    )?;
    let out = &a.out;
    if out.exists()
        && fs::read_dir(out)
            .map_err(|e| format!("{}: {e}", out.display()))?
            .next()
            .is_some()
    {
        return Err(format!("--out {} must be empty", out.display()));
    }
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let hash = m.compute_sha256()?;
    let ctx = Ctx {
        m: &m,
        measurement_id: lock.measurement_id.clone(),
        hash: hash.clone(),
        input_sha: input.sha256.clone(),
        kb_sha: m.snapshot.kb_snapshot.clone(),
        inputs: lock_exec::locked_inputs(&lock),
    };
    let arms = [arm_a(&ctx, out, ca, a)?, arm_b(&ctx, out, cb, a)?];
    // 両腕は同じ初期状態から始まる（checker が snapshot の seed hash と照合する値）。
    let seed = arms[0].rec.workspace_sha256_initial.clone();
    if arms[1].rec.workspace_sha256_initial != seed {
        return Err(format!(
            "condition deviation: A and B started from different workspaces ({} vs {})",
            seed, arms[1].rec.workspace_sha256_initial
        ));
    }
    lock_exec::save(out, &lock)?;
    write_json(
        &out.join(CAMPAIGN_FILE),
        &serde_json::to_value(m.normalized()).map_err(|e| e.to_string())?,
    )?;
    write_json(
        &out.join(INPUT_FILE),
        &serde_json::to_value(&input).map_err(|e| e.to_string())?,
    )?;
    let snap = out.join(SNAPSHOT_FILE);
    if let Some(d) = snap.parent() {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let mut inputs_sha = BTreeMap::new();
    for i in ctx.inputs.values().flatten() {
        inputs_sha.insert(i.name.clone(), i.sha256.clone());
    }
    write_json(
        &snap,
        &json!({
            "repo_commit": m.snapshot.repo_commit,
            "kb_snapshot": m.snapshot.kb_snapshot,
            // 資料・KB は試行の前に測定者が配ったもの。assemble は再 hash できないので lock の固定値を写す。
            "materials_sha256": lock.materials_sha256,
            "kb_sha256": ctx.kb_sha,
            "workspace_seed_sha256": seed,
            "workspace_seed_empty_sha256": Value::Null,
            "materials_pin_verified": false,
            "inputs_sha256": inputs_sha,
        }),
    )?;
    write_json(
        &out.join(META_FILE),
        &json!({
            "schema": "celeris.campaign-run-meta/1",
            "campaign_id": m.campaign_id,
            "campaign_sha256": hash,
            "plan_identity": m.plan_identity()?,
            "input_sha256": input.sha256,
            "split": m.split,
            "executor": EXECUTOR,
            "evidence_kind": crate::record::evidence_kind(false),
            "real_performance": true,
            "locked": true,
            "configs": m.normalized().configs,
            "budget": m.budget,
            "termination": m.termination,
            "snapshot": m.snapshot,
            "expected_trials": crate::runner::plan(&m).len(),
            "order": m.order,
            "seed": m.seed,
        }),
    )?;
    let mut trials = String::new();
    let mut ledger = String::new();
    let mut records = String::new();
    for arm in &arms {
        trials.push_str(&serde_json::to_string(&arm.rec).map_err(|e| e.to_string())?);
        trials.push('\n');
        ledger.push_str(&serde_json::to_string(&arm.ledger).map_err(|e| e.to_string())?);
        ledger.push('\n');
        for r in &arm.records {
            records.push_str(&serde_json::to_string(r).map_err(|e| e.to_string())?);
            records.push('\n');
        }
    }
    fs::write(out.join(TRIALS_FILE), &trials).map_err(|e| e.to_string())?;
    fs::write(out.join(LEDGER_FILE), &ledger).map_err(|e| e.to_string())?;
    let recs: Vec<TrialRecord> = arms.iter().map(|x| x.rec.clone()).collect();
    crate::runner::write_assembled_summary(&m, &hash, &recs, out)?;
    fs::write(&a.records_out, &records).map_err(|e| format!("{}: {e}", a.records_out.display()))?;
    Ok(json!({
        "executor": EXECUTOR,
        "campaign_sha256": hash,
        "trials": recs.len(),
        "outcomes": recs.iter().map(|r| json!({
            "config": r.config, "rep": r.rep, "outcome": r.outcome, "reason": r.reason,
        })).collect::<Vec<_>>(),
        "records": records.lines().count(),
        "records_sha256": crate::ops_r1_intake::sha256_hex(records.as_bytes()),
        "trials_sha256": crate::ops_r1_intake::sha256_hex(trials.as_bytes()),
    }))
}

/// CLI の入口。`args[0]` は `ops-r1-assemble`。
pub fn cli(args: &[String]) -> Result<String, String> {
    let a = Args::parse(args)?;
    serde_json::to_string_pretty(&assemble(&a)?).map_err(|e| e.to_string())
}
