//! ADR 2026-11-12-host-check-slots の試験。時間は `tokio::time::pause` で進め、負荷は「枠を別の holder が持つ」で与える
//! （CPU を焼かない。agent-docs/guides/testing.md）。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use task_core::{ArtifactRef, Task};

use super::*;
use crate::workspace::{ExecResult, Workspace, WorkspaceError};

/// exec の呼び出し（command・timeout・呼ばれた時刻）を記録し、常に exit 0 を返す作業場所。
#[derive(Clone, Default)]
struct Recording {
    calls: Arc<Mutex<Vec<(String, Duration, tokio::time::Instant)>>>,
}

impl Recording {
    fn calls(&self) -> Vec<(String, Duration, tokio::time::Instant)> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl Workspace for Recording {
    async fn prepare(&self, _task: &Task) -> Result<std::path::PathBuf, WorkspaceError> {
        Ok(std::path::PathBuf::from("."))
    }
    async fn exec(&self, cmd: &str, timeout: Duration) -> Result<ExecResult, WorkspaceError> {
        if let Ok(mut c) = self.calls.lock() {
            c.push((cmd.to_string(), timeout, tokio::time::Instant::now()));
        }
        Ok(ExecResult {
            exit: Some(0),
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            timed_out: false,
        })
    }
    async fn collect(&self, _task: &Task) -> Result<Vec<ArtifactRef>, WorkspaceError> {
        Ok(Vec::new())
    }
}

fn counting_note() -> (WaitNote, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let note: WaitNote = Arc::new(move |_label: &str, reason: &str| {
        if let Ok(mut v) = s.lock() {
            v.push(reason.to_string());
        }
    });
    (note, seen)
}

/// 不変条件（D3・D5）: 枠がすべて使用中の間、重い検査は始まらず、待った時間は timeout に数えない（枠を取った後に
/// 元の timeout をまるごと渡す）。待ちの理由は 1 回だけ残り、検査は時間切れにも人への質問にもならず合格する。
/// 既存の試験は枠の無い `exec` しか見ておらず、待ちを timeout に数える退行を検出できない。
#[tokio::test(start_paused = true)]
async fn a_heavy_check_waits_for_a_free_slot_and_the_wait_is_not_counted_in_its_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let slots = CheckSlots::new(dir.path(), 1).with_poll(Duration::from_secs(5));
    let other = slots
        .try_take("release gate: test-parallel.sh")
        .expect("take")
        .expect("the only slot is free");
    let inner = Recording::default();
    let (note, seen) = counting_note();
    let ws = SlottedWorkspace::new(Box::new(inner.clone()), slots.clone(), "review 01TASK")
        .with_wait_note(note);
    let timeout = Duration::from_secs(60);
    let check =
        tokio::spawn(async move { ws.exec("bash scripts/dev/test-affected.sh", timeout).await });

    // timeout の 10 倍待っても、枠が空かなければ検査は始まらない。
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(
        inner.calls().is_empty(),
        "the check must not start without a slot"
    );
    assert!(
        !check.is_finished(),
        "the check must keep waiting, not time out"
    );
    let status = slots.status();
    assert_eq!(status.held.len(), 1);
    assert_eq!(status.held[0].label, "release gate: test-parallel.sh");
    assert_eq!(status.waiting.len(), 1, "{status:?}");
    assert_eq!(
        status.waiting[0].label,
        "review 01TASK: bash scripts/dev/test-affected.sh"
    );
    assert!(
        status.waiting[0]
            .reason
            .contains("release gate: test-parallel.sh"),
        "{status:?}"
    );

    let released_at = tokio::time::Instant::now();
    drop(other);
    let result = check.await.expect("join").expect("exec");
    assert_eq!(result.exit, Some(0));
    assert!(!result.timed_out);

