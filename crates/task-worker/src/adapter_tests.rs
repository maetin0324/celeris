//! 実行共通処理（ADR 2026-11-10-adapter-result-contract D2・(4) 4b）の試験。harness adapter 5 本は
//! `prepare_run`・`deadline_terminal`・`with_estimated_cost`・`finish_run` を 1 か所で呼ぶので、run の準備・
//! 締め切り・cost の推定・終端の書き出しの規則はここで正本を直接試す。adapter ごとの試験は結線だけを見る
//! （fake の harness で時間切れや供給側失敗を起こすには実時間か stub の script が要り、規則の境界
//! 〈両方の超過・実測の cost・model の不明〉を 1 件ずつ固定できない）。

use std::os::unix::process::ExitStatusExt;

use super::*;
use crate::protocol::{ProviderFailure, RunContext, RunRequest};

const LONG: Duration = Duration::from_secs(1_000);

fn ago(secs: u64) -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(secs))
        .expect("the monotonic clock is past the test offset")
}

fn usage(cost_usd: Option<f64>) -> Option<Usage> {
    Some(Usage {
        input_tokens: Some(1_000),
        output_tokens: Some(2_000),
        cost_usd,
        ..Default::default()
    })
}

fn request(workspace: &Path) -> RunRequest {
    RunRequest {
        llm_route: None,
        cargo_target_dir: None,
        protocol: crate::protocol::PROTOCOL_VERSION,
        task: crate::protocol::tests::sample_task(),
        artifacts_dir: workspace.join("artifacts"),
        workspace: workspace.to_path_buf(),
        work_dir: None,
        context: RunContext::default(),
    }
}

/// 不変条件: wall clock の超過は `BudgetExhausted{WallClock}`、文言 `wall clock exceeded`（dispatcher の
/// `classify_budget_kind_from_text`・`is_budget_outcome` が見る語）で、idle も超過していても wall clock が勝つ。
/// adapter の試験は実時間の打ち切りを 1 経路ずつしか見ず、両方の超過の優先順位を固定していなかった。
#[test]
fn run_common_deadline_wall_clock_is_budget_exhausted() {
    let expected = Terminal::BudgetExhausted {
        kind: task_core::BudgetKind::WallClock,
        message: "wall clock exceeded".into(),
        usage: None,
    };
    let wall_only = deadline_terminal(ago(10), Instant::now(), Duration::from_secs(5), LONG);
    assert_eq!(wall_only, Err(Box::new(expected.clone())));
    let both = deadline_terminal(
        ago(10),
        ago(10),
        Duration::from_secs(5),
        Duration::from_secs(5),
    );
    assert_eq!(both, Err(Box::new(expected)));
}

/// 不変条件: idle の超過は harness_error に分類変更しない retryable な `Error`（文言 `idle timeout`、
/// ADR-0072 §7 U7）。どちらも超過していなければ、次に待つ時間は wall clock と idle の残りの小さい方。
#[test]
fn run_common_deadline_idle_is_retryable_error() {
    let idle = deadline_terminal(ago(1), ago(10), LONG, Duration::from_secs(5));
    assert_eq!(
        idle,
        Err(Box::new(Terminal::Error {
            message: "idle timeout".into(),
            retryable: true,
            usage: None,
        }))
    );
    // idle の残り（8 秒以下）が wall clock の残り（999 秒以下）より小さい。
    let wait = deadline_terminal(ago(1), ago(2), LONG, Duration::from_secs(10))
        .expect("neither limit is exceeded");
    assert!(wait <= Duration::from_secs(8), "{wait:?}");
    assert!(wait > Duration::ZERO);
    // wall clock の残りの方が小さいときはそちら。
    let wait = deadline_terminal(ago(98), Instant::now(), Duration::from_secs(100), LONG)
        .expect("neither limit is exceeded");
    assert!(wait <= Duration::from_secs(2), "{wait:?}");
}

/// 不変条件: harness が報告した実測の cost（aider の `Cost:` 行）は上書きしない。cost が無く model が単価表に
/// あれば埋め、model が無い・単価表に無いなら `None` のまま（不明を推測で埋めない）。claude_code・codex は
/// これまで常に上書きしていたので、実測を優先する規則はどの adapter の試験にも無かった。
#[test]
fn run_common_cost_estimate_keeps_reported_cost() {
    let reported = with_estimated_cost(usage(Some(0.5)), Some("claude-sonnet-5"));
    assert_eq!(reported.and_then(|u| u.cost_usd), Some(0.5));

    let estimated = with_estimated_cost(usage(None), Some("claude-sonnet-5"));
    let expected = usage(None).and_then(|u| task_core::estimate_cost_usd("claude-sonnet-5", &u));
    assert!(expected.is_some());
    assert_eq!(estimated.and_then(|u| u.cost_usd), expected);

    assert_eq!(with_estimated_cost(usage(None), None), usage(None));
    assert_eq!(
        with_estimated_cost(usage(None), Some("some-local-llm")),
        usage(None)
    );
    assert_eq!(with_estimated_cost(None, Some("claude-sonnet-5")), None);
}

