//! 評価側の PBS 終了記録収集。`celeris.observed-jobs/1` を、trial の自己申告ではなく
//! 投入先（Celeris が台帳に残した `scheduler:cluster:job_id`）・実 scheduler の応答・case の条件・raw の hash から作る。
//! 生成物は `grade-out` / `grade --observed` が読む `<out>/observed/<problem>/<Config>-rep<N>.json`。
//! scheduler への問い合わせは read-only の `SchedulerTransport` の背後に置く（新 daemon・scheduler は作らない）。
use std::{collections::BTreeMap, fs, path::Path};

use serde_json::{Map, Value, json};

use crate::{
    grader::{GradeCase, evidence::OBSERVED_SCHEMA, safe_join, sha256_file},
    ledger::{Ledger, TrialKey as LedgerKey},
    manifest::{CampaignManifest, ConfigId},
    record::{TRIALS_FILE, TrialRecord},
    snapshot::hash_tree,
};

/// 台帳（`cluster_job_ids`）の `scheduler:cluster:job_id`。trial の自己申告ではなく Celeris が観測した投入先。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRef {
    pub scheduler: String,
    pub cluster: String,
    pub job_id: String,
}

impl JobRef {
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut it = s.splitn(3, ':');
        match (it.next(), it.next(), it.next()) {
            (Some(sch), Some(c), Some(id)) if !sch.is_empty() && !c.is_empty() => Ok(Self {
                scheduler: sch.into(),
                cluster: c.into(),
                job_id: id.into(),
            }),
            _ => Err(format!("{s}: not a scheduler:cluster:job_id reference")),
        }
    }
}

/// 取得元の種別。合成（試験の固定応答）は本番証拠と区別して記録する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Live,
    Synthetic,
}

/// scheduler の応答。`cluster` は実際に答えた取得元（要求した cluster と一致しなければ拒否）。
#[derive(Clone, Debug)]
pub struct SchedulerReply {
    pub cluster: String,
    pub raw: String,
}

/// read-only の scheduler 照会（`qstat -x -f <id>` 相当の本文を返す）。`Err` は取得不能。
pub trait SchedulerTransport {
    fn origin(&self) -> Origin;
    fn qstat(&self, scheduler: &str, cluster: &str, job_id: &str)
    -> Result<SchedulerReply, String>;
}

/// 投入前に固定した期待。`conditions` は case の `experiment.conditions`（queue・ncpus 等）。
#[derive(Clone, Debug)]
pub struct Expect {
    pub scheduler: String,
    pub cluster: String,
    pub conditions: BTreeMap<String, Value>,
}

/// PBS の `key = value`（継続行は前行へ連結）。
fn parse_qstat(text: &str) -> (Option<String>, BTreeMap<String, String>) {
    let mut id = None;
    let mut kv: BTreeMap<String, String> = BTreeMap::new();
    let mut last: Option<String> = None;
    for l in text.lines() {
        let t = l.trim();
        if let Some(r) = t.strip_prefix("Job Id:") {
            id = Some(r.trim().to_string());
            continue;
        }
        if let Some((k, v)) = t.split_once(" = ")
            && !l.starts_with('\t')
            && !k.contains(' ')
        {
            kv.insert(k.to_string(), v.trim().to_string());
            last = Some(k.to_string());
        } else if l.starts_with('\t')
            && let Some(k) = &last
            && let Some(v) = kv.get_mut(k)
        {
            v.push_str(t);
        }
    }
    (id, kv)
}

fn typed(v: &str) -> Value {
    v.parse::<i64>()
        .map(Value::from)
        .unwrap_or_else(|_| v.into())
}

/// PBS の job id は `<数字>.<server>`。host を欠く数字だけ（PID に見えるもの）は scheduler id とみなさない。
pub(crate) fn is_scheduler_job_id(id: &str) -> bool {
    id.split_once('.').is_some_and(|(n, h)| {
        !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    })
}

