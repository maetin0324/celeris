//! 必要能力 probe: case 固定・A と同じ sandbox argv・不足は env_failure で最初の LLM 呼出し前に停止・B/C 同等。
//! userns が要る実走は worker sandbox で出来ないので、argv 組立・判定・停止順序を stub runner で確かめる。
use std::{cell::RefCell, path::Path};

use celeris_campaign::{
    cap_probe::{self, CapEnv, ProbeRunner, Ran},
    cases::{CapKind, CaseSet},
    harness_exec::{HarnessCommand, Sandbox, Toolchain},
    manifest::ConfigId,
    preflight::{Host, ResolvedProgram, check, gate},
};
use serde_json::{Value, json};

mod preflight_setup;
use preflight_setup::*;

fn env_json(extra: Value) -> CapEnv {
    let v = json!({"schema":"celeris.campaign-cap-env/1","a":extra.clone(),"worker":extra});
    serde_json::from_value(v).expect("cap env")
}

fn cases() -> CaseSet {
    CaseSet::load(
        &root().join("cases"),
        &root().join("fixtures/grader-private"),
    )
    .expect("cases")
}

#[test]
fn k2_and_k4_fix_their_required_capabilities_as_case_data() {
    let set = cases();
    let k2 = &set.get("k2-bf-xattr-restart-campaign").expect("k2").case;
    let ids: Vec<_> = k2
        .required_capabilities
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    for want in [
        "cc",
        "c++",
        "cmake",
        "ucx-pkg",
        "min-build-ucx",
        "min-build-cmake",
        "rustc-min",
        "min-build-locusta",
        "autotools",
        "libclang",
        "sirius-link",
    ] {
        assert!(ids.contains(&want), "k2 lacks {want}: {ids:?}");
    }
    let link = k2
        .required_capabilities
        .iter()
        .find(|c| c.kind == CapKind::Connection)
        .expect("connection");
    assert!(
        link.argv.contains(&"-O".into()) && link.argv.contains(&"check".into()),
        "connection only, no job"
    );
    assert!(
        k2.required_capabilities
            .iter()
            .all(|c| !c.argv.join(" ").contains("sbatch"))
    );
    let k4 = &set.get("k4-cmp4-validity-analysis").expect("k4").case;
    assert!(
        k4.required_capabilities
            .iter()
            .any(|c| c.argv[0] == "python3")
    );
}

fn sb(env: &CapEnv, arm: ConfigId) -> Sandbox {
    cap_probe::sandbox_for(env, arm, "lab", Path::new("/acct/lab"))
}

#[derive(Default)]
struct Rec(RefCell<Vec<Vec<String>>>);
impl ProbeRunner for Rec {
    fn run(&self, program: &str, args: &[String]) -> Ran {
        assert_eq!(program, "bwrap");
        self.0.borrow_mut().push(args.to_vec());
        Ran::Exit {
            code: 0,
            stdout: "cmake version git version Python 3 rustc-min ok".into(),
        }
    }
}

/// probe の argv は A の起動と同じ `Sandbox::wrap_resolved` 出力（引数列が一致し、toolchain の固定 mount が入る）。
#[test]
fn probe_argv_is_exactly_the_a_sandbox_argv() {
    let env = env_json(
        json!({"ro_mounts":["/opt/ucx"],"sockets":["/run/celeris-cap/sirius-ssh.sock"],"env":[["PKG_CONFIG_PATH","/opt/ucx/lib/pkgconfig"]]}),
    );
    let a = sb(&env, ConfigId::A);
    let ws = Path::new("/out/ws");
    let set = cases();
    let case = &set.get("k4-cmp4-validity-analysis").expect("k4").case;
    let rec = Rec::default();
    let rep = cap_probe::probe(&a, ConfigId::A, ws, case, &rec);
    assert!(rep.ok, "{:?}", rep.results);
    assert_eq!(rep.paid_calls_made, 0);
    for (cap, got) in case.required_capabilities.iter().zip(rec.0.borrow().iter()) {
        let real = celeris_campaign::preflight::resolve_executable(
            Path::new(&cap.argv[0]),
            std::env::var_os("PATH").as_deref(),
        )
        .expect("resolve");
        let want = a
            .wrap_resolved(
                &HarnessCommand {
                    program: cap.argv[0].clone().into(),
                    args: cap.argv[1..].to_vec(),
                    env: vec![],
                },
                ws,
                Path::new("/acct/lab"),
                &real,
            )
            .expect("wrap");
        assert_eq!(&want.args, got, "{}", cap.id);
        let base = a.args(ws, Path::new("/acct/lab"), &real, &[]);
        assert_eq!(
            &got[..base.len()],
            &base[..],
            "sandbox prefix is Sandbox::args"
        );
    }
    let first = rec.0.borrow()[0].join(" ");
    assert!(
        first.contains("--ro-bind /opt/ucx /opt/ucx")
            && first.contains("--bind /run/celeris-cap/sirius-ssh.sock")
    );
    assert!(first.contains("--clearenv") && first.contains("--setenv HOME /tmp/home"));
}

