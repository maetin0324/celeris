//! B/C の能力 probe の環境は、測定専用 daemon の process から取る（CLI の環境で代用しない）。
//! 実 process 境界: 別 process（daemon の代役。`env_clear` で環境を絞った実 process）の `/proc/<pid>/environ` から
//! worker 環境を組み、実起動関数（`FakeAdapter` → `run_subprocess`）で検査する。WorkerDaemonEnv は手で注入しない。
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use celeris_campaign::{
    cap_probe::{self, CapEnv, ProbeRunner},
    cases::CaseSet,
    manifest::ConfigId,
    preflight::{Host, ResolvedProgram, check},
};
use serde_json::{Value, json};

mod daemon_identity;
mod preflight_setup;
use preflight_setup::*;

/// 測定 daemon の代役の実 process。環境は `env` だけ（CLI = この試験 process の環境は継がない）。
struct FakeDaemon(Child);
impl FakeDaemon {
    fn start(env: &[(&str, &str)]) -> Self {
        let mut c = Command::new("/bin/sleep");
        c.arg("300")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        let d = Self(c.spawn().expect("spawn daemon stand-in"));
        // exec 完了直後は /proc/<pid>/environ がまだ空のことがある（長く動く daemon では起きない）。出来事を待つ。
        let p = format!("/proc/{}/environ", d.pid());
        let first = env.first().map(|(k, _)| format!("{k}="));
        for _ in 0..2000 {
            let b = std::fs::read(&p).unwrap_or_default();
            if first
                .as_ref()
                .is_none_or(|f| String::from_utf8_lossy(&b).contains(f.as_str()))
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        d
    }
    fn pid(&self) -> u32 {
        self.0.id()
    }
}
impl Drop for FakeDaemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn k4() -> celeris_campaign::cases::Case {
    CaseSet::load(
        &root().join("cases"),
        &root().join("fixtures/grader-private"),
    )
    .expect("cases")
    .get("k4-cmp4-validity-analysis")
    .expect("k4")
    .case
    .clone()
}

fn tools(names: &[&str]) -> PathBuf {
    let d = tempfile::tempdir().expect("dir").keep();
    for n in names {
        let mut c = Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"cat > "$1" && chmod 755 "$1""#)
            .arg("sh")
            .arg(d.join(n))
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn");
        std::io::Write::write_all(
            &mut c.stdin.take().expect("stdin"),
            b"#!/bin/sh\necho \"Python 3 mark=$CAP_MARK\"\n",
        )
        .expect("write");
        assert!(c.wait().expect("wait").success());
    }
    d
}

fn cap_env(v: Value) -> CapEnv {
    serde_json::from_value(json!({"schema":"celeris.campaign-cap-env/1","a":v.clone(),"worker":v}))
        .expect("cap env")
}

fn failing(rep: &cap_probe::ProbeReport) -> Vec<String> {
    rep.results
        .iter()
        .filter(|r| !r.ok)
        .map(|r| r.id.clone())
        .collect()
}

fn daemon_env(d: &FakeDaemon, env: &CapEnv) -> cap_probe::WorkerDaemonEnv {
    cap_probe::WorkerDaemonEnv::from_daemon_pid(d.pid(), &env.worker).expect("daemon env")
}

/// daemon にだけある変数・PATH で通る（CLI の環境には CAP_MARK も道具の dir も無い）。取るのは名指した変数だけ。
#[test]
fn daemon_only_environment_passes_and_only_named_variables_are_taken() {
    assert!(
        std::env::var_os("CAP_MARK").is_none(),
        "CLI env must differ"
    );
    let t = tools(&["python3"]);
    let env = cap_env(json!({"env": [["CAP_MARK", "m1"], ["PATH", t.display().to_string()]]}));
    let d = FakeDaemon::start(&[
        ("PATH", &format!("{}:/usr/bin:/bin", t.display())),
        ("CAP_MARK", "m1"),
        ("HOME", "/home/daemon-home"),
        ("API_TOKEN", "super-secret-value"),
    ]);
    let w = daemon_env(&d, &env);
    // 秘密・無関係な変数は持ち込まない。
    let all = format!("{w:?}");
    assert!(
        !all.contains("super-secret-value") && !all.contains("API_TOKEN"),
        "{all}"
    );
    let ws = tempfile::tempdir().expect("ws");
    for arm in [ConfigId::B, ConfigId::C] {
        let rep = cap_probe::probe_worker(&env, &w, arm, ws.path(), &k4());
        assert!(rep.ok, "{:?}", rep.results);
        assert_eq!(rep.paid_calls_made, 0);
        let id = rep.daemon.as_ref().expect("target identity recorded");
        assert_eq!(id.pid, d.pid());
        assert!(id.starttime > 0);
        let out =
            std::fs::read_to_string(ws.path().join("artifacts/cap-probe/0.out")).expect("out");
        assert!(out.contains("mark=m1"), "{out}");
        // 報告に環境変数全体は出ない。
        let json = serde_json::to_string(&rep).expect("json");
        assert!(
            !json.contains("super-secret-value") && !json.contains("daemon-home"),
            "{json}"
        );
    }
}

/// CLI は道具の dir も変数も持つが、daemon 側に無ければ env_failure（CLI の環境で代用しない）。
#[test]
fn daemon_missing_toolchain_or_variable_is_env_failure_even_if_cli_has_it() {
    let t = tools(&["python3"]);
    let env = cap_env(json!({"env": [["CAP_MARK", "m1"], ["PATH", t.display().to_string()]]}));
    // daemon の PATH に道具の dir が無く、変数も無い。CLI(この process)の PATH は通常どおり /usr/bin 等を持つ。
    let d = FakeDaemon::start(&[("PATH", "/usr/bin:/bin")]);
    assert!(std::env::var_os("PATH").is_some());
    let w = daemon_env(&d, &env);
    let ws = tempfile::tempdir().expect("ws");
    for arm in [ConfigId::B, ConfigId::C] {
        let rep = cap_probe::probe_worker(&env, &w, arm, ws.path(), &k4());
        assert!(!rep.ok);
        assert_eq!(rep.failure_class, Some("env_failure"));
        assert_eq!(rep.arm, arm);
        assert_eq!(rep.daemon.as_ref().expect("identity").pid, d.pid());
        let f = failing(&rep);
        assert!(
            f.iter().all(|x| x == "worker-env" || x == "python3"),
            "{f:?}"
        );
        assert!(f.contains(&"worker-env".to_string()), "{f:?}");
    }
}

/// daemon 側の承認済み socket / ro mount の欠落。
#[test]
fn daemon_side_missing_socket_or_mount_is_env_failure() {
    let t = tools(&["python3"]);
    let gone = tempfile::tempdir().expect("g").keep().join("absent");
    let d = FakeDaemon::start(&[("PATH", &format!("{}:/usr/bin:/bin", t.display()))]);
    let ws = tempfile::tempdir().expect("ws");
    for (key, id) in [("sockets", "socket"), ("ro_mounts", "toolchain")] {
        let env = cap_env(json!({ key: [gone] }));
        let rep =
            cap_probe::probe_worker(&env, &daemon_env(&d, &env), ConfigId::C, ws.path(), &k4());
        assert_eq!(rep.failure_class, Some("env_failure"));
        assert_eq!(failing(&rep), vec![format!("{id}:{}", gone.display())]);
    }
}

/// 子 process は daemon の環境だけ（CLI の環境が残らない）: CLI にだけある変数（cargo test が足す
/// `CARGO_MANIFEST_DIR`）は、道具の stub から見えない。
#[test]
fn probe_child_does_not_inherit_the_cli_environment() {
    assert!(
        std::env::var_os("CARGO_MANIFEST_DIR").is_some(),
        "CLI-only variable"
    );
    let t = tools(&[]);
    let mut c = Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(t.join("python3"))
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn");
    std::io::Write::write_all(
        &mut c.stdin.take().expect("stdin"),
        b"#!/bin/sh\necho \"Python 3 leak=[$CARGO_MANIFEST_DIR]\"\n",
    )
    .expect("write");
    assert!(c.wait().expect("wait").success());
    let env = cap_env(json!({}));
    let d = FakeDaemon::start(&[("PATH", &format!("{}:/usr/bin:/bin", t.display()))]);
    let ws = tempfile::tempdir().expect("ws");
    let rep = cap_probe::probe_worker(&env, &daemon_env(&d, &env), ConfigId::B, ws.path(), &k4());
    assert!(rep.ok, "{:?}", rep.results);
    let out = std::fs::read_to_string(ws.path().join("artifacts/cap-probe/0.out")).expect("out");
    assert!(out.contains("leak=[]"), "{out}");
}

// ---- preflight 全体（identity 照合 → 照合済み daemon の pid から環境 → 実起動関数）----

struct Real {
    /// identity が答える pid。
    answers_pid: u32,
}
impl Host for Real {
    fn env_nonempty(&self, _: &str) -> bool {
        true
    }
    fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
        Ok(ResolvedProgram {
            path: p.into(),
            version: "x".into(),
        })
    }
    fn api_get(&self, _: &str, _: &str, path: &str) -> Result<Value, String> {
        match path {
            "/api/v1/config" => {
                let mut b = daemon_identity::matching_daemon_body();
                b["identity"]["pid"] = json!(self.answers_pid);
                Ok(b)
            }
            _ => Err(format!("GET {path}: not in this test")),
        }
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
    fn probe_runner(&self) -> Box<dyn ProbeRunner> {
        Box::new(OkRunner)
    }
    // worker_daemon_env は既定実装（identity 照合済み pid の process から取る）を使う。
}

fn capability_misses(
    inp: &celeris_campaign::preflight::Inputs,
    h: &Real,
) -> (Vec<String>, Vec<Value>) {
    let r = check(inp, h);
    let v = serde_json::to_value(&r).expect("report");
    (
        r.missing
            .iter()
            .filter(|m| m.check == "capability")
            .map(|m| m.detail.clone())
            .collect(),
        v["probe_targets"].as_array().cloned().unwrap_or_default(),
    )
}

#[test]
fn preflight_probes_the_verified_daemon_process_and_records_its_identity() {
    let good_path = fake_worker_daemon().path.expect("path");
    // 通る: daemon の PATH は fake program の dir。
    let ok = FakeDaemon::start(&[("PATH", &good_path)]);
    let mut s = setup();
    s.inp.measure.as_mut().expect("measure").daemon_pid = Some(ok.pid());
    let (miss, targets) = capability_misses(
        &s.inp,
        &Real {
            answers_pid: ok.pid(),
        },
    );
    assert!(miss.is_empty(), "{miss:?}");
    assert!(!targets.is_empty());
    for t in &targets {
        assert_eq!(t["daemon"]["pid"], ok.pid());
        assert_eq!(t["daemon"]["instance_id"], "01MEASUREDAEMON00000000000");
        assert!(t["arm"] == "B" || t["arm"] == "C", "{t}");
    }
    // 落ちる: CLI は正常でも daemon の PATH に道具が無い → B と C が env_failure。LLM 呼出し前（preflight）。
    let bad = FakeDaemon::start(&[("PATH", "/nonexistent-daemon-path")]);
    let mut s = setup();
    s.inp.measure.as_mut().expect("measure").daemon_pid = Some(bad.pid());
    let r = check(
        &s.inp,
        &Real {
            answers_pid: bad.pid(),
        },
    );
    assert_eq!(r.failure_class, Some("env_failure"));
    let miss: Vec<_> = r
        .missing
        .iter()
        .filter(|m| m.check == "capability")
        .collect();
    assert!(miss.iter().any(|m| m.detail.contains("arm B")), "{miss:?}");
    assert!(miss.iter().any(|m| m.detail.contains("arm C")), "{miss:?}");
}

/// 検査先が測定 daemon でない（identity が別 pid）なら、環境を読まず B/C は検査しない。
#[test]
fn probe_target_must_pass_the_identity_check() {
    let d = FakeDaemon::start(&[("PATH", &fake_worker_daemon().path.expect("path"))]);
    let mut s = setup();
    s.inp.measure.as_mut().expect("measure").daemon_pid = Some(d.pid());
    let (miss, targets) = capability_misses(
        &s.inp,
        &Real {
            answers_pid: d.pid() + 1,
        },
    );
    assert!(targets.is_empty());
    assert!(
        miss.iter()
            .any(|m| m.contains("not the measurement daemon")),
        "{miss:?}"
    );
    // 測定 daemon の指定が無い（CLI の環境で代用しない）。
    let mut s = setup();
    s.inp.measure = None;
    let (miss, _) = capability_misses(&s.inp, &Real { answers_pid: 1 });
    assert!(
        miss.iter().any(|m| m.contains("measurement daemon")),
        "{miss:?}"
    );
}