    let calls = inner.calls();
    assert_eq!(calls.len(), 1);
    let (cmd, given, at) = &calls[0];
    assert_eq!(
        *given, timeout,
        "the whole timeout is given after the slot is taken"
    );
    assert!(*at >= released_at);
    assert!(cmd.starts_with("CELERIS_CHECK_SLOT='"), "{cmd}");
    assert!(
        cmd.ends_with("; bash scripts/dev/test-affected.sh"),
        "{cmd}"
    );

    let reasons = seen.lock().map(|v| v.clone()).unwrap_or_default();
    assert_eq!(
        reasons.len(),
        1,
        "the wait reason is recorded once: {reasons:?}"
    );
    assert!(reasons[0].contains("枠 1 本がすべて使用中"), "{reasons:?}");
    let after = slots.status();
    assert!(
        after.waiting.is_empty() && after.held.is_empty(),
        "{after:?}"
    );
}

/// 不変条件（D4）: 軽い検査（cargo・試験の script を含まない）は枠が全部使用中でも待たずに走る。
#[tokio::test(start_paused = true)]
async fn a_light_check_never_waits_for_a_slot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let slots = CheckSlots::new(dir.path(), 1);
    let _other = slots.try_take("other").expect("take").expect("free");
    let inner = Recording::default();
    let (note, seen) = counting_note();
    let ws = SlottedWorkspace::new(Box::new(inner.clone()), slots, "review").with_wait_note(note);
    let r = ws
        .exec(
            "sh scripts/dev/progress-index.sh --check",
            Duration::from_secs(5),
        )
        .await
        .expect("exec");
    assert_eq!(r.exit, Some(0));
    assert_eq!(
        inner.calls()[0].0,
        "sh scripts/dev/progress-index.sh --check"
    );
    assert!(seen.lock().map(|v| v.is_empty()).unwrap_or(false));
}

/// 不変条件（D4）: 重い command の判定は決定的な字句で、cargo・nextest・試験の script を含む command だけ。
#[test]
fn heavy_commands_are_the_cargo_and_test_scripts() {
    for heavy in [
        "bash scripts/dev/test-affected.sh",
        "cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check",
        "/home/u/.cargo/bin/cargo nextest run -p task-core",
        "bash scripts/dev/test-parallel.sh",
        "(cd web && cargo build)",
    ] {
        assert!(is_heavy(heavy), "{heavy}");
    }
    for light in [
        "sh scripts/dev/progress-index.sh --check",
        "git merge-base --is-ancestor main HEAD",
        "test -f artifacts/report.md",
        "grep -q cargoship README.md",
    ] {
        assert!(!is_heavy(light), "{light}");
    }
    assert_eq!(default_slots(24), 2);
    assert_eq!(default_slots(8), 1);
    assert_eq!(default_slots(48), 4);
}

/// 不変条件（D1・D6）: 使用中の判定は lock で行い、drop で枠は空く。死んだ pid の待ちは status に出さない。
#[test]
fn status_lists_holders_and_live_waiters() {
    let dir = tempfile::tempdir().expect("tempdir");
    let slots = CheckSlots::new(dir.path(), 2);
    let a = slots.try_take("a").expect("take").expect("free");
    let b = slots.try_take("b").expect("take").expect("free");
    assert!(slots.try_take("c").expect("take").is_none());
    std::fs::create_dir_all(dir.path().join("waiting")).expect("mkdir");
    std::fs::write(
        dir.path().join("waiting/dead"),
        "pid=4294967294\nlabel=gone\nsince=x\nreason=r\n",
    )
    .expect("write");
    std::fs::write(
        dir.path().join("waiting/live"),
        format!(
            "pid={}\nlabel=worker test-affected\nsince=t\nreason=host の検査の枠 2 本がすべて使用中（a、b）\n",
            std::process::id()
        ),
    )
    .expect("write");
    let s = slots.status();
    let labels: Vec<&str> = s.held.iter().map(|h| h.label.as_str()).collect();
    assert_eq!(labels, vec!["a", "b"]);
    assert_eq!(s.waiting.len(), 1);
    assert_eq!(s.waiting[0].label, "worker test-affected");
    drop(a);
    assert_eq!(slots.status().held.len(), 1);
    // holder の file が残っていても、lock が外れていれば空き（kill -9 で Drop が走らない場合）。
    std::fs::write(dir.path().join("slot-0.holder"), "label=stale\n").expect("write");
    assert_eq!(slots.status().held.len(), 1);
    drop(b);
    assert!(slots.status().held.is_empty());
}