#[test]
fn b_and_c_get_the_same_sandbox_argv_as_a() {
    let env = env_json(json!({"ro_mounts":["/opt/ucx"]}));
    let ws = Path::new("/out/ws");
    let set = cases();
    let case = &set.get("k4-cmp4-validity-analysis").expect("k4").case;
    let run = |arm| {
        let r = Rec::default();
        assert!(cap_probe::probe(&sb(&env, arm), arm, ws, case, &r).ok);
        r.0.into_inner()
    };
    assert_eq!(run(ConfigId::A), run(ConfigId::B));
    assert_eq!(run(ConfigId::A), run(ConfigId::C));
}

#[test]
fn worker_toolchain_must_equal_a_and_no_home_or_root_exposure() {
    let bad =
        json!({"schema":"celeris.campaign-cap-env/1","a":{"ro_mounts":["/opt/ucx"]},"worker":{}});
    let e: CapEnv = serde_json::from_value(bad).expect("parse");
    assert!(
        e.validate(None)
            .unwrap_err()
            .contains("worker toolchain must equal")
    );
    let t = |m: &str| Toolchain {
        ro_mounts: vec![m.into()],
        ..Default::default()
    };
    for m in ["/", "/home", "/root", "/home/op/../x", "rel/path"] {
        assert!(t(m).validate(None).is_err(), "{m}");
    }
    assert!(
        t("/home/op")
            .validate(Some(Path::new("/home/op/sub")))
            .is_err(),
        "covers HOME"
    );
    assert!(
        Toolchain {
            env: vec![("HOME".into(), "/x".into())],
            ..Default::default()
        }
        .validate(None)
        .is_err()
    );
    assert!(t("/opt/ucx").validate(Some(Path::new("/home/op"))).is_ok());
}

struct FailRunner;
impl ProbeRunner for FailRunner {
    fn run(&self, _: &str, _: &[String]) -> Ran {
        Ran::Exit {
            code: 127,
            stdout: String::new(),
        }
    }
}

#[test]
fn missing_capability_is_env_failure_and_counts_no_paid_call() {
    let set = cases();
    let case = &set.get("k4-cmp4-validity-analysis").expect("k4").case;
    let env = env_json(json!({}));
    let rep = cap_probe::probe(
        &sb(&env, ConfigId::A),
        ConfigId::A,
        Path::new("/w"),
        case,
        &FailRunner,
    );
    assert!(!rep.ok);
    assert_eq!(rep.failure_class, Some("env_failure"));
    assert_eq!(rep.paid_calls_made, 0);
    // 出力に期待文字列が無い場合も不足。
    struct Silent;
    impl ProbeRunner for Silent {
        fn run(&self, _: &str, _: &[String]) -> Ran {
            Ran::Exit {
                code: 0,
                stdout: String::new(),
            }
        }
    }
    let r = cap_probe::probe(
        &sb(&env, ConfigId::B),
        ConfigId::B,
        Path::new("/w"),
        case,
        &Silent,
    );
    assert!(
        r.results
            .iter()
            .any(|x| !x.ok && x.detail.contains("stdout lacks"))
    );
}

