use std::{path::PathBuf, process::ExitCode};

use celeris_campaign::manifest::CampaignManifest;

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
        _ => Err("usage: celeris-campaign <dry-run|validate> --manifest <path> [--input-manifest <path>]".into()),
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
