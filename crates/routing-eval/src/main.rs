use std::{path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        help();
        return ExitCode::SUCCESS;
    };
    if command == "--help" || command == "-h" {
        help();
        return ExitCode::SUCCESS;
    }
    if command == "--version" || command == "-V" {
        println!("celeris-routing-eval {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let values: Vec<String> = args.collect();
    let result = match command.as_str() {
        "manifest" => manifest(&values),
        "recheck" => routing_eval::recheck::run(&values),
        "dry-run" => routing_eval::dryrun::run(),
        "run" => routing_eval::run::run(),
        "aggregate" => routing_eval::aggregate::run(),
        "export" => routing_eval::export::run(),
        "stats" => routing_eval::stats::run(),
        _ => Err(format!("unknown subcommand: {command}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn manifest(args: &[String]) -> Result<(), String> {
    let value = |flag: &str, default: Option<&str>| -> Result<String, String> {
        args.iter()
            .position(|x| x == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| default.map(str::to_owned))
            .ok_or_else(|| format!("missing required option {flag}"))
    };
    let db = PathBuf::from(value("--db", None)?);
    let out = PathBuf::from(value("--out", None)?);
    let cutoff = value("--cutoff", Some("2026-10-05"))?;
    let seed = value("--seed", Some("0"))?
        .parse::<u64>()
        .map_err(|e| e.to_string())?;
    let version = value("--version", Some("1"))?;
    let m = routing_eval::manifest::create(&db, &out, &cutoff, seed, &version)?;
    println!(
        "manifest={} problems={} sha256={}",
        out.join("manifest.json").display(),
        m.problems.len(),
        m.sha256
    );
    Ok(())
}

fn help() {
    println!(
        "celeris-routing-eval — deterministic routing evaluation tools\n\nUSAGE:\n  celeris-routing-eval <COMMAND> [OPTIONS]\n\nCOMMANDS:\n  manifest   Freeze the eligible evaluation problem set\n  recheck    Re-run deterministic checks against base and head commits\n  dry-run    Estimate calls, cost, and quota use\n  run        Dispatch evaluation runs through Celeris\n  aggregate  Build interval-based evidence from completed runs\n  export     Export results joined with task features\n  stats      Compute evaluation statistics\n\nmanifest OPTIONS:\n  --db <path>       Read-only Celeris database\n  --out <dir>       Output directory\n  --cutoff <date>   Dev/held-out cutoff (default 2026-10-05)\n  --seed <n>        Deterministic stratification seed\n  --version <id>    Manifest version (default 1)\n\nrun OPTIONS:\n  --api <url>             Celeris API base URL\n  --manifest <file>       Frozen manifest JSON\n  --candidates <list>     Candidate model IDs, comma separated\n  --token-file <path>     Optional bearer token file\n  --repo <path>           Repository path (default .)\n  --wait                  Poll until every trial is terminal\n  --poll-interval-ms <n>  Poll interval (default 1000)\n  --out <file>            Write summary JSON to a file"
    );
}
