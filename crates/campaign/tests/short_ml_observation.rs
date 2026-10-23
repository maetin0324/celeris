//! short-ml-campaign の評価側 PBS 終了記録収集（`observe::collect_for_trial`）。scheduler の固定応答を注入し、
//! 生成した `celeris.observed-jobs/1` を通常 grader の実験証拠検査（`grader::evidence::check`）に通す。
//! 固定応答は合成（origin=synthetic）であり本番証拠ではない。実 job は投入せず、network も使わない。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use celeris_campaign::{
    grader::{GradeCase, Status, evidence},
    manifest::ConfigId,
    observe::{
        CollectClass, CollectIssue, Expect, JobRef, Origin, SchedulerReply, SchedulerTransport,
        TrialKey, collect_for_trial,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const JOB: &str = "4242.sirius";
const RAW: [&str; 3] = [
    "results/coefficients.csv",
    "results/metrics.csv",
    "results/predictions.csv",
];

struct Fixed {
    cluster: String,
    raw: String,
}

impl SchedulerTransport for Fixed {
    fn origin(&self) -> Origin {
        Origin::Synthetic
    }
    fn qstat(&self, _s: &str, _c: &str, _id: &str) -> Result<SchedulerReply, String> {
        Ok(SchedulerReply {
            cluster: self.cluster.clone(),
            raw: self.raw.clone(),
        })
    }
}

fn qstat(id: &str, state: &str, exit: Option<i32>, queue: &str, ncpus: u32) -> String {
    let e = exit
        .map(|e| format!("    Exit_status = {e}\n"))
        .unwrap_or_default();
    format!(
        "Job Id: {id}\n    Job_Name = sml\n    job_state = {state}\n    queue = {queue}\n    Resource_List.ncpus = {ncpus}\n{e}"
    )
}

fn grade_case() -> GradeCase {
    GradeCase::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/grade-cases/dev/short-ml-campaign/short-ml-campaign.json"),
    )
    .unwrap()
}

fn expect(gc: &GradeCase) -> Expect {
    Expect {
        scheduler: "pbs".into(),
        cluster: "sirius".into(),
        conditions: gc.experiment.as_ref().unwrap().conditions.clone(),
        trial_key: None,
        require_marker: false,
    }
}

struct Fx {
    _t: tempfile::TempDir,
    out: PathBuf,
    art: PathBuf,
}

fn artifact(job: &str) -> Fx {
    let t = tempfile::tempdir().unwrap();
    let (out, art) = (t.path().join("out"), t.path().join("art"));
    let mut raw = vec![];
    for p in RAW {
        let body = format!("raw {p}\n");
        fs::create_dir_all(art.join(p).parent().unwrap()).unwrap();
        fs::write(art.join(p), &body).unwrap();
        raw.push(json!({"path": p, "sha256": format!("{:x}", Sha256::digest(body.as_bytes()))}));
    }
    fs::create_dir_all(art.join("evidence")).unwrap();
    fs::write(
        art.join(format!("evidence/{job}.qstat")),
        format!("Job Id: {job}\n Exit_status = 0\n"),
    )
    .unwrap();
    fs::write(art.join(format!("evidence/{job}.log")), "done\n").unwrap();
    let doc = json!({"jobs": [{
        "job_id": job, "evidence_kind": "real",
        "scheduler_record": format!("evidence/{job}.qstat"), "log": format!("evidence/{job}.log"),
        "raw": raw, "conditions": {"queue": "debug", "ncpus": 24}}]});
    fs::write(art.join("evidence/jobs.json"), doc.to_string()).unwrap();
    Fx { _t: t, out, art }
}

fn raws() -> Vec<String> {
    RAW.iter().map(|s| s.to_string()).collect()
}

fn refs(id: &str) -> Vec<JobRef> {
    vec![JobRef::parse(&format!("pbs:sirius:{id}")).unwrap()]
}

