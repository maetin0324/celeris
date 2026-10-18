//! 必要能力 probe: case 固定・A の sandbox argv・不足は env_failure で最初の LLM 呼出し前に停止・B/C は同じ能力を
//! 測定専用 daemon の worker で検査する。userns が要る実走は worker sandbox で出来ないので、A は argv 組立・判定・
//! 停止順序を stub runner で、B/C は結果の判定を stub で確かめる（daemon の worker 起動経路は別 file の実 process 試験）。
use std::{cell::RefCell, fs, path::Path};

use celeris_campaign::{
    cap_probe::{self, CapEnv, ProbeRunner, Ran},
    cases::{CapKind, CaseSet},
    harness_exec::{HarnessCommand, Sandbox, Toolchain},
    manifest::ConfigId,
    preflight::{Host, ResolvedProgram, check, gate},
};
use serde_json::{Value, json};

mod daemon_identity;
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

/// `.0`: worker の検査が通るか、`.1`: A の probe が通るか。`.2`: 測定 daemon へ投入された検査 task の数。
struct StubHost(bool, bool, std::cell::Cell<usize>);
impl StubHost {
    fn new(worker_ok: bool, a_ok: bool) -> Self {
        Self(worker_ok, a_ok, std::cell::Cell::new(0))
    }
}
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
        match path {
            "/api/v1/config" => Ok(daemon_identity::matching_daemon_body()),
            // 他の検査（task-template・A のアカウント）を通すための偽 API。
            "/api/v1/projects/01ABC" => {
                Ok(json!({"project":{"id":"01ABC","auto_integrate":false},"repos":[]}))
            }
            "/api/v1/projects/01ABC/repos" => Ok(
                json!({"items":[{"name":"scratch","is_primary":true,"location":{"kind":"local","path":"/x"}}]}),
            ),
            "/api/v1/llm/accounts" => {
                static D: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
                let d = D.get_or_init(|| tempfile::tempdir().unwrap()).path();
                Ok(
                    json!({"items":[{"source_id":"claude_oauth","account_id":"lab","dir":d,"logged_in":true}]}),
                )
            }
            _ => Err(format!("GET {path}: no api in this test")),
        }
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
    /// daemon の worker が返す結果の代役（経路そのものは cap_probe_daemon.rs / cap_probe_real_daemon.rs）。
    fn probe_daemon_worker(
        &self,
        _: &str,
        _: &str,
        spec: &cap_probe::WorkerProbeSpec,
    ) -> Result<cap_probe::WorkerProbeRun, String> {
        self.2.set(self.2.get() + 1);
        let mut run = ok_worker_run(spec);
        if !self.0 {
            // 道具（sirius-link）が worker の PATH に無い。
            for r in run.results.iter_mut().filter(|r| r.id == "sirius-link") {
                r.ok = false;
                r.detail = "exit 127".into();
            }
        }
        Ok(run)
    }
    fn probe_runner(&self) -> Box<dyn ProbeRunner> {
        if self.1 {
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
    let r = check(inp, &StubHost::new(ok, true));
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
    let err = gate(&s.inp, &StubHost::new(false, true)).unwrap_err();
    assert!(err.contains("no paid execution or task was started") && err.contains("env_failure"));
    // 他の検査（A の probe）が落ちている preflight は、B/C の検査 task を測定 daemon に 1 件も投入しない。
    let host = StubHost::new(true, false);
    let r = check(&s.inp, &host);
    assert!(r.missing.iter().any(|m| m.check == "capability"));
    assert_eq!(host.2.get(), 0, "no probe task may be submitted");
    // 対照: A が通れば投入される。
    let host = StubHost::new(true, true);
    assert!(check(&s.inp, &host).missing.is_empty());
    assert!(host.2.get() > 0);
    // cap env が無いのも不足（fail-closed）。
    let mut inp = s.inp.clone();
    inp.cap_env = None;
    let (miss, class) = capability_missing(&inp, true);
    assert!(miss.iter().any(|m| m.contains("--cap-env is required")));
    assert_eq!(class, Some("env_failure"));
}

// ---- B/C: probe_worker の判定（worker の結果 → 報告）。daemon の worker 起動経路は cap_probe_daemon.rs ----

fn k4() -> celeris_campaign::cases::Case {
    cases()
        .get("k4-cmp4-validity-analysis")
        .expect("k4")
        .case
        .clone()
}

fn hs() -> Vec<String> {
    vec!["claude-code".into()]
}

fn target() -> cap_probe::DaemonRef {
    cap_probe::DaemonRef {
        pid: 4242,
        instance_id: Some("inst-1".into()),
        release: Some("0123456789ab".into()),
        probe_task_id: None,
    }
}

#[test]
fn probe_worker_reports_identity_and_task_of_the_daemon_that_ran_it() {
    let env = env_json(json!({}));
    for arm in [ConfigId::B, ConfigId::C] {
        let rep = cap_probe::probe_worker(
            &env,
            &target(),
            arm,
            &k4(),
            &["claude-code".into()],
            &|spec| Ok(ok_worker_run(spec)),
        );
        assert!(rep.ok, "{:?}", rep.results);
        assert_eq!(rep.paid_calls_made, 0);
        let d = rep.daemon.expect("identity");
        assert_eq!((d.pid, d.instance_id.as_deref()), (4242, Some("inst-1")));
        assert_eq!(d.probe_task_id.as_deref(), Some("01TESTPROBETASK"));
    }
}

#[test]
fn probe_worker_failed_check_missing_result_or_no_run_is_env_failure() {
    let env = env_json(json!({}));
    // worker が 1 件落とした。
    let rep = cap_probe::probe_worker(&env, &target(), ConfigId::B, &k4(), &hs(), &|spec| {
        let mut run = ok_worker_run(spec);
        run.results[1].ok = false;
        run.results[1].detail = "exit 127".into();
        Ok(run)
    });
    assert_eq!(rep.failure_class, Some("env_failure"));
    assert_eq!(rep.arm, ConfigId::B);
    // 返さなかった検査は不合格（黙って通さない）。
    let rep = cap_probe::probe_worker(&env, &target(), ConfigId::C, &k4(), &hs(), &|spec| {
        let mut run = ok_worker_run(spec);
        run.results.truncate(1);
        Ok(run)
    });
    assert_eq!(rep.failure_class, Some("env_failure"));
    assert!(
        rep.results
            .iter()
            .any(|r| !r.ok && r.detail.contains("no result returned")),
        "{:?}",
        rep.results
    );
    // daemon が検査 task を走らせなかった（harness 未宣言など）。
    let rep = cap_probe::probe_worker(&env, &target(), ConfigId::C, &k4(), &hs(), &|_| {
        Err("task did not run".into())
    });
    assert_eq!(rep.failure_class, Some("env_failure"));
    assert!(
        rep.results
            .iter()
            .any(|r| r.id == "claude-code/worker-run" && !r.ok)
    );
    assert_eq!(rep.daemon.expect("identity").pid, 4242);
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

/// min-build-locusta は継承された CARGO_TARGET_DIR に左右されない（probe 専用 dir に出力し、そこを実行する）。
/// 実際の cargo で、未設定と別の絶対 path の両方が成功する。
#[test]
fn min_build_locusta_ignores_inherited_cargo_target_dir() {
    let set = cases();
    let case = &set.get("k2-bf-xattr-restart-campaign").expect("k2").case;
    let argv = &case
        .required_capabilities
        .iter()
        .find(|c| c.id == "min-build-locusta")
        .expect("min-build-locusta")
        .argv;
    let run = |target: Option<&Path>, path: Option<&str>| {
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]).env_remove("CARGO_TARGET_DIR");
        if let Some(t) = target {
            cmd.env("CARGO_TARGET_DIR", t);
        }
        if let Some(p) = path {
            cmd.env("PATH", p);
        }
        cmd.output().expect("run")
    };
    let unset = run(None, None);
    assert!(
        unset.status.success(),
        "{}",
        String::from_utf8_lossy(&unset.stderr)
    );
    let shared = tempfile::tempdir().expect("tmp");
    let set_abs = run(Some(shared.path()), None);
    assert!(
        set_abs.status.success(),
        "{}",
        String::from_utf8_lossy(&set_abs.stderr)
    );
    // probe は共有 target を使わず、書き込みもしない。
    assert_eq!(std::fs::read_dir(shared.path()).expect("ls").count(), 0);

    // 既存成果物を偶然実行して合格しない: build が何も出力しない cargo と、継承 target の古い t。
    use std::os::unix::fs::PermissionsExt;
    let stale = tempfile::tempdir().expect("tmp");
    std::fs::create_dir_all(stale.path().join("release")).expect("mkdir");
    let old = stale.path().join("release/t");
    std::fs::write(&old, "#!/bin/sh\nexit 0\n").expect("write");
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let bin = tempfile::tempdir().expect("tmp");
    let cargo = bin.path().join("cargo");
    std::fs::write(&cargo, "#!/bin/sh\nexit 0\n").expect("write");
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = format!("{}:/usr/bin:/bin", bin.path().display());
    let fake = run(Some(stale.path()), Some(&path));
    assert!(!fake.status.success(), "stale t must not pass the probe");
}