/// 1 job を照会して記録 1 件にする。未終了・id/取得元の不一致・欠落は Err。exit_status と条件は観測のまま記録し、
/// 合否は grader が case と照合して決める。
fn observe_job(
    t: &dyn SchedulerTransport,
    r: &JobRef,
    expect: &Expect,
    artifact: &Path,
    raw_paths: &[String],
    raw_dir: &Path,
) -> Result<Value, String> {
    let tag = format!("{}:{}:{}", r.scheduler, r.cluster, r.job_id);
    if !is_scheduler_job_id(&r.job_id) {
        return Err(format!(
            "{tag}: not a scheduler job id (PIDs are not accepted)"
        ));
    }
    if r.scheduler != expect.scheduler || r.cluster != expect.cluster {
        return Err(format!(
            "{tag}: submitted to {}:{} but the case fixes {}:{}",
            r.scheduler, r.cluster, expect.scheduler, expect.cluster
        ));
    }
    let reply = t
        .qstat(&r.scheduler, &r.cluster, &r.job_id)
        .map_err(|e| format!("{tag}: scheduler query failed: {e}"))?;
    if reply.cluster != r.cluster {
        return Err(format!(
            "{tag}: answered by {} not the submission cluster {}",
            reply.cluster, r.cluster
        ));
    }
    let (rid, kv) = parse_qstat(&reply.raw);
    if rid.as_deref() != Some(r.job_id.as_str()) {
        return Err(format!(
            "{tag}: scheduler record is for {rid:?}, not this job id"
        ));
    }
    if kv.get("job_state").map(String::as_str) != Some("F") {
        return Err(format!(
            "{tag}: job is not finished (job_state={:?})",
            kv.get("job_state")
        ));
    }
    let exit: i64 = kv
        .get("Exit_status")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("{tag}: scheduler record has no Exit_status"))?;
    let mut conditions = Map::new();
    for k in expect.conditions.keys() {
        let v = kv
            .get(&format!("Resource_List.{k}"))
            .or_else(|| kv.get(k.as_str()));
        if let Some(v) = v {
            conditions.insert(k.clone(), typed(v));
        }
    }
    let mut raw = Vec::new();
    for p in raw_paths {
        let f = safe_join(artifact, p).map_err(|e| format!("{tag}: {e}"))?;
        if !f.is_file() {
            return Err(format!("{tag}: raw data {p} is missing"));
        }
        raw.push(json!({"path": p, "sha256": sha256_file(&f)?}));
    }
    if raw.is_empty() {
        return Err(format!("{tag}: no raw data to hash"));
    }
    fs::create_dir_all(raw_dir).map_err(|e| e.to_string())?;
    let file = raw_dir.join(format!("{}.qstat", r.job_id));
    fs::write(&file, &reply.raw).map_err(|e| e.to_string())?;
    Ok(json!({
        "job_id": r.job_id,
        "scheduler": r.scheduler,
        "cluster": r.cluster,
        "source": reply.cluster,
        "exit_status": exit,
        "conditions": Value::Object(conditions),
        "raw": raw,
        "scheduler_record_sha256": sha256_file(&file)?,
    }))
}