fn run(
    fx: &Fx,
    t: &dyn SchedulerTransport,
    r: &[JobRef],
    e: &Expect,
) -> Result<PathBuf, Vec<String>> {
    run_classified(fx, t, r, e).map_err(|v| v.iter().map(ToString::to_string).collect())
}

fn run_classified(
    fx: &Fx,
    t: &dyn SchedulerTransport,
    r: &[JobRef],
    e: &Expect,
) -> Result<PathBuf, Vec<CollectIssue>> {
    collect_for_trial(
        t,
        &fx.out,
        TrialKey {
            problem: "short-ml-campaign",
            config: ConfigId::B,
            rep: 1,
        },
        &json!({"campaign_sha256": "t", "config": "B", "problem_id": "short-ml-campaign", "rep": 1, "trial_no": 1}),
        r,
        e,
        &fx.art,
        &raws(),
    )
}

fn grade(fx: &Fx, gc: &GradeCase, observed: Option<&Path>) -> (Status, String) {
    let r = evidence::check(gc.experiment.as_ref().unwrap(), &fx.art, false, observed);
    (r.status, r.reasons.join(" | "))
}

fn ok_reply() -> Fixed {
    Fixed {
        cluster: "sirius".into(),
        raw: qstat(JOB, "F", Some(0), "debug", 24),
    }
}

#[test]
fn success_is_collected_by_the_evaluator_and_passes_the_normal_grader() {
    let (gc, fx) = (grade_case(), artifact(JOB));
    let p = run(&fx, &ok_reply(), &refs(JOB), &expect(&gc)).unwrap();
    assert_eq!(p, fx.out.join("observed/short-ml-campaign/B-rep1.json"));
    let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v["schema"], "celeris.observed-jobs/1");
    assert_eq!(
        v["origin"], "synthetic",
        "fixed replies are not production evidence"
    );
    assert_eq!(v["jobs"][0]["exit_status"], 0);
    assert_eq!(
        v["jobs"][0]["conditions"],
        json!({"queue": "debug", "ncpus": 24})
    );
    assert_eq!(grade(&fx, &gc, Some(&p)), (Status::Pass, String::new()));
}

#[test]
fn nonzero_exit_is_recorded_and_rejected_by_the_grader() {
    let (gc, fx) = (grade_case(), artifact(JOB));
    let t = Fixed {
        cluster: "sirius".into(),
        raw: qstat(JOB, "F", Some(1), "debug", 24),
    };
    let p = run(&fx, &t, &refs(JOB), &expect(&gc)).unwrap();
    let (s, why) = grade(&fx, &gc, Some(&p));
    assert_eq!(s, Status::Fail);
    assert!(why.contains("exit status"), "{why}");
}

#[test]
fn wrong_conditions_are_rejected_by_the_grader() {
    let (gc, fx) = (grade_case(), artifact(JOB));
    let t = Fixed {
        cluster: "sirius".into(),
        raw: qstat(JOB, "F", Some(0), "workq", 4),
    };
    let p = run(&fx, &t, &refs(JOB), &expect(&gc)).unwrap();
    let (s, why) = grade(&fx, &gc, Some(&p));
    assert_eq!(s, Status::Fail);
    assert!(
        why.contains("condition queue") && why.contains("condition ncpus"),
        "{why}"
    );
}

#[test]
fn raw_tamper_after_collection_is_rejected() {
    let (gc, fx) = (grade_case(), artifact(JOB));
    let p = run(&fx, &ok_reply(), &refs(JOB), &expect(&gc)).unwrap();
    fs::write(fx.art.join(RAW[0]), "tampered\n").unwrap();
    let (s, why) = grade(&fx, &gc, Some(&p));
    assert_eq!(s, Status::Fail);
    assert!(why.contains("sha256"), "{why}");
}