/// bash に `scripts/dev/lib/check-slot.sh` を読ませて `script` を走らせる。env はこの Command にだけ渡し（process の env は
/// 変えない）、`PATH` と `HOME`（試験の一時 dir）のほかは空にする。core 数は `OMP_NUM_THREADS` で与える（GNU nproc が従う）。
fn run_shell_lib(script: &str, home: &Path, env: &[(&str, &str)]) -> std::process::Output {
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/dev/lib/check-slot.sh");
    let mut cmd = std::process::Command::new("bash");
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .arg("-c")
        .arg(format!(". \"$1\"; {script}"))
        .arg("check-slot-parity")
        .arg(lib);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run bash")
}

fn stdout_lines(out: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// 不変条件（D2）: 枠の数と試験の並列度は daemon（Rust の `slot_count`・`test_threads`）と script（`check-slot.sh` の
/// `check_slot_count`・`check_slot_test_threads`）で同じ値になる（core 数 1・12・24・48 と `CELERIS_CHECK_SLOTS` の値ごと）。
/// 式は言語ごとに 2 か所にあり、既存の試験は Rust の `default_slots` しか見ないので、片方だけ変えた食い違いを検出できない。
#[test]
fn slot_count_and_test_threads_match_the_shell_lib() {
    let home = tempfile::tempdir().expect("tempdir");
    for (cores, slots, threads) in [(1, 1, 1), (12, 1, 12), (24, 2, 12), (48, 4, 12)] {
        assert_eq!(slot_count(None, cores), slots, "cores {cores}");
        assert_eq!(test_threads(cores, slots), threads, "cores {cores}");
    }
    for cores in [1usize, 12, 24, 48] {
        for slots_env in [
            None,
            Some(""),
            Some("3"),
            Some("08"),
            Some("0"),
            Some("00"),
            Some("abc"),
            Some("+3"),
            Some(" 3"),
        ] {
            let omp = cores.to_string();
            let mut env = vec![("OMP_NUM_THREADS", omp.as_str())];
            if let Some(v) = slots_env {
                env.push((SLOTS_ENV, v));
            }
            let out = run_shell_lib(
                "check_slot_count; check_slot_test_threads",
                home.path(),
                &env,
            );
            assert!(out.status.success(), "{out:?}");
            let slots = slot_count(slots_env, cores);
            assert_eq!(
                stdout_lines(&out),
                vec![slots.to_string(), test_threads(cores, slots).to_string()],
                "cores {cores}, {SLOTS_ENV}={slots_env:?}"
            );
        }
    }
}

/// 不変条件（D1）: 枠の dir は `CELERIS_CHECK_SLOTS_DIR`、無ければ `$XDG_RUNTIME_DIR/celeris/check-slots` で、daemon と
/// script が同じ dir を選ぶ。どちらの env も無ければ dir を決めず、`HOME` には落ちない（script は理由を stderr に出して
/// exit 2、`HOME` に何も作らない）。既存の試験は dir を直に渡すので、既定の dir の解決と NFS の home への退行を検出できない。
#[test]
fn slots_dir_matches_the_shell_lib_and_never_falls_back_to_home() {
    let home = tempfile::tempdir().expect("tempdir");
    for (dir_env, runtime) in [
        (Some("/srv/slots"), Some("/run/user/7")),
        (Some("/srv/slots"), None),
        (None, Some("/run/user/7")),
        (Some(""), Some("/run/user/7")),
        (None, None),
        (Some(""), Some("")),
    ] {
        let rust = slots_dir(dir_env.map(OsStr::new), runtime.map(OsStr::new));
        let mut env = Vec::new();
        if let Some(v) = dir_env {
            env.push((SLOTS_DIR_ENV, v));
        }
        if let Some(v) = runtime {
            env.push((RUNTIME_DIR_ENV, v));
        }
        let out = run_shell_lib("check_slot_dir", home.path(), &env);
        let case = format!("{SLOTS_DIR_ENV}={dir_env:?} {RUNTIME_DIR_ENV}={runtime:?}");
        match rust {
            Some(dir) => {
                assert!(out.status.success(), "{case}: {out:?}");
                assert_eq!(
                    stdout_lines(&out),
                    vec![dir.to_string_lossy().to_string()],
                    "{case}"
                );
            }
            None => {
                assert_eq!(out.status.code(), Some(2), "{case}: {out:?}");
                assert!(
                    String::from_utf8_lossy(&out.stderr).contains("XDG_RUNTIME_DIR"),
                    "{case}: {out:?}"
                );
            }
        }
    }
    assert_eq!(
        slots_dir(None, Some(OsStr::new("/run/user/7"))),
        Some(PathBuf::from("/run/user/7/celeris/check-slots"))
    );
    assert!(CheckSlots::resolve(None, None, None, 24).is_none());

    // script: 枠を取る前に dir を決められなければ exit 2 で止まり、HOME に何も作らない。
    let out = run_shell_lib(
        "check_slot_acquire parity",
        home.path(),
        &[("OMP_NUM_THREADS", "24")],
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("XDG_RUNTIME_DIR"),
        "{out:?}"
    );
    assert_eq!(
        std::fs::read_dir(home.path()).expect("read home").count(),
        0,
        "nothing is created under HOME"
    );
}

/// warn の event を数える tracing の subscriber（試験用）。
struct CountWarns(Arc<std::sync::atomic::AtomicUsize>);

impl tracing::Subscriber for CountWarns {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::WARN {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// 不変条件（D3）: 枠の dir を決められない daemon は warn を 1 行出し、検査の作業場所を包まずに返す（重い検査も枠なしで
/// 走り、`CELERIS_CHECK_SLOT` を渡さない）。dir が決まれば包む。既存の試験は `SlottedWorkspace::new` を直に使うので、
/// env が欠けたときの扱いの退行（別の dir に落ちる・止まる）を検出できない。
#[tokio::test]
async fn without_a_slot_dir_the_daemon_warns_once_and_runs_checks_unwrapped() {
    let warns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let inner = Recording::default();
    let ws = tracing::subscriber::with_default(CountWarns(warns.clone()), || {
        SlottedWorkspace::wrap_with(
            Box::new(inner.clone()),
            CheckSlots::resolve(None, None, None, 24),
            "review",
        )
    });
    assert_eq!(warns.load(Ordering::SeqCst), 1);
    ws.exec("bash scripts/dev/test-affected.sh", Duration::from_secs(5))
        .await
        .expect("exec");
    assert_eq!(inner.calls()[0].0, "bash scripts/dev/test-affected.sh");

    let runtime = tempfile::tempdir().expect("tempdir");
    let slots = CheckSlots::resolve(None, Some(runtime.path().as_os_str()), None, 24)
        .expect("XDG_RUNTIME_DIR gives the dir");
    assert_eq!(slots.dir(), runtime.path().join("celeris/check-slots"));
    assert_eq!(slots.slots(), 2);
    let wrapped = Recording::default();
    let ws = SlottedWorkspace::wrap_with(Box::new(wrapped.clone()), Some(slots), "review");
    ws.exec("bash scripts/dev/test-affected.sh", Duration::from_secs(5))
        .await
        .expect("exec");
    assert!(
        wrapped.calls()[0].0.starts_with("CELERIS_CHECK_SLOT='"),
        "{:?}",
        wrapped.calls()
    );
}