/// 標準の収集呼出し。`refs` は Celeris 台帳の投入先（空＝未実行は拒否）。`raw_paths` は成果物内の raw（hash は評価側が計算）。
/// 全 job が揃ったときだけ `celeris.observed-jobs/1` を返す。1 件でも拒否なら Err（部分記録は作らない）。
pub fn collect(
    t: &dyn SchedulerTransport,
    refs: &[JobRef],
    expect: &Expect,
    artifact: &Path,
    raw_paths: &[String],
    raw_dir: &Path,
) -> Result<Value, Vec<String>> {
    if refs.is_empty() {
        return Err(vec![
            "no job was submitted by Celeris for this trial (not executed)".into(),
        ]);
    }
    let (mut jobs, mut errs) = (Vec::new(), Vec::new());
    for r in refs {
        match observe_job(t, r, expect, artifact, raw_paths, raw_dir) {
            Ok(j) => jobs.push(j),
            Err(e) => errs.push(e),
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    Ok(json!({
        "schema": OBSERVED_SCHEMA,
        "origin": match t.origin() { Origin::Live => "live", Origin::Synthetic => "synthetic" },
        "jobs": jobs,
    }))
}

/// 収集対象の試行（`grade-out` の記録 path の鍵）。
#[derive(Clone, Copy, Debug)]
pub struct TrialKey<'a> {
    pub problem: &'a str,
    pub config: ConfigId,
    pub rep: u32,
}

/// `grade-out` が読む場所（`<out>/observed/<problem>/<Config>-rep<N>.json`）へ収集して書く。拒否時は記録を書かない。
///
/// `identity`（[`trial_identity`]）は記録の `trial` に入り、`grade-out` が当該 trial の記録と照合する。
#[allow(clippy::too_many_arguments)]
pub fn collect_for_trial(
    t: &dyn SchedulerTransport,
    out: &Path,
    key: TrialKey<'_>,
    identity: &Value,
    refs: &[JobRef],
    expect: &Expect,
    artifact: &Path,
    raw_paths: &[String],
) -> Result<std::path::PathBuf, Vec<String>> {
    let dir = out.join("observed").join(key.problem);
    let stem = format!("{:?}-rep{}", key.config, key.rep);
    let raw_dir = dir.join("scheduler").join(&stem);
    let mut doc = collect(t, refs, expect, artifact, raw_paths, &raw_dir)?;
    doc["trial"] = identity.clone();
    let path = dir.join(format!("{stem}.json"));
    fs::write(&path, doc.to_string()).map_err(|e| vec![e.to_string()])?;
    Ok(path)
}

/// 観測記録に結ぶ trial の identity（campaign sha256・arm・problem・rep・trial_no）。
pub fn trial_identity(
    campaign_sha256: &str,
    config: ConfigId,
    problem_id: &str,
    rep: u32,
    trial_no: u32,
) -> Value {
    json!({
        "campaign_sha256": campaign_sha256,
        "config": format!("{config:?}"),
        "problem_id": problem_id,
        "rep": rep,
        "trial_no": trial_no,
    })
}

/// 台帳の job ID（`scheduler:cluster:job_id`、または PBS の素の `<n>.<server>`）を投入先の参照にする。
fn ledger_ref(id: &str, cluster: &str) -> Result<JobRef, String> {
    if id.contains(':') {
        JobRef::parse(id)
    } else {
        Ok(JobRef {
            scheduler: "pbs".into(),
            cluster: cluster.into(),
            job_id: id.into(),
        })
    }
}

fn ledger_row<'a>(
    ledger: &'a Ledger,
    hash: &str,
    r: &TrialRecord,
) -> Option<&'a crate::ledger::LedgerRow> {
    ledger.get(&LedgerKey {
        manifest_sha256: hash.into(),
        problem_id: r.problem_id.clone(),
        arm: r.config,
        trial_no: r.trial_no,
        rep: r.rep,
    })
}

/// `grade-out` が観測記録を使う前の照合。記録の `trial` が当該 trial の identity と同じ、記録の job 集合が Celeris の台帳に残した
/// 投入先と同じ、各 job の scheduler 応答の控えが記録した hash と同じ、であること。別 trial・別 campaign の記録、台帳に無い job、
/// 改変された控えは Err（その trial は成功証拠にしない）。
pub(crate) fn verify_binding(
    out: &Path,
    path: &Path,
    ledger: &Ledger,
    r: &TrialRecord,
    hash: &str,
) -> Result<(), String> {
    let doc: Value = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let want = trial_identity(hash, r.config, &r.problem_id, r.rep, r.trial_no);
    if doc["trial"] != want {
        return Err(format!(
            "observation {} belongs to {}, not this trial {want}",
            path.display(),
            doc["trial"]
        ));
    }
    let row = ledger_row(ledger, hash, r).ok_or("no ledger row for this trial")?;
    let ledger_ids: std::collections::BTreeSet<String> = row
        .cluster_job_ids
        .iter()
        .map(|i| i.rsplit(':').next().unwrap_or(i).to_string())
        .collect();
    let jobs = doc["jobs"].as_array().ok_or("observation has no jobs")?;
    let seen: std::collections::BTreeSet<String> = jobs
        .iter()
        .filter_map(|j| j["job_id"].as_str().map(String::from))
        .collect();
    if ledger_ids.is_empty() || seen != ledger_ids {
        return Err(format!(
            "observed jobs {seen:?} differ from the jobs Celeris submitted for this trial {ledger_ids:?}"
        ));
    }
    let stem = format!("{:?}-rep{}", r.config, r.rep);
    for id in &seen {
        let f = out
            .join("observed")
            .join(&r.problem_id)
            .join("scheduler")
            .join(&stem)
            .join(format!("{id}.qstat"));
        let sha = sha256_file(&f).map_err(|e| format!("{id}: scheduler record: {e}"))?;
        let rec = jobs
            .iter()
            .find(|j| j["job_id"].as_str() == Some(id))
            .and_then(|j| j["scheduler_record_sha256"].as_str());
        if rec != Some(sha.as_str()) {
            return Err(format!(
                "{id}: scheduler record differs from the hash recorded at collection (altered)"
            ));
        }
    }
    Ok(())
}