#[test]
fn self_reported_job_id_alone_is_not_accepted() {
    // 評価側は別の job（4243）を観測した。成果物が自己申告した 4242 は記録に無い。
    let (gc, fx) = (grade_case(), artifact(JOB));
    let other = "4243.sirius";
    let t = Fixed {
        cluster: "sirius".into(),
        raw: qstat(other, "F", Some(0), "debug", 24),
    };
    let p = run(&fx, &t, &refs(other), &expect(&gc)).unwrap();
    let (s, why) = grade(&fx, &gc, Some(&p));
    assert_eq!(s, Status::Fail);
    assert!(why.contains("not recorded by the evaluator"), "{why}");
    // 記録が無い（観測ファイル無し）なら自己申告のみとして拒否。
    let (s, why) = grade(&fx, &gc, None);
    assert_eq!(s, Status::Fail);
    assert!(why.contains("self-reported"), "{why}");
}

#[test]
fn collection_refuses_unexecuted_unfinished_mismatched_and_missing() {
    let (gc, fx) = (grade_case(), artifact(JOB));
    let e = expect(&gc);
    let dest = fx.out.join("observed/short-ml-campaign/B-rep1.json");
    let refuse = |t: &Fixed, r: &[JobRef], e: &Expect, needle: &str| {
        let errs = run(&fx, t, r, e).unwrap_err().join(" | ");
        assert!(errs.contains(needle), "{needle}: {errs}");
        assert!(!dest.exists(), "a refused collection leaves no record");
    };
    refuse(&ok_reply(), &[], &e, "not executed");
    refuse(
        &Fixed {
            cluster: "sirius".into(),
            raw: qstat(JOB, "R", None, "debug", 24),
        },
        &refs(JOB),
        &e,
        "not finished",
    );
    refuse(
        &Fixed {
            cluster: "sirius".into(),
            raw: qstat(JOB, "F", None, "debug", 24),
        },
        &refs(JOB),
        &e,
        "no Exit_status",
    );
    refuse(
        &Fixed {
            cluster: "sirius".into(),
            raw: qstat("9.sirius", "F", Some(0), "debug", 24),
        },
        &refs(JOB),
        &e,
        "not this job id",
    );
    // 取得元が投入先と違う。
    refuse(
        &Fixed {
            cluster: "pegasus".into(),
            raw: qstat(JOB, "F", Some(0), "debug", 24),
        },
        &refs(JOB),
        &e,
        "answered by pegasus",
    );
    // 投入先が case の固定と違う。
    let wrong = Expect {
        cluster: "pegasus".into(),
        ..e.clone()
    };
    refuse(&ok_reply(), &refs(JOB), &wrong, "case fixes");
    // PID は scheduler id ではない。
    refuse(&ok_reply(), &refs("4242"), &e, "PIDs are not accepted");
    // raw 欠落。
    fs::remove_file(fx.art.join(RAW[2])).unwrap();
    refuse(&ok_reply(), &refs(JOB), &e, "is missing");
    let _: BTreeMap<String, Value> = e.conditions;
}

#[test]
fn ssh_qstat_refuses_non_pbs_pid_like_ids_and_option_like_clusters_without_running_ssh() {
    use celeris_campaign::{observe::SchedulerTransport, ssh_qstat::SshQstat};
    let t = SshQstat;
    assert!(
        t.qstat("slurm", "sirius", "1.pbs")
            .unwrap_err()
            .contains("pbs only")
    );
    assert!(
        t.qstat("pbs", "sirius", "12345")
            .unwrap_err()
            .contains("refusing")
    );
    assert!(
        t.qstat("pbs", "-oProxyCommand=x", "1.pbs")
            .unwrap_err()
            .contains("refusing")
    );
    assert!(
        t.qstat("pbs", "sirius", "1.pbs;rm")
            .unwrap_err()
            .contains("refusing")
    );
}

