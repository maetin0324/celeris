use std::{path::PathBuf, process::ExitCode};

use celeris_campaign::{
    checker, executor::MockExecutor, export, manifest::CampaignManifest, runner,
};

const USAGE: &str = "usage: celeris-campaign <dry-run|validate|run|check|check-report|export> --manifest <path> \
[--input-manifest <path>] [--out <dir>] [--executor mock] [--dest <dir>] \
[--report <file>] [--repo-src <dir>] [--materials-src <dir>] [--kb-src <dir>]";

fn arg(args: &[String], name: &str) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

fn run(args: Vec<String>) -> Result<String, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let manifest_path = arg(&args, "--manifest").ok_or("--manifest <path> is required")?;
    let m = CampaignManifest::load(&manifest_path)?;
    let out = || arg(&args, "--out").ok_or("--out <dir> is required".to_string());
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
        "run" => {
            // 実 LLM・外部 network を呼ぶ実行器はこの crate にない。mock のみ。
            let which = arg(&args, "--executor").ok_or("--executor mock is required")?;
            if which.to_string_lossy() != "mock" {
                return Err(format!(
                    "unknown executor {which:?}: only `mock` is available"
                ));
            }
            let input = m.load_input(&manifest_path, arg(&args, "--input-manifest").as_deref())?;
            let opts = runner::RunOptions {
                out: out()?,
                repo_src: arg(&args, "--repo-src"),
                materials_src: arg(&args, "--materials-src"),
                kb_src: arg(&args, "--kb-src"),
            };
            let rep = runner::run(&m, &input, &MockExecutor::seeded(), &opts)?;
            Ok(format!(
                "{{\"executed\":{},\"resumed_skipped\":{},\"budget_cancelled\":{},\"blocked\":{},\"waiting\":{},\"tokens_total\":{},\"budget_stopped\":{}}}",
                rep.executed,
                rep.resumed_skipped,
                rep.budget_cancelled,
                rep.blocked,
                rep.waiting,
                rep.tokens_total,
                rep.budget_stopped
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