/// `collect-observed-out --manifest <m> --out <out> --cases-root <dir> --grader-root <dir> --grade-case-root <dir>`:
/// out の実行済み trial のうち実験工程のある case について、台帳の job ID・case の cluster・grade-case の条件・成果物内の raw から
/// `<out>/observed/…` を作る（trial identity 付き）。成果物が実行終了時の hash と違う・job 未終了・欠測は記録を作らず、
/// 失敗した trial を報告して Err を返す。
pub fn collect_out(
    m: &CampaignManifest,
    out: &Path,
    cases: &crate::cases::CaseSet,
    grade_cases: &Path,
    t: &dyn SchedulerTransport,
) -> Result<String, String> {
    let hash = m.compute_sha256()?;
    let ledger = Ledger::open_readonly(out)?;
    let text = fs::read_to_string(out.join(TRIALS_FILE))
        .map_err(|e| format!("{}: {e}", out.join(TRIALS_FILE).display()))?;
    let (mut collected, mut failed, mut skipped) = (Vec::new(), Vec::new(), 0u32);
    for (i, l) in text.lines().enumerate() {
        let r: TrialRecord =
            serde_json::from_str(l).map_err(|e| format!("trials.jsonl line {}: {e}", i + 1))?;
        if r.campaign_sha256 != hash {
            return Err(format!(
                "trials.jsonl line {}: record of another campaign",
                i + 1
            ));
        }
        if !r.ran() || r.workspace.is_empty() {
            continue;
        }
        let Some(exp) = cases
            .get(&r.problem_id)
            .and_then(|c| c.case.experiment.as_ref())
        else {
            skipped += 1;
            continue;
        };
        let tag = format!(
            "{:?}/{}/rep{}#{}",
            r.config, r.problem_id, r.rep, r.trial_no
        );
        let fail = |why: Vec<String>| json!({"trial": tag, "reasons": why});
        let gpath = crate::grade_out::resolve_case_path(m, grade_cases, &r.problem_id, true);
        let gc = match GradeCase::load(&gpath) {
            Ok(g) => g,
            Err(e) => {
                failed.push(fail(vec![e]));
                continue;
            }
        };
        let Some(spec) = gc.experiment else {
            skipped += 1;
            continue;
        };
        let artifact = out.join(&r.workspace);
        match hash_tree(&artifact) {
            Ok(h) if h == r.workspace_sha256_final => {}
            _ => {
                failed.push(fail(vec![
                    "artifact hash differs from the trial's recorded final workspace".into(),
                ]));
                continue;
            }
        }
        let refs: Result<Vec<JobRef>, String> = ledger_row(&ledger, &hash, &r)
            .map(|row| {
                row.cluster_job_ids
                    .iter()
                    .map(|i| ledger_ref(i, &exp.cluster))
                    .collect()
            })
            .unwrap_or_else(|| Err("no ledger row for this trial".into()));
        let refs = match refs {
            Ok(v) => v,
            Err(e) => {
                failed.push(fail(vec![e]));
                continue;
            }
        };
        let raw_paths: Vec<String> = safe_join(&artifact, &spec.jobs_file)
            .ok()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|d| {
                let mut v: Vec<String> = d["jobs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|j| j["raw"].as_array().into_iter().flatten())
                    .filter_map(|x| x["path"].as_str().map(String::from))
                    .collect();
                v.sort();
                v.dedup();
                v
            })
            .unwrap_or_default();
        let expect = Expect {
            scheduler: "pbs".into(),
            cluster: exp.cluster.clone(),
            conditions: spec.conditions.clone(),
        };
        match collect_for_trial(
            t,
            out,
            TrialKey {
                problem: &r.problem_id,
                config: r.config,
                rep: r.rep,
            },
            &trial_identity(&hash, r.config, &r.problem_id, r.rep, r.trial_no),
            &refs,
            &expect,
            &artifact,
            &raw_paths,
        ) {
            Ok(p) => collected.push(json!({"trial": tag, "path": p.display().to_string()})),
            Err(e) => failed.push(fail(e)),
        }
    }
    let rep = json!({
        "schema": "celeris.campaign-collect-out/1",
        "ok": failed.is_empty(),
        "collected": collected,
        "failed": failed,
        "skipped_no_experiment": skipped,
    });
    let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
    if failed.is_empty() { Ok(s) } else { Err(s) }
}