/// arm の worker harness ごとに検査 task を投入し、不足は harness 名つきで報告する（B/C の harness 差を畳まない）。
#[test]
fn probe_worker_runs_one_task_per_arm_harness_and_names_the_failing_one() {
    let env = env_json(json!({}));
    let seen = std::cell::RefCell::new(Vec::new());
    let rep = cap_probe::probe_worker(
        &env,
        &target(),
        ConfigId::B,
        &k4(),
        &["claude-code".into(), "codex".into()],
        &|spec| {
            seen.borrow_mut().push(spec.harness.clone());
            let mut run = ok_worker_run(spec);
            if spec.harness == "codex" {
                run.results[1].ok = false;
                run.results[1].detail = "exit 127".into();
            }
            Ok(run)
        },
    );
    assert_eq!(*seen.borrow(), ["claude-code", "codex"]);
    assert_eq!(rep.failure_class, Some("env_failure"));
    let bad: Vec<_> = rep
        .results
        .iter()
        .filter(|r| !r.ok)
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(bad.len(), 1);
    assert!(bad[0].starts_with("codex/"), "{bad:?}");
    assert_eq!(cap_probe::probe_genre("codex"), "cap-probe-codex");
    // harness の無い arm は検査できない。
    let rep = cap_probe::probe_worker(&env, &target(), ConfigId::C, &k4(), &[], &|spec| {
        Ok(ok_worker_run(spec))
    });
    assert!(rep.results.iter().any(|r| r.id == "harness" && !r.ok));
}