struct StubHost(bool);
impl Host for StubHost {
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
        Err(format!("GET {path}: no api in this test"))
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
    fn worker_daemon_env(&self, _: &Toolchain) -> cap_probe::WorkerDaemonEnv {
        if self.0 {
            fake_worker_daemon()
        } else {
            // 何も解決できない PATH（socket も道具も無い worker）。
            cap_probe::WorkerDaemonEnv {
                path: Some("/nonexistent-cap-probe".into()),
                env: vec![],
            }
        }
    }
    fn probe_runner(&self) -> Box<dyn ProbeRunner> {
        if self.0 {
            Box::new(OkRunner)
        } else {
            Box::new(FailRunner)
        }
    }
}

fn capability_missing(
    inp: &celeris_campaign::preflight::Inputs,
    ok: bool,
) -> (Vec<String>, Option<&'static str>) {
    let r = check(inp, &StubHost(ok));
    let m = r
        .missing
        .iter()
        .filter(|m| m.check == "capability")
        .map(|m| m.detail.clone())
        .collect();
    (m, r.failure_class)
}

/// preflight（= 最初の LLM 呼出しの前）で、能力不足は fail-closed。env_failure として記録され、gate が Err を返す。
#[test]
fn preflight_stops_before_any_llm_call_on_missing_capability() {
    let s = setup();
    let (miss, class) = capability_missing(&s.inp, true);
    assert!(miss.is_empty(), "{miss:?}");
    assert_eq!(class, None);
    let (miss, class) = capability_missing(&s.inp, false);
    assert!(
        miss.iter()
            .any(|m| m.contains("k2-bf-xattr-restart-campaign") && m.contains("sirius-link")),
        "{miss:?}"
    );
    assert!(miss.iter().any(|m| m.contains("arm B")) && miss.iter().any(|m| m.contains("arm C")));
    assert_eq!(class, Some("env_failure"));
    let err = gate(&s.inp, &StubHost(false)).unwrap_err();
    assert!(err.contains("no paid execution or task was started") && err.contains("env_failure"));
    // cap env が無いのも不足（fail-closed）。
    let mut inp = s.inp.clone();
    inp.cap_env = None;
    let (miss, class) = capability_missing(&inp, true);
    assert!(miss.iter().any(|m| m.contains("--cap-env is required")));
    assert_eq!(class, Some("env_failure"));
}

// ---- B/C: task-worker の実起動関数（FakeAdapter → run_subprocess）経由の能力 probe ----

fn k4() -> celeris_campaign::cases::Case {
    cases()
        .get("k4-cmp4-validity-analysis")
        .expect("k4")
        .case
        .clone()
}

fn fake_dir(names: &[&str], body: &str) -> std::path::PathBuf {
    let d = tempfile::tempdir().expect("dir").keep();
    for n in names {
        let mut c = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"cat > "$1" && chmod 755 "$1""#)
            .arg("sh")
            .arg(d.join(n))
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        std::io::Write::write_all(&mut c.stdin.take().expect("stdin"), body.as_bytes())
            .expect("write");
        assert!(c.wait().expect("wait").success());
    }
    d
}

fn daemon(dir: &Path, env: Vec<(String, String)>) -> cap_probe::WorkerDaemonEnv {
    cap_probe::WorkerDaemonEnv {
        path: Some(format!("{}:/usr/bin:/bin", dir.display())),
        env,
    }
}

const PY_STUB: &str = "#!/bin/sh\necho \"Python 3 pwd=$PWD mark=$CAP_MARK\"\n";