/// `celeris-campaign collect-observed --case <case.json> --grade-case <grade-case.json> --artifact <dir> --out <dir>
/// --problem <id> --config a|b|c [--rep N] --campaign-sha256 <hex> --trial-no N --job <scheduler:cluster:job_id>... --raw <artifact 内 path>...`。
/// cluster は case.json の `experiment.cluster`、条件は grade case の `experiment.conditions` から取り、引数では変えない。
pub fn cli(args: &[String], t: &dyn SchedulerTransport) -> Result<String, String> {
    let one = |n: &str| {
        args.iter()
            .position(|a| a == n)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or(format!("{n} is required"))
    };
    let many = |n: &str| -> Vec<String> {
        args.iter()
            .enumerate()
            .filter(|(_, a)| *a == n)
            .filter_map(|(i, _)| args.get(i + 1).cloned())
            .collect()
    };
    let read = |p: &str| -> Result<Value, String> {
        serde_json::from_str(&fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?)
            .map_err(|e| format!("{p}: {e}"))
    };
    let case = read(&one("--case")?)?;
    let gcase = read(&one("--grade-case")?)?;
    let cluster = case["experiment"]["cluster"]
        .as_str()
        .ok_or("case.json has no experiment.cluster")?
        .to_string();
    let conditions: BTreeMap<String, Value> = gcase["experiment"]["conditions"]
        .as_object()
        .ok_or("grade case has no experiment.conditions")?
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let config = match one("--config")?.as_str() {
        "a" => ConfigId::A,
        "b" => ConfigId::B,
        "c" => ConfigId::C,
        o => return Err(format!("--config {o}: expected a|b|c")),
    };
    let rep = match args.iter().position(|a| a == "--rep") {
        Some(i) => args
            .get(i + 1)
            .and_then(|v| v.parse().ok())
            .ok_or("--rep needs a number")?,
        None => 1,
    };
    let refs = many("--job")
        .iter()
        .map(|s| JobRef::parse(s))
        .collect::<Result<Vec<_>, _>>()?;
    let problem = one("--problem")?;
    let expect = Expect {
        scheduler: "pbs".into(),
        cluster,
        conditions,
    };
    // 単発の手動収集は trial_no を引数で受け、`collect-observed-out`（台帳から全 trial）と同じ identity を書く。
    let identity = trial_identity(
        &one("--campaign-sha256")?,
        config,
        &problem,
        rep,
        one("--trial-no")?
            .parse()
            .map_err(|_| "--trial-no needs a number".to_string())?,
    );
    let path = collect_for_trial(
        t,
        Path::new(&one("--out")?),
        TrialKey {
            problem: &problem,
            config,
            rep,
        },
        &identity,
        &refs,
        &expect,
        Path::new(&one("--artifact")?),
        &many("--raw"),
    )
    .map_err(|e| e.join("\n"))?;
    Ok(path.display().to_string())
}
