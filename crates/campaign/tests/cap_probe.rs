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
            stdout: "cmake version git version Python 3".into(),
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
    fn probe_runner(&self) -> Box<dyn ProbeRunner> {
        if self.0 {
            Box::new(OkRunner)
        } else {
            Box::new(FailRunner)
        }
    }
    fn worker_host(&self) -> Box<dyn celeris_campaign::cap_probe::WorkerHost> {
        Box::new(WorkerStub {
            sockets: self.0,
            rust: self.0,
        })
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

/// B/C は A の sandbox ではなく worker 環境で確かめる。A が正常でも worker から socket / Rust を外すと、
/// その arm だけが preflight（最初の LLM 呼出しの前）で env_failure になる。
struct WorkerStub {
    sockets: bool,
    rust: bool,
}
impl celeris_campaign::cap_probe::WorkerHost for WorkerStub {
    fn socket_present(&self, _: &Path) -> bool {
        self.sockets
    }
    fn dir_present(&self, _: &Path) -> bool {
        true
    }
    fn run(&self, env: &[(String, String)], argv: &[String], _: &Path) -> Ran {
        assert!(
            env.iter().any(|(k, _)| k == "PATH"),
            "worker env is built explicitly"
        );
        if !self.rust && argv.join(" ").contains("cargo") || !self.rust && argv[0] == "rustc" {
            return Ran::Exit {
                code: 127,
                stdout: String::new(),
            };
        }
        Ran::Exit {
            code: 0,
            stdout: "cmake version git version Python 3 rustc 1.80 cargo 1.80".into(),
        }
    }
}
struct WorkerHostStub(StubHost, std::sync::Arc<WorkerStub>);
struct Shared(std::sync::Arc<WorkerStub>);
impl celeris_campaign::cap_probe::WorkerHost for Shared {
    fn socket_present(&self, p: &Path) -> bool {
        self.0.socket_present(p)
    }
    fn dir_present(&self, p: &Path) -> bool {
        self.0.dir_present(p)
    }
    fn run(&self, e: &[(String, String)], a: &[String], w: &Path) -> Ran {
        self.0.run(e, a, w)
    }
}
impl Host for WorkerHostStub {
    fn env_nonempty(&self, n: &str) -> bool {
        self.0.env_nonempty(n)
    }
    fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
        self.0.resolve_program(p)
    }
    fn api_get(&self, a: &str, t: &str, p: &str) -> Result<Value, String> {
        self.0.api_get(a, t, p)
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
    fn probe_runner(&self) -> Box<dyn ProbeRunner> {
        Box::new(OkRunner) // A は正常
    }
    fn worker_host(&self) -> Box<dyn celeris_campaign::cap_probe::WorkerHost> {
        Box::new(Shared(self.1.clone()))
    }
}

#[test]
fn worker_missing_socket_or_rust_is_env_failure_for_that_arm_only() {
    use std::sync::Arc;
    let s = setup();
    // 承認済み socket を worker 環境に宣言する（B/C が確認すべき接続）
    let tc = json!({"ro_mounts":[],"sockets":["/run/approved/mgmt.sock"],"env":[]});
    std::fs::write(
        s.inp.cap_env.as_ref().unwrap(),
        json!({"schema":"celeris.campaign-cap-env/1","a":tc,"worker":tc}).to_string(),
    )
    .unwrap();
    let run = |sockets, rust| {
        let h = WorkerHostStub(StubHost(true), Arc::new(WorkerStub { sockets, rust }));
        let r = check(&s.inp, &h);
        let m: Vec<String> = r
            .missing
            .iter()
            .filter(|m| m.check == "capability")
            .map(|m| m.detail.clone())
            .collect();
        (m, r.failure_class)
    };
    let (m, class) = run(true, true);
    assert!(m.is_empty(), "{m:?}");
    assert_eq!(class, None);
    let (m, class) = run(false, true);
    assert_eq!(class, Some("env_failure"));
    assert!(
        m.iter()
            .any(|x| x.contains("arm B") && x.contains("worker-socket")),
        "{m:?}"
    );
    assert!(
        m.iter().all(|x| !x.contains("arm A")),
        "A stays healthy: {m:?}"
    );
    let (m, class) = run(true, false);
    assert_eq!(class, Some("env_failure"));
    for id in ["rustc", "cargo", "min-build-cargo"] {
        assert!(
            m.iter().any(|x| x.contains("arm C") && x.contains(id)),
            "{id}: {m:?}"
        );
    }
    let err = gate(
        &s.inp,
        &WorkerHostStub(
            StubHost(true),
            Arc::new(WorkerStub {
                sockets: true,
                rust: false,
            }),
        ),
    )
    .unwrap_err();
    assert!(err.contains("no paid execution or task was started"));
}
