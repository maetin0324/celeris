use std::{path::PathBuf, process::ExitCode};

use celeris_campaign::{
    celeris_exec::Transport,
    celeris_exec::{CelerisExecutor, check_template},
    checker,
    curl_transport::CurlTransport,
    dispatch::ArmDispatch,
    executor::{Executor, MockExecutor},
    export,
    grade_runner::ProcessRunner,
    harness_exec::{BudgetGuard, ClaudeCodeBuilder, HarnessExecutor, Sandbox},
    ledger::Ledger,
    manifest::CampaignManifest,
    preflight::{self, Inputs, RealHost},
    runner,
    watch::{self, SystemClock, WatchOptions},
};

const USAGE: &str = "usage: celeris-campaign <dry-run|validate|env-lock|config-check|env-check|preflight|cap-probe|cap-worker|run|watch|check|check-report|export|final-accounting|grade-out|usage> --manifest <path> \
[--interval-secs N] [--max-iterations M] [--cases <grade-case dir> [--initial-trees <dir>]] \
[--input-manifest <path>] [--out <dir>] [--executor mock|harness|celeris] [--api-url <url> --token-env <ENV_NAME> [--task-template <json>]] [--dest <dir>] \
[--cases-root <dir>] [--grader-root <dir>] [--grade-case-root <dir>] [--a-program <path>] [--report <file>] [--measurement-id <id>] [--repo-src <dir>] [--materials-src <dir>] [--kb-src <dir>] [--inputs-src <dir>] [--records <records.jsonl>] [--cos-not-called <basis>] [--quota <usage.json>] [--a-account <account_id>] [--measure-config <toml> --normal-config <toml> --canaries <json>] [--max-7d <ratio>] [--max-7d-codex <ratio>]";

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
        inputs_src: arg(args, "--inputs-src"),
        a_account: text("--a-account"),
        measure: arg(args, "--measure-config").map(|config| preflight::MeasureEnv {
            config,
            normal_config: arg(args, "--normal-config").unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                    .join(".config/celeris/config.toml")
            }),
            canaries: or("--canaries", "canaries.json"),
            out: arg(args, "--out"),
            daemon_pid: text("--daemon-pid").and_then(|p| p.parse().ok()),
        }),
        measurement_id: text("--measurement-id"),
        cap_env: arg(args, "--cap-env"),
    }
}

/// `--records` があれば最終集計を作り、整合を検査する（不整合は Err）。無ければ None。
fn accounting(
    args: &[String],
    m: &CampaignManifest,
    out: &std::path::Path,
) -> Result<Option<serde_json::Value>, String> {
    let Some(rp) = arg(args, "--records") else {
        return Ok(None);
    };
    let records = std::fs::read_to_string(&rp).map_err(|e| format!("{}: {e}", rp.display()))?;
    let quota = match arg(args, "--quota") {
        Some(p) => Some(
            serde_json::from_slice(
                &std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?,
            )
            .map_err(|e| format!("{}: {e}", p.display()))?,
        ),
        None => None,
    };
    let cos = arg(args, "--cos-not-called").map(|p| p.to_string_lossy().into_owned());
    let a = celeris_campaign::final_accounting::compute(
        m,
        out,
        &celeris_campaign::final_accounting::FinalInputs {
            records_jsonl: &records,
            cos_not_called_basis: cos.as_deref(),
            quota,
        },
    )?;
    let v = celeris_campaign::final_accounting::recheck(&a);
    if v.is_empty() {
        Ok(Some(a))
    } else {
        Err(v.join("\n"))
    }
}

/// locked な out（`lock.json` あり）では、cases root から作り直した期待 lock を返す。
/// check / grade-out / export が保存 lock と突き合わせる（保存 lock の丸ごと差替えを見逃さない）。
/// lock の無い legacy out は None（従来どおり。本測定の根拠にならない）。
fn cli_expected_lock(
    args: &[String],
    manifest_path: &std::path::Path,
    m: &CampaignManifest,
    out: &std::path::Path,
) -> Result<Option<celeris_campaign::lock::CampaignLock>, String> {
    if !out.join(celeris_campaign::lock_exec::LOCK_FILE).exists() {
        return Ok(None);
    }
    preflight::expected_lock(&preflight_inputs(args, manifest_path), m).map(Some)
}

fn transport(args: &[String]) -> Result<CurlTransport, String> {
    let url = arg(args, "--api-url").ok_or("--api-url is required")?;
    let tok = arg(args, "--token-env").ok_or("--token-env <ENV_NAME> is required")?;
    CurlTransport::new(&url.to_string_lossy(), &tok.to_string_lossy())
}

