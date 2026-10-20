//! 外部入力（case.json の `inputs[]`）が hash 付きで全腕の workspace の `inputs/` に届くこと、
//! 取り込み元の欠落・改変で run が始まらないこと（ADR 2026-10-17-mlsys-pilot2-fixes D1）。
//! mock 実行器と stub 実行ファイルだけを使う（network・LLM なし）。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread::sleep,
    time::Duration,
};

use celeris_campaign::{
    dispatch::ArmDispatch,
    executor::MockExecutor,
    harness_exec::{CommandBuilder, HarnessCommand, HarnessExecutor},
    manifest::CampaignManifest,
    record::DeliveredInput,
    runner::{RunOptions, run},
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const NAME: &str = "raw-input.tgz";
const BODY: &[u8] = b"pretend this is main-43223.tgz";

fn fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

struct Env {
    d: tempfile::TempDir,
    m: CampaignManifest,
    problem: String,
    inputs_src: PathBuf,
}

fn env() -> Env {
    let d = tempfile::tempdir().expect("dir");
    let mp = fx().join("pilot.json");
    let m = CampaignManifest::load(&mp).expect("pilot");
    let problem = m.problems[0].clone();
    let inputs_src = d.path().join("inputs-src");
    fs::create_dir_all(&inputs_src).expect("mkdir");
    fs::write(inputs_src.join(NAME), BODY).expect("write");
    Env {
        d,
        m,
        problem,
        inputs_src,
    }
}

impl Env {
    fn opts(&self, src: Option<PathBuf>) -> RunOptions {
        RunOptions {
            public_cases: Default::default(),
            global_ledger: None,
            out: self.d.path().join("out"),
            repo_src: Some(fx().join("repo")),
            materials_src: Some(fx().join("materials")),
            kb_src: Some(fx().join("kb")),
            inputs_src: src,
            case_inputs: BTreeMap::from([(
                self.problem.clone(),
                vec![DeliveredInput {
                    name: NAME.into(),
                    sha256: sha(BODY),
                }],
            )]),
            lock: None,
        }
    }
    fn records(&self) -> Vec<Value> {
        fs::read_to_string(self.d.path().join("out/trials.jsonl"))
            .expect("trials")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect()
    }
}

#[test]
fn every_arm_of_the_declaring_problem_gets_the_input_and_others_do_not() {
    let e = env();
    let i =
        e.m.load_input(&fx().join("pilot.json"), None)
            .expect("input");
    run(
        &e.m,
        &i,
        &MockExecutor::seeded(),
        &e.opts(Some(e.inputs_src.clone())),
    )
    .expect("run");
    let recs = e.records();
    let mut arms = std::collections::BTreeSet::new();
    for r in &recs {
        let ws =
            e.d.path()
                .join("out")
                .join(r["workspace"].as_str().expect("ws"));
        if r["problem_id"] == e.problem.as_str() {
            arms.insert(r["config"].to_string());
            let got = fs::read(ws.join("inputs").join(NAME)).expect("delivered");
            assert_eq!(sha(&got), sha(BODY));
            assert_eq!(r["inputs"][0]["sha256"], sha(BODY).as_str());
            assert_eq!(r["inputs"][0]["name"], NAME);
        } else {
            assert!(!ws.join("inputs").exists(), "undeclared problem got inputs");
            assert!(r.get("inputs").is_none());
        }
    }
    assert_eq!(arms.len(), 3, "A, B and C all received it: {arms:?}");
    // 初期 hash は入力を除いた木で数え、全腕で一致する。
    let init: std::collections::BTreeSet<_> = recs
        .iter()
        .map(|r| r["workspace_sha256_initial"].to_string())
        .collect();
    assert_eq!(init.len(), 1, "{init:?}");
    // snapshot は取り込み時の hash を残す。
    let snap: Value = serde_json::from_slice(
        &fs::read(e.d.path().join("out/snapshot/snapshot.json")).expect("snap"),
    )
    .expect("json");
    assert_eq!(snap["inputs_sha256"][NAME], sha(BODY).as_str());
}

#[test]
fn run_refuses_to_start_without_or_with_a_wrong_input() {
    let e = env();
    let i =
        e.m.load_input(&fx().join("pilot.json"), None)
            .expect("input");
    let err = run(&e.m, &i, &MockExecutor::seeded(), &e.opts(None)).expect_err("no src");
    assert!(err.contains("--inputs-src"), "{err}");
    assert!(!e.d.path().join("out/trials.jsonl").exists());
    fs::write(e.inputs_src.join(NAME), b"tampered").expect("tamper");
    let err = run(
        &e.m,
        &i,
        &MockExecutor::seeded(),
        &e.opts(Some(e.inputs_src.clone())),
    )
    .expect_err("wrong hash");
    assert!(err.contains("sha256"), "{err}");
    fs::remove_file(e.inputs_src.join(NAME)).expect("rm");
    let err = run(
        &e.m,
        &i,
        &MockExecutor::seeded(),
        &e.opts(Some(e.inputs_src.clone())),
    )
    .expect_err("missing");
    assert!(err.contains(NAME), "{err}");
}

/// A（非同期の harness 経路）の workspace にも入力が届く。入力の展開だけでは成果物に数えない。
struct Stub(PathBuf);
impl CommandBuilder for Stub {
    fn build(&self, _: &celeris_campaign::executor::TrialRequest) -> HarnessCommand {
        HarnessCommand {
            program: self.0.clone(),
            args: vec![],
            env: vec![],
        }
    }
}

#[test]
fn async_a_arm_gets_inputs_and_unpacking_them_is_not_an_artifact() {
    use std::io::Write;
    let e = env();
    let stub = e.d.path().join("stub.sh");
    // 入力を読み、inputs/ の下に展開物を書くだけで、成果物は作らない。
    let script = format!(
        "#!/bin/sh\ntest -f inputs/{NAME} || exit 3\nmkdir -p inputs/unpacked && echo x > inputs/unpacked/f\nexit 0\n"
    );
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(&stub)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(script.as_bytes())
        .expect("write");
    assert!(child.wait().expect("wait").success());
    let i =
        e.m.load_input(&fx().join("pilot.json"), None)
            .expect("input");
    let exec = ArmDispatch {
        a: Some(HarnessExecutor::new(
            e.d.path().join("hs"),
            Box::new(Stub(stub)),
        )),
        bc: MockExecutor::seeded(),
    };
    let o = e.opts(Some(e.inputs_src.clone()));
    for _ in 0..200 {
        let r = run(&e.m, &i, &exec, &o).expect("run");
        if r.waiting == 0 {
            break;
        }
        sleep(Duration::from_millis(50));
    }
    let a: Vec<Value> = e
        .records()
        .into_iter()
        .filter(|r| r["config"] == "A" && r["problem_id"] == e.problem.as_str())
        .collect();
    assert_eq!(a.len(), 1, "A trial recorded");
    // stub は inputs/<name> が無ければ exit 3 する。届いていて、成果物が無いので no_artifact。
    assert_eq!(a[0]["outcome"], "failed", "{}", a[0]);
    assert_eq!(a[0]["reason"], "no_artifact");
    let ws =
        e.d.path()
            .join("out")
            .join(a[0]["workspace"].as_str().expect("ws"));
    assert!(ws.join("inputs/unpacked/f").is_file());
}
