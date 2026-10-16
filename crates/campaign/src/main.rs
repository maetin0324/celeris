use std::{path::PathBuf, process::ExitCode};

use celeris_campaign::{
    celeris_exec::{CelerisExecutor, check_template},
    checker,
    curl_transport::CurlTransport,
    dispatch::ArmDispatch,
    executor::MockExecutor,
    export,
    grade_runner::ProcessRunner,
    harness_exec::{ClaudeCodeBuilder, HarnessExecutor},
    ledger::Ledger,
    manifest::CampaignManifest,
    preflight::{self, Inputs, RealHost},
    runner,
};

const USAGE: &str = "usage: celeris-campaign <dry-run|validate|preflight|run|check|check-report|export|grade-out> --manifest <path> \
[--cases <grade-case dir>] \
[--input-manifest <path>] [--out <dir>] [--executor mock|harness|celeris] [--api-url <url> --token-env <ENV_NAME> [--task-template <json>]] [--dest <dir>] \
[--cases-root <dir>] [--grader-root <dir>] [--grade-case-root <dir>] [--a-program <path>] [--report <file>] [--repo-src <dir>] [--materials-src <dir>] [--kb-src <dir>]";

fn arg(args: &[String], name: &str) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

fn preflight_inputs(args: &[String], manifest_path: &std::path::Path) -> Inputs {
    let text = |n: &str| arg(args, n).map(|p| p.to_string_lossy().into_owned());
    let or = |n: &str, d: &str| arg(args, n).unwrap_or_else(|| PathBuf::from(d));
    Inputs {
        manifest_path: manifest_path.to_path_buf(),
        input_override: arg(args, "--input-manifest"),
        cases_root: or("--cases-root", "crates/campaign/cases"),
        grader_root: or("--grader-root", "crates/campaign/fixtures/grader-private"),
        grade_cases_root: or("--grade-case-root", "crates/campaign/fixtures/grade-cases"),
        a_program: or("--a-program", "claude"),
        api_url: text("--api-url"),
        token_env: text("--token-env"),
        task_template: arg(args, "--task-template"),
        repo_src: arg(args, "--repo-src"),
        materials_src: arg(args, "--materials-src"),
        kb_src: arg(args, "--kb-src"),
    }
}