fn get_accounts(args: &[String]) -> Result<serde_json::Value, String> {
    let r = transport(args)?.request("GET", "/api/v1/llm/accounts", None)?;
    if (200..300).contains(&r.status) {
        Ok(r.body)
    } else {
        Err(format!(
            "GET /api/v1/llm/accounts: HTTP {} {}",
            r.status, r.body
        ))
    }
}

/// A の実行器: Celeris 管理アカウントで動き、bwrap で隔離され、token 予算を実行中に強制される。
fn a_executor(
    args: &[String],
    m: &CampaignManifest,
    out_dir: &std::path::Path,
    program: &preflight::ResolvedProgram,
) -> Result<HarnessExecutor, String> {
    let id = arg(args, "--a-account")
        .ok_or("--a-account <account_id> is required (A runs under a Celeris-managed account)")?
        .to_string_lossy()
        .into_owned();
    let dir = preflight::find_a_account(&get_accounts(args)?, &id)?;
    Ok(HarnessExecutor::new(
        out_dir.join("harness-state"),
        Box::new(ClaudeCodeBuilder::from_resolved(program)),
    )
    .with_sandbox(Sandbox {
        account_id: id,
        auth_dir: dir,
        state_root: out_dir.join("trial-state"),
        toolchain: celeris_campaign::cap_probe::load_toolchain(args)?,
    })
    .with_budget_guard(BudgetGuard {
        basis: m.budget.token_basis,
        interval: std::time::Duration::from_secs(1),
    }))
}

/// `cap-probe --case <id> --arm a|b|c --cap-env <json> [--dry-argv]`（b/c は `--manifest --measure-config --daemon-pid --api-url --token-env` も要る）。LLM も job も使わない。
fn cap_probe_cli(args: &[String]) -> Result<String, String> {
    use celeris_campaign::{cap_probe, cases::CaseSet, manifest::ConfigId};
    let text = |n: &str| arg(args, n).map(|p| p.to_string_lossy().into_owned());
    let id = text("--case").ok_or("--case <id> is required")?;
    let arm = match text("--arm").as_deref() {
        Some("a") => ConfigId::A,
        Some("b") => ConfigId::B,
        Some("c") => ConfigId::C,
        _ => return Err("--arm a|b|c is required".into()),
    };
    let env =
        cap_probe::CapEnv::load(&arg(args, "--cap-env").ok_or("--cap-env <json> is required")?)?;
    let set = CaseSet::load(
        &arg(args, "--cases-root").unwrap_or_else(|| PathBuf::from("crates/campaign/cases")),
        &arg(args, "--grader-root")
            .unwrap_or_else(|| PathBuf::from("crates/campaign/fixtures/grader-private")),
    )?;
    let case = &set
        .get(&id)
        .ok_or_else(|| format!("{id}: no such case"))?
        .case;
    let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let ws = arg(args, "--workspace").unwrap_or_else(|| tmp.path().join("ws"));
    let cfg = arg(args, "--config-dir").unwrap_or_else(|| tmp.path().join("cfg"));
    let _ = std::fs::create_dir_all(&ws);
    cap_probe::prepare_state_dir(&cfg)?;
    let sb = cap_probe::sandbox_for(
        &env,
        arm,
        text("--a-account").as_deref().unwrap_or("probe"),
        &cfg,
    );
    if args.iter().any(|a| a == "--dry-argv") {
        let v: Vec<_> = cap_probe::dry_argv(&sb, &ws, case)
            .into_iter()
            .map(|(id, r)| match r {
                Ok(a) => serde_json::json!({"id": id, "argv": a}),
                Err(e) => serde_json::json!({"id": id, "error": e}),
            })
            .collect();
        return serde_json::to_string_pretty(&v).map_err(|e| e.to_string());
    }
    let rep = if arm == ConfigId::A {
        cap_probe::probe(
            &sb,
            arm,
            &ws,
            case,
            &cap_probe::ProcessProbeRunner {
                timeout: std::time::Duration::from_secs(300),
            },
        )
    } else {
        // B/C は測定専用 daemon の process から worker 環境を取り（この CLI の環境は使わない）、
        // worker の実起動関数でその環境の子 process に走らせる。検査先は identity 照合に通った daemon だけ。
        let manifest_path =
            arg(args, "--manifest").ok_or("--manifest <path> is required for b/c")?;
        let m = CampaignManifest::load(&manifest_path)?;
        let pi = preflight_inputs(args, &manifest_path);
        let (me, url, tok) = match (&pi.measure, &pi.api_url, &pi.token_env) {
            (Some(me), Some(u), Some(t)) => (me, u, t),
            _ => {
                return Err(
                    "b/c need --measure-config, --daemon-pid, --api-url and --token-env: the probe target is the measurement daemon, not this CLI"
                        .into(),
                );
            }
        };
        let harnesses = m
            .configs
            .iter()
            .find(|x| x.id == arm)
            .map(|x| x.worker_harnesses())
            .ok_or("the manifest has no such arm")?;
        // 投入の前に、検査分野が無 LLM の検査 harness に結ばれていることを宣言から確かめる（宣言が無ければ何も投入しない）。
        let problems = cap_probe::declaration_problems_of_file(&me.config, &harnesses);
        if !problems.is_empty() {
            return Err(format!(
                "env_failure: B/C probe not submitted: {}",
                problems.join("; ")
            ));
        }
        let target = celeris_campaign::env_check::verify(me, &m, url, tok, &RealHost)
            .map_err(|es| format!("not the measurement daemon: {}", es.join("; ")))?;
        let t = celeris_campaign::curl_transport::CurlTransport::new(url, tok)?;
        cap_probe::probe_worker(&env, &target, arm, case, &harnesses, &|spec| {
            cap_probe::run_in_daemon(
                &t,
                spec,
                std::time::Duration::from_secs(300),
                std::time::Duration::from_millis(500),
            )
        })
    };
    let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
    if rep.ok { Ok(s) } else { Err(s) }
}

