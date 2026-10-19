//! 評価側の PBS 終了記録収集。`celeris.observed-jobs/1` を、trial の自己申告ではなく
//! 投入先（Celeris が台帳に残した `scheduler:cluster:job_id`）・実 scheduler の応答・case の条件・raw の hash から作る。
//! 生成物は `grade-out` / `grade --observed` が読む `<out>/observed/<problem>/<Config>-rep<N>.json`。
//! scheduler への問い合わせは read-only の `SchedulerTransport` の背後に置く（新 daemon・scheduler は作らない）。
use std::{collections::BTreeMap, fs, path::Path};

use serde_json::{Map, Value, json};

use crate::{
    grader::{evidence::OBSERVED_SCHEMA, safe_join, sha256_file},
    manifest::ConfigId,
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
pub fn collect_for_trial(
    t: &dyn SchedulerTransport,
    out: &Path,
    key: TrialKey<'_>,
    refs: &[JobRef],
    expect: &Expect,
    artifact: &Path,
    raw_paths: &[String],
) -> Result<std::path::PathBuf, Vec<String>> {
    let dir = out.join("observed").join(key.problem);
    let stem = format!("{:?}-rep{}", key.config, key.rep);
    let raw_dir = dir.join("scheduler").join(&stem);
    let doc = collect(t, refs, expect, artifact, raw_paths, &raw_dir)?;
    let path = dir.join(format!("{stem}.json"));
    fs::write(&path, doc.to_string()).map_err(|e| vec![e.to_string()])?;
    Ok(path)
}

/// `celeris-campaign collect-observed --case <case.json> --grade-case <grade-case.json> --artifact <dir> --out <dir>
/// --problem <id> --config a|b|c [--rep N] --job <scheduler:cluster:job_id>... --raw <artifact 内 path>...`。
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
    let path = collect_for_trial(
        t,
        Path::new(&one("--out")?),
        TrialKey {
            problem: &problem,
            config,
            rep,
        },
        &refs,
        &expect,
        Path::new(&one("--artifact")?),
        &many("--raw"),
    )
    .map_err(|e| e.join("\n"))?;
    Ok(path.display().to_string())
}
