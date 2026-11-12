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