/// 子 process は daemon 設定の PATH・env と worker の cwd で走り、実起動関数の run（request.json）が残る。
#[test]
fn worker_probe_runs_through_run_subprocess_with_daemon_path_env_and_cwd() {
    let tools = fake_dir(&["python3"], PY_STUB);
    let env = env_json(json!({
        "env": [["CAP_MARK", "m1"], ["PATH", tools.display().to_string()]]
    }));
    let ws = tempfile::tempdir().expect("ws");
    let d = daemon(&tools, vec![("CAP_MARK".into(), "m1".into())]);
    for arm in [ConfigId::B, ConfigId::C] {
        let rep = cap_probe::probe_worker(&env, &d, arm, ws.path(), &k4());
        assert!(rep.ok, "{:?}", rep.results);
        assert_eq!(rep.paid_calls_made, 0);
        let out =
            std::fs::read_to_string(ws.path().join("artifacts/cap-probe/0.out")).expect("out");
        let cwd = std::fs::canonicalize(ws.path()).expect("canon");
        assert!(out.contains("mark=m1"), "{out}");
        assert!(out.contains(&format!("pwd={}", cwd.display())), "{out}");
        assert!(ws.path().join("runs/cap-probe/request.json").exists());
        assert!(ws.path().join("runs/cap-probe/stdout.jsonl").exists());
    }
}

fn failing(rep: &cap_probe::ProbeReport) -> Vec<&str> {
    rep.results
        .iter()
        .filter(|r| !r.ok)
        .map(|r| r.id.as_str())
        .collect()
}

#[test]
fn worker_probe_missing_socket_is_env_failure_per_arm() {
    let tools = fake_dir(&["python3"], PY_STUB);
    let sock = tempfile::tempdir().expect("s").keep().join("absent.sock");
    let env = env_json(json!({"sockets":[sock]}));
    let d = daemon(&tools, vec![]);
    let ws = tempfile::tempdir().expect("ws");
    for arm in [ConfigId::B, ConfigId::C] {
        let rep = cap_probe::probe_worker(&env, &d, arm, ws.path(), &k4());
        assert!(!rep.ok);
        assert_eq!(rep.arm, arm);
        assert_eq!(rep.failure_class, Some("env_failure"));
        assert_eq!(failing(&rep), vec![format!("socket:{}", sock.display())]);
    }
}

#[test]
fn worker_probe_missing_toolchain_is_env_failure_per_arm() {
    let tools = fake_dir(&["python3"], PY_STUB);
    let mount = tempfile::tempdir().expect("m").keep().join("absent-prefix");
    let env = env_json(json!({"ro_mounts":[mount]}));
    let d = daemon(&tools, vec![]);
    let ws = tempfile::tempdir().expect("ws");
    for arm in [ConfigId::B, ConfigId::C] {
        let rep = cap_probe::probe_worker(&env, &d, arm, ws.path(), &k4());
        assert_eq!(rep.failure_class, Some("env_failure"));
        assert_eq!(
            failing(&rep),
            vec![format!("toolchain:{}", mount.display())]
        );
    }
    // 道具（python3）そのものが daemon の PATH に無い場合。
    let empty = fake_dir(&[], "");
    let rep = cap_probe::probe_worker(
        &env_json(json!({})),
        &cap_probe::WorkerDaemonEnv {
            path: Some(empty.display().to_string()),
            env: vec![],
        },
        ConfigId::C,
        ws.path(),
        &k4(),
    );
    assert_eq!(rep.failure_class, Some("env_failure"));
    assert!(failing(&rep).contains(&"python3"), "{:?}", rep.results);
}

#[test]
fn worker_probe_daemon_config_mismatch_is_env_failure() {
    let tools = fake_dir(&["python3"], PY_STUB);
    let other = fake_dir(&[], "");
    let env = env_json(json!({
        "env": [["CAP_MARK", "m1"], ["PATH", other.display().to_string()]]
    }));
    let ws = tempfile::tempdir().expect("ws");
    // env 値の違い・欠落、期待 PATH 要素が daemon の PATH に無い。
    let d = daemon(&tools, vec![("CAP_MARK".into(), "other".into())]);
    let rep = cap_probe::probe_worker(&env, &d, ConfigId::B, ws.path(), &k4());
    assert_eq!(rep.failure_class, Some("env_failure"));
    let details: Vec<_> = rep
        .results
        .iter()
        .filter(|r| r.id == "worker-env")
        .map(|r| r.detail.clone())
        .collect();
    assert_eq!(details.len(), 2, "{details:?}");
    assert!(details.iter().any(|x| x.contains("CAP_MARK")));
    assert!(details.iter().any(|x| x.contains("PATH lacks")));
    let none = daemon(&tools, vec![]);
    let rep = cap_probe::probe_worker(&env, &none, ConfigId::C, ws.path(), &k4());
    assert!(
        rep.results
            .iter()
            .any(|r| r.detail.contains("daemon env lacks CAP_MARK"))
    );
}