/// 照会が届かない（ssh・qstat の不通）か、scheduler が job を知らないと答える transport。
struct Down(&'static str);

impl SchedulerTransport for Down {
    fn origin(&self) -> Origin {
        Origin::Synthetic
    }
    fn qstat(&self, _s: &str, _c: &str, _id: &str) -> Result<SchedulerReply, String> {
        Err(self.0.into())
    }
}

fn with_marker(raw: String, key: &str) -> String {
    format!(
        "{raw}    Variable_List = PBS_O_HOME=/home/u,CELERIS_TRIAL_KEY={key},\n\tPBS_O_SHELL=/bin/sh\n"
    )
}

/// 収集の結果は分類付き: 未投入・未完了・取得失敗・未観測（未知 ID）・identity 不一致を分け、失敗は分類と原因
/// （failure_class）を持つ記録に残す。取得失敗は env_failure（model_failure にしない）で、直れば収集し直せる。
/// job の非 0 終了（研究の失敗）は収集の失敗ではなく、記録を作って job_failed と印す。
#[test]
fn collection_outcomes_are_classified_and_fetch_failures_are_not_model_failures() {
    let (gc, fx) = (grade_case(), artifact(JOB));
    let key = "campaign-0123456789abcdef";
    let e = Expect {
        trial_key: Some(key.into()),
        require_marker: true,
        ..expect(&gc)
    };
    let failure = fx
        .out
        .join("observed/short-ml-campaign/B-rep1.collect-failure.json");
    let class = |t: &dyn SchedulerTransport, r: &[JobRef]| -> CollectClass {
        let issues = run_classified(&fx, t, r, &e).unwrap_err();
        let doc: Value = serde_json::from_slice(&fs::read(&failure).unwrap()).unwrap();
        assert_eq!(doc["schema"], "celeris.observe-failure/1");
        assert_eq!(
            doc["issues"][0]["class"],
            serde_json::to_value(issues[0].class).unwrap()
        );
        issues[0].class
    };
    let marked = |state: &str, exit: Option<i32>, k: &str| Fixed {
        cluster: "sirius".into(),
        raw: with_marker(qstat(JOB, state, exit, "debug", 24), k),
    };
    assert_eq!(class(&ok_reply(), &[]), CollectClass::NotSubmitted);
    assert_eq!(
        class(&marked("R", None, key), &refs(JOB)),
        CollectClass::NotFinished
    );
    // 取得失敗（ssh 不通）は測定基盤の失敗: env_failure。
    assert_eq!(
        class(&Down("ssh sirius: Connection timed out"), &refs(JOB)),
        CollectClass::FetchFailed
    );
    let doc: Value = serde_json::from_slice(&fs::read(&failure).unwrap()).unwrap();
    assert_eq!(doc["failure_class"], "env_failure");
    // scheduler が知らない ID（未知 ID）は未観測で、取得失敗とは分ける。
    assert_eq!(
        class(&Down("qstat: Unknown Job Id 4242.sirius"), &refs(JOB)),
        CollectClass::Unobserved
    );
    // 別 trial の印を持つ job・印の無い job（wrapper を通らない投入）は identity 不一致。
    assert_eq!(
        class(
            &marked("F", Some(0), "campaign-ffffffffffffffff"),
            &refs(JOB)
        ),
        CollectClass::IdentityMismatch
    );
    assert_eq!(
        class(&ok_reply(), &refs(JOB)),
        CollectClass::IdentityMismatch
    );
    let doc: Value = serde_json::from_slice(&fs::read(&failure).unwrap()).unwrap();
    assert_eq!(doc["failure_class"], "undetermined");
    // 直った後の再収集: 記録ができ、失敗記録は消え、試みの履歴は collect-log に残る。
    let p = run_classified(&fx, &marked("F", Some(3), key), &refs(JOB), &e).unwrap();
    assert!(!failure.exists());
    let doc: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    assert_eq!(doc["jobs"][0]["job_status"], "job_failed");
    assert_eq!(doc["jobs"][0]["exit_status"], 3);
    let log =
        fs::read_to_string(fx.out.join("observed/short-ml-campaign/collect-log.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 7, "{log}");
    assert!(log.lines().last().unwrap().contains("\"ok\":true"), "{log}");
}