/// 測定 config の宣言漏れ・別 harness への一致では、検査 task を投入せず env_failure（検査 JSON が実 harness に渡らない）。
#[test]
fn undeclared_or_misdeclared_probe_harness_submits_nothing_and_is_env_failure() {
    let s = setup();
    let good = fs::read_to_string(&s.inp.measure.as_ref().unwrap().config).unwrap();
    let decl = probe_declarations("celeris");
    assert!(good.contains(&decl));
    let base = good.replace(&decl, "");
    let cases: Vec<(&str, String, &str)> = vec![
        ("no declaration", base.clone(), "no harness_selection rule"),
        (
            "rule without genre (real harness could match)",
            format!(
                "{base}[harnesses.celeris]\nkind = \"fake\"\n[harness_selection]\nrules = [{{ harness = \"celeris\", required_capabilities = [] }}]\n"
            ),
            "no genre",
        ),
        (
            "genre bound to a real harness",
            format!(
                "{base}[harnesses.celeris]\nkind = \"claude_code\"\n[harness_selection]\nrules = [{{ genre = \"cap-probe-celeris\", harness = \"celeris\", required_capabilities = [] }}]\n"
            ),
            "not kind",
        ),
        (
            "env_of missing",
            decl.replace("env_of = \"celeris\"\n", ""),
            "env_of",
        ),
    ];
    for (label, text, want) in cases {
        let text = if label == "env_of missing" {
            format!("{base}{text}")
        } else {
            text
        };
        fs::write(&s.inp.measure.as_ref().unwrap().config, &text).unwrap();
        let host = StubHost::new(true, true);
        let r = check(&s.inp, &host);
        let miss: Vec<_> = r
            .missing
            .iter()
            .filter(|m| m.check == "capability")
            .map(|m| m.detail.clone())
            .collect();
        assert!(
            miss.iter()
                .any(|m| m.contains("not submitted") && m.contains(want)),
            "{label}: {miss:?} / {:?}",
            r.missing
        );
        assert_eq!(r.failure_class, Some("env_failure"), "{label}");
        assert_eq!(host.2.get(), 0, "{label}: nothing submitted");
    }
}