/// Rust が無い/古い host は最初の LLM 呼出し前に env_failure で止まる。検査は部分一致 `rustc 1.` ではなく最小版の比較で、
/// min-build は transport-locusta feature を含む。ビルド場所は case に明示（cluster は接続確認のみ）。
#[test]
fn missing_or_old_rust_stops_before_any_llm_call() {
    let set = cases();
    let case = &set.get("k2-bf-xattr-restart-campaign").expect("k2").case;
    let cap = |id: &str| {
        case.required_capabilities
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("k2 lacks {id}"))
    };
    let min = cap("rustc-min");
    let script = min.argv.join(" ");
    assert!(
        script.contains("sort -V") && script.contains("1.85.0"),
        "{script}"
    );
    assert!(!script.contains("rustc 1."), "no substring match");
    let build = cap("min-build-locusta").argv.join(" ");
    assert!(build.contains("--features transport-locusta"), "{build}");
    assert!(
        case.experiment.as_ref().expect("experiment").params["build_location"]
            .as_str()
            .is_some_and(|s| s.contains("local-host") && s.contains("接続確認のみ")),
    );
    // rustc 系の能力だけ不在の host（他は ok）。
    struct NoRust;
    impl ProbeRunner for NoRust {
        fn run(&self, _: &str, args: &[String]) -> Ran {
            let joined = args.join(" ");
            let rust = joined.contains("rustc --version") || joined.contains("cargo build");
            Ran::Exit {
                code: if rust { 127 } else { 0 },
                stdout: "cmake version git version rustc-min ok".into(),
            }
        }
    }
    let env = env_json(json!({}));
    for arm in [ConfigId::A, ConfigId::B, ConfigId::C] {
        let rep = cap_probe::probe(&sb(&env, arm), arm, Path::new("/w"), case, &NoRust);
        assert!(!rep.ok);
        assert_eq!(rep.failure_class, Some("env_failure"));
        assert_eq!(rep.paid_calls_made, 0);
        let bad: Vec<_> = rep
            .results
            .iter()
            .filter(|r| !r.ok)
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(bad, ["rustc-min", "min-build-locusta"], "{arm:?}");
    }
}

/// 古い rustc は `sort -V` の最小版比較で落ちる。case の実 argv を fake rustc 付き PATH で実行する。
#[test]
fn old_rustc_fails_the_min_version_comparison() {
    use std::os::unix::fs::PermissionsExt;
    let set = cases();
    let case = &set.get("k2-bf-xattr-restart-campaign").expect("k2").case;
    let argv = &case
        .required_capabilities
        .iter()
        .find(|c| c.id == "rustc-min")
        .expect("rustc-min")
        .argv;
    let run = |version: &str| {
        let dir = tempfile::tempdir().expect("tmp");
        for (name, body) in [
            (
                "rustc",
                format!("#!/bin/sh\necho 'rustc {version} (fake)'\n"),
            ),
            ("cargo", "#!/bin/sh\necho 'cargo fake'\n".to_string()),
        ] {
            let bin = dir.path().join(name);
            std::fs::write(&bin, body).expect("write");
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let path = format!("{}:/usr/bin:/bin", dir.path().display());
        std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .env("PATH", path)
            .output()
            .expect("run")
    };
    assert!(!run("1.2.0").status.success(), "old rustc must fail");
    let ok = run("1.85.0");
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(String::from_utf8_lossy(&ok.stdout).contains("rustc-min ok"));
}