fn run(args: Vec<String>) -> Result<String, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    if cmd == "grade" {
        return celeris_campaign::grader::cli(&args, &ProcessRunner::default());
    }
    let manifest_path = arg(&args, "--manifest").ok_or("--manifest <path> is required")?;
    let m = CampaignManifest::load(&manifest_path)?;
    // 絶対 path にする: A の harness は試行の workspace を cwd にして動くので、相対 path の状態 dir は壊れる。
    let out = || -> Result<PathBuf, String> {
        let p = arg(&args, "--out").ok_or("--out <dir> is required")?;
        std::path::absolute(&p).map_err(|e| format!("--out {}: {e}", p.display()))
    };
    match cmd {
        "dry-run" => {
            let input = m.load_input(&manifest_path, arg(&args, "--input-manifest").as_deref())?;
            let v = celeris_campaign::dryrun::estimate(&m, &input)?;
            serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
        }
        "validate" => {
            m.validate()?;
            Ok(m.compute_sha256()?)
        }
        "preflight" => {
            let rep = preflight::check(&preflight_inputs(&args, &manifest_path), &RealHost);
            let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
            if rep.ok { Ok(s) } else { Err(s) }
        }
        "run" => {
            let which =
                arg(&args, "--executor").ok_or("--executor mock|harness|celeris is required")?;
            let which = which.to_string_lossy().into_owned();
            // 実行（有料）経路は preflight を通ってからだけ始める。mock は無課金なので対象外。
            if matches!(which.as_str(), "harness" | "celeris") {
                preflight::gate(&preflight_inputs(&args, &manifest_path), &RealHost)?;
            }
            let input = m.load_input(&manifest_path, arg(&args, "--input-manifest").as_deref())?;
            let out_dir = out()?;
            let opts = runner::RunOptions {
                out: out_dir.clone(),
                repo_src: arg(&args, "--repo-src"),
                materials_src: arg(&args, "--materials-src"),
                kb_src: arg(&args, "--kb-src"),
            };
            let rep = match which.as_str() {
                "mock" => runner::run(&m, &input, &MockExecutor::seeded(), &opts)?,
                "harness" => runner::run(
                    &m,
                    &input,
                    &HarnessExecutor::new(
                        out_dir.join("harness-state"),
                        Box::new(ClaudeCodeBuilder::default()),
                    ),
                    &opts,
                )?,
                "celeris" => {
                    // 秘密の値は受け取らない: token は環境変数の「名前」だけ。
                    let url = arg(&args, "--api-url").ok_or("--api-url is required for celeris")?;
                    let tok = arg(&args, "--token-env")
                        .ok_or("--token-env <ENV_NAME> is required for celeris")?;
                    let template = match arg(&args, "--task-template") {
                        Some(p) => {
                            let v: serde_json::Value = serde_json::from_slice(
                                &std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?,
                            )
                            .map_err(|e| format!("{}: {e}", p.display()))?;
                            let t = v
                                .as_object()
                                .cloned()
                                .ok_or("--task-template must be a JSON object")?;
                            check_template(&t)?;
                            t
                        }
                        None => serde_json::Map::new(),
                    };
                    let transport =
                        CurlTransport::new(&url.to_string_lossy(), &tok.to_string_lossy())?;
                    let bc = CelerisExecutor::new(
                        transport,
                        template,
                        m.termination.trial_timeout_secs,
                        out_dir.join("cluster-cancel.jsonl"),
                    );
                    // A 腕を含む manifest でも同じ入口で走る。A は既存ハーネス（後付け制限なし）。
                    let a = HarnessExecutor::new(
                        out_dir.join("harness-state"),
                        Box::new(ClaudeCodeBuilder::default()),
                    );
                    runner::run(&m, &input, &ArmDispatch { a, bc }, &opts)?
                }
                other => {
                    return Err(format!(
                        "unknown executor {other:?}: expected mock|harness|celeris"
                    ));
                }
            };
            let mut by_state = std::collections::BTreeMap::<String, u64>::new();
            let mut rows = 0u64;
            for r in Ledger::open(&out_dir)?.rows() {
                rows += 1;
                let st = serde_json::to_value(r.state)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default();
                *by_state.entry(st).or_default() += 1;
            }
            Ok(format!(
                "{{\"executed\":{},\"resumed_skipped\":{},\"budget_cancelled\":{},\"blocked\":{},\"waiting\":{},\"tokens_total\":{},\"tokens_missing\":{},\"case_budget_cancelled\":{},\"arm_budget_cancelled\":{},\"max_overshoot_tokens\":{},\"max_overshoot_wall_secs\":{},\"parallel_deferred\":{},\"no_artifact\":{},\"executor_errors\":{},\"budget_stopped\":{},\"ledger\":{{\"rows\":{},\"by_state\":{}}}}}",
                rep.executed,
                rep.resumed_skipped,
                rep.budget_cancelled,
                rep.blocked,
                rep.waiting,
                rep.tokens_total,
                rep.tokens_missing,
                rep.case_budget_cancelled,
                rep.arm_budget_cancelled,
                rep.max_overshoot_tokens,
                rep.max_overshoot_wall_secs,
                rep.parallel_deferred,
                rep.no_artifact,
                rep.executor_errors,
                rep.budget_stopped,
                rows,
                serde_json::to_string(&by_state).map_err(|e| e.to_string())?
            ))
        }
        "check" => {
            let rep = checker::check(&m, &out()?);
            let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
            if rep.ok { Ok(s) } else { Err(s) }
        }
        "check-report" => {
            let rp = arg(&args, "--report").ok_or("--report <file> is required")?;
            let text =
                std::fs::read_to_string(&rp).map_err(|e| format!("{}: {e}", rp.display()))?;
            let v = celeris_campaign::report::check_report(&m, &out()?, &text);
            if v.is_empty() {
                Ok("{\"ok\":true}".into())
            } else {
                Err(v.join("\n"))
            }
        }
        "grade-out" => {
            let cases = arg(&args, "--cases").ok_or("--cases <grade-case dir> is required")?;
            let v = celeris_campaign::grade_out::grade_out(
                &m,
                &out()?,
                &cases,
                &ProcessRunner::default(),
            )?;
            let s = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
            if v["ok"] == true { Ok(s) } else { Err(s) }
        }
        "export" => {
            let out = out()?;
            let dest = arg(&args, "--dest").unwrap_or_else(|| out.join("export"));
            let t = export::export(&m, &out, &dest)?;
            serde_json::to_string_pretty(&t).map_err(|e| e.to_string())
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