/// 不変条件: `prepare_run` は前回の run が残した `artifacts/result.json`・`delegate.json` を消し（今回の結果と
/// 誤読しない。ADR-0006 D3）、`runs/<run_id>/request.json` を書く。browser の run は log を `/dev/null` にする。
/// これまで adapter ごとの写しで、codex・pi・aider には browser の log の分岐が無かった。
#[tokio::test]
async fn run_common_prepare_run_clears_previous_result_and_delegate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut req = request(dir.path());
    std::fs::create_dir_all(&req.artifacts_dir).expect("artifacts dir");
    let result = req.artifacts_dir.join("result.json");
    let delegate = req
        .artifacts_dir
        .join(crate::delegate_file::DELEGATE_FILE_NAME);
    std::fs::write(&result, r#"{"summary":"stale"}"#).expect("stale result");
    std::fs::write(&delegate, r#"{"tasks":[]}"#).expect("stale delegate");

    let paths = prepare_run(&req, "run-1", "stdout.log")
        .await
        .expect("prepare");
    let run_dir = dir.path().join("runs").join("run-1");
    assert_eq!(
        paths,
        RunPaths {
            run_dir: run_dir.clone(),
            stdout_log: run_dir.join("stdout.log"),
            stderr_log: run_dir.join("stderr.log"),
            result_path: result.clone(),
            artifacts_rel: req.artifacts_rel(),
        }
    );
    assert!(!result.exists());
    assert!(!delegate.exists());
    assert!(run_dir.join("request.json").is_file());

    req.context.browser = Some(crate::browser::BrowserContext {
        credential_used: false,
        approval_actions: Vec::new(),
        approved_operation: None,
        post_login: None,
        run: task_core::BrowserRun {
            task_id: req.task.id,
            run_id: "run-2".into(),
            session_id: "isolated-test".into(),
            state: task_core::BrowserRunState::Running,
            live_view_url: None,
            policy: None,
        },
        cli: dir.path().join("celeris-browser.py"),
    });
    let browser = prepare_run(&req, "run-2", "stdout.jsonl")
        .await
        .expect("prepare browser run");
    assert_eq!(browser.stdout_log, PathBuf::from("/dev/null"));
    assert_eq!(browser.stderr_log, PathBuf::from("/dev/null"));
}

/// 不変条件: `finish_run` は終端を `runs/<run_id>/result.json` に書いてから、`Error` と供給側失敗の組だけを
/// `Err(AdapterError)`（account の cooldown の入口）にする。供給側失敗の無い `Error` と他の終端は `Ok`。
/// 写しのどれかが書き出しを飛ばしても、adapter の試験は戻り値しか見ないので検出できなかった。
#[tokio::test]
async fn run_common_finish_run_maps_provider_failure_to_adapter_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = Terminal::Error {
        message: "401 Unauthorized".into(),
        retryable: true,
        usage: None,
    };
    let outcome = finish_run(
        dir.path(),
        error.clone(),
        Some(ProviderFailure::AuthFailed),
        ExitStatus::from_raw(1 << 8),
    )
    .await;
    assert!(
        matches!(&outcome, Err(AdapterError::AuthFailed(m)) if m == "401 Unauthorized"),
        "{outcome:?}"
    );
    let written = std::fs::read_to_string(dir.path().join("result.json")).expect("result.json");
    assert!(written.contains("401 Unauthorized"), "{written}");

    let outcome = finish_run(
        dir.path(),
        error.clone(),
        None,
        ExitStatus::from_raw(1 << 8),
    )
    .await
    .expect("an error without a provider failure stays a terminal");
    assert_eq!(outcome.terminal, error);
    assert_eq!(outcome.exit_code, Some(1));

    let done = Terminal::Done {
        summary: "ok".into(),
        evidence: Vec::new(),
        usage: None,
    };
    let outcome = finish_run(
        dir.path(),
        done.clone(),
        Some(ProviderFailure::Exhausted),
        ExitStatus::from_raw(0),
    )
    .await
    .expect("a provider failure only matters with an error terminal");
    assert_eq!(outcome.terminal, done);
    assert_eq!(outcome.exit_code, Some(0));
}