fn run(args: Vec<String>) -> Result<String, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    if cmd == "grade" {
        return celeris_campaign::grader::cli(&args, &ProcessRunner::default());
    }
    if cmd == "cap-probe" {
        return cap_probe_cli(&args);
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
        "env-lock" => {
            use celeris_campaign::env_lock;
            let path_var = std::env::var_os("PATH");
            // A が有効なときだけ A の実行ファイルを固定する（B/C のみの測定は A を探さない）。
            let a_program = if m.is_active(celeris_campaign::manifest::ConfigId::A) {
                let p = arg(&args, "--a-program").unwrap_or_else(|| PathBuf::from("claude"));
                Some(env_lock::pin_program(&p, path_var.as_deref())?)
            } else {
                None
            };
            let mut worker_programs = Vec::new();
            for w in args
                .iter()
                .enumerate()
                .filter(|(_, a)| *a == "--worker-program")
                .filter_map(|(i, _)| args.get(i + 1))
            {
                let (name, p) = w
                    .split_once('=')
                    .ok_or("--worker-program expects <name>=<program>")?;
                worker_programs.push(env_lock::WorkerProgram {
                    name: name.into(),
                    pin: env_lock::pin_program(std::path::Path::new(p), path_var.as_deref())?,
                });
            }
            // B/C の候補集合・階層は実 daemon の `GET /api/v1/llm/models`（正式 DTO）から取る。空は固定しない。
            let url = arg(&args, "--api-url").ok_or("--api-url is required")?;
            let tok = arg(&args, "--token-env").ok_or("--token-env is required")?;
            let catalog =
                env_lock::fetch_catalog(&RealHost, &url.to_string_lossy(), &tok.to_string_lossy())?;
            let cap_env = match arg(&args, "--cap-env") {
                Some(p) => Some(celeris_campaign::cap_probe::CapEnv::load(&p)?.pin()?),
                None => None,
            };
            let obs = env_lock::Observed {
                a_program,
                worker_programs,
                catalog,
                cap_env,
            };
            let verify = args.iter().any(|a| a == "--verify");
            let o = env_lock::establish(&m, &obs, &out()?, verify)?;
            Ok(format!("{{\"env_lock\":\"{o:?}\"}}").to_lowercase())
        }
        "config-check" => {
            // 起動前の検査（daemon に接続しない・token を読まない）: 測定 config と通常運用の分離・canary 定義。
            let pi = preflight_inputs(&args, &manifest_path);
            let me = pi
                .measure
                .as_ref()
                .ok_or("--measure-config <toml> (and --normal-config, --canaries) is required")?;
            let mut missing = celeris_campaign::measure_env::check_config(
                &me.config,
                &me.normal_config,
                pi.kb_src.as_deref(),
            );
            if let Err(e) = celeris_campaign::measure_env::Canaries::load(&me.canaries) {
                missing.push(e);
            }
            let s = serde_json::to_string_pretty(&serde_json::json!({
                "schema": "celeris.campaign-config-check/1",
                "ok": missing.is_empty(),
                "paid_calls_made": 0,
                "missing": missing,
            }))
            .map_err(|e| e.to_string())?;
            if missing.is_empty() { Ok(s) } else { Err(s) }
        }
        "env-check" => {
            let pi = preflight_inputs(&args, &manifest_path);
            let me = pi
                .measure
                .as_ref()
                .ok_or("--measure-config <toml> (and --normal-config, --canaries) is required")?;
            let url = pi.api_url.as_deref().ok_or("--api-url is required")?;
            let tok = pi.token_env.as_deref().ok_or("--token-env is required")?;
            let rep =
                celeris_campaign::env_check::run(me, &m, pi.kb_src.as_deref(), url, tok, &RealHost);
            let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
            if rep.ok { Ok(s) } else { Err(s) }
        }
        "preflight" => {
            let rep = preflight::check(&preflight_inputs(&args, &manifest_path), &RealHost);
            let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
            if rep.ok { Ok(s) } else { Err(s) }
        }
        "run" | "watch" => {
            let which =
                arg(&args, "--executor").ok_or("--executor mock|harness|celeris is required")?;
            let which = which.to_string_lossy().into_owned();
            // 実行（有料）経路は preflight を通ってからだけ始める。mock は無課金なので対象外。
            // A はここで検査した実行ファイル・版をそのまま起動する（PATH を引き直さない）。
            let a_program = if matches!(which.as_str(), "harness" | "celeris") {
                preflight::gate(&preflight_inputs(&args, &manifest_path), &RealHost)?.a_program
            } else {
                None
            };
            let a_program = || {
                a_program
                    .as_ref()
                    .ok_or_else(|| "internal: preflight did not resolve the A program".to_string())
            };
            let input = m.load_input(&manifest_path, arg(&args, "--input-manifest").as_deref())?;
            let out_dir = out()?;
            // 外部入力は case.json の宣言から引く（実行経路だけ。mock は case を読まない）。
            let case_inputs = if matches!(which.as_str(), "harness" | "celeris") {
                let pi = preflight_inputs(&args, &manifest_path);
                let set = celeris_campaign::cases::CaseSet::load(&pi.cases_root, &pi.grader_root)?;
                celeris_campaign::inputs::for_problems(&set, &m.problems)
            } else {
                Default::default()
            };
            let opts = runner::RunOptions {
                out: out_dir.clone(),
                repo_src: arg(&args, "--repo-src"),
                materials_src: arg(&args, "--materials-src"),
                kb_src: arg(&args, "--kb-src"),
                inputs_src: arg(&args, "--inputs-src"),
                case_inputs,
                global_ledger: arg(&args, "--global-ledger"),
                // 実行（有料）経路は、preflight・runner・再開で同じ期待 lock を使う。
                lock: if matches!(which.as_str(), "harness" | "celeris") {
                    Some(preflight::expected_lock(
                        &preflight_inputs(&args, &manifest_path),
                        &m,
                    )?)
                } else {
                    None
                },
            };
            // watch は同じ走査を台帳が閉じるまで繰り返す（再起動しても台帳から再開する）。
            let wopts = (cmd == "watch")
                .then(|| -> Result<WatchOptions, String> {
                    let n = |name: &str, d: u64| -> Result<u64, String> {
                        match arg(&args, name) {
                            None => Ok(d),
                            Some(v) => v
                                .to_string_lossy()
                                .parse::<u64>()
                                .map_err(|e| format!("{name}: {e}")),
                        }
                    };
                    Ok(WatchOptions {
                        interval_secs: n("--interval-secs", 60)?,
                        max_iterations: u32::try_from(n("--max-iterations", 1440)?)
                            .map_err(|e| format!("--max-iterations: {e}"))?,
                    })
                })
                .transpose()?;
            let watched = std::cell::Cell::new(None);
            let drive = |e: &dyn Executor| -> Result<runner::RunReport, String> {
                match wopts {
                    None => runner::run(&m, &input, e, &opts),
                    Some(w) => {
                        let r = watch::watch(&m, &input, e, &opts, w, &SystemClock)?;
                        watched.set(Some((r.iterations, r.end)));
                        Ok(r.last)
                    }
                }
            };
            let rep = match which.as_str() {
                "mock" => drive(&MockExecutor::seeded())?,
                "harness" => drive(&a_executor(&args, &m, &out_dir, a_program()?)?)?,
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
                    let a = a_executor(&args, &m, &out_dir, a_program()?)?;
                    drive(&ArmDispatch { a, bc })?
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
                "{{\"executed\":{},\"resumed_skipped\":{},\"budget_cancelled\":{},\"blocked\":{},\"waiting\":{},\"tokens_total\":{},\"tokens_missing\":{},\"case_budget_cancelled\":{},\"arm_budget_cancelled\":{},\"max_overshoot_tokens\":{},\"max_overshoot_wall_secs\":{},\"parallel_deferred\":{},\"no_artifact\":{},\"executor_errors\":{},\"budget_stopped\":{},{}\"ledger\":{{\"rows\":{},\"by_state\":{}}}}}",
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
                match watched.get() {
                    Some((n, end)) => format!(
                        "\"watch\":{{\"iterations\":{n},\"end\":\"{}\"}},",
                        end.as_str()
                    ),
                    None => String::new(),
                },
                rows,
                serde_json::to_string(&by_state).map_err(|e| e.to_string())?
            ))
        }
        "check" => {
            let expected = cli_expected_lock(&args, &manifest_path, &m, &out()?)?;
            let rep = checker::check_expected(&m, &out()?, expected.as_ref());
            let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
            if rep.ok { Ok(s) } else { Err(s) }
        }
        "check-report" => {
            let rp = arg(&args, "--report").ok_or("--report <file> is required")?;
            let text =
                std::fs::read_to_string(&rp).map_err(|e| format!("{}: {e}", rp.display()))?;
            let out = out()?;
            let mut v = celeris_campaign::report::check_report(&m, &out, &text);
            if let Some(root) = arg(&args, "--cases-root") {
                match celeris_campaign::report::campaign_scope(&m, &root) {
                    Ok((j, n)) => {
                        v.extend(celeris_campaign::report::check_scope_claims(&j, &n, &text))
                    }
                    Err(e) => v.push(e),
                }
            }
            match accounting(&args, &m, &out) {
                Ok(Some(a)) => v.extend(celeris_campaign::report::check_consumption_claims(
                    &a, &text,
                )),
                Ok(None) => {}
                Err(e) => v.push(e),
            }
            if v.is_empty() {
                Ok("{\"ok\":true}".into())
            } else {
                Err(v.join("\n"))
            }
        }
        "grade-out" => {
            let cases = arg(&args, "--cases").ok_or("--cases <grade-case dir> is required")?;
            let expected = cli_expected_lock(&args, &manifest_path, &m, &out()?)?;
            let initial_trees = arg(&args, "--initial-trees");
            let v = celeris_campaign::grade_out::grade_out_with(
                &m,
                &out()?,
                &cases,
                &ProcessRunner::default(),
                expected.as_ref(),
                initial_trees.as_deref(),
            )?;
            let s = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
            if v["ok"] == true { Ok(s) } else { Err(s) }
        }
        "usage" => {
            // A が有効計画に無ければ要らない（report が summary の cells で判断する）。
            let acct = arg(&args, "--a-account")
                .map(|a| a.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ratio = |n: &str, d: f64| -> Result<f64, String> {
                match arg(&args, n) {
                    None => Ok(d),
                    Some(v) => v
                        .to_string_lossy()
                        .parse::<f64>()
                        .map_err(|e| format!("{n}: {e}")),
                }
            };
            let summary: serde_json::Value = serde_json::from_slice(
                &std::fs::read(out()?.join(celeris_campaign::record::SUMMARY_FILE))
                    .map_err(|e| format!("summary.json: {e}"))?,
            )
            .map_err(|e| format!("summary.json: {e}"))?;
            let (rep, violations) = celeris_campaign::usage::report(
                &get_accounts(&args)?,
                &summary,
                &acct,
                ratio("--max-7d", celeris_campaign::usage::DEFAULT_MAX_7D_CLAUDE)?,
                ratio(
                    "--max-7d-codex",
                    celeris_campaign::usage::DEFAULT_MAX_7D_CODEX,
                )?,
            );
            let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
            if violations.is_empty() { Ok(s) } else { Err(s) }
        }
        "final-accounting" => {
            let out = out()?;
            let a = accounting(&args, &m, &out)?.ok_or("--records <records.jsonl> is required")?;
            serde_json::to_string_pretty(&a).map_err(|e| e.to_string())
        }
        "export" => {
            let out = out()?;
            let dest = arg(&args, "--dest").unwrap_or_else(|| out.join("export"));
            let expected = cli_expected_lock(&args, &manifest_path, &m, &out)?;
            let acct = accounting(&args, &m, &out)?;
            let t = export::export_full(&m, &out, &dest, expected.as_ref(), acct.as_ref())?;
            serde_json::to_string_pretty(&t).map_err(|e| e.to_string())
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    // 測定専用 daemon が `[adapters.fake] command` として起こす無 LLM の検査 worker（stdin の run → stdout の done 1 行）。
    if std::env::args().nth(1).as_deref() == Some("cap-worker") {
        return match celeris_campaign::cap_probe::cap_worker_main() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(1)
            }
        };
    }
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
