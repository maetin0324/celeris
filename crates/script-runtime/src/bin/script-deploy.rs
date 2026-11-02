//! bundle（`bundle.mjs` と `manifest.json`）を検証し、`<state_dir>/script-bundles/<bundle_sha256>/` へ配備する。
//! 成功すると `bundle_sha256` を stdout に 1 行出す。失敗は非 0（stdout には何も出さない）。
//!
//! 同じ hash の配備が在れば検証して再利用する。契約（`host_api`・`schema_sha256`・`domains`・`capabilities`・
//! `checkpoint_versions`）が食い違えば上書きせず拒否する（境界 ADR 2026-11-02「TS の置き場と配備先」）。
//!
//! 使い方: `cargo run -p celeris-script --bin script-deploy -- --dist <dir> --state-dir <dir>`

use std::path::PathBuf;
use std::process::ExitCode;

use celeris_script::manifest::deploy_bundle;

const USAGE: &str = "usage: script-deploy --dist <dir> --state-dir <dir>";

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<(PathBuf, PathBuf), String> {
    let mut dist = None;
    let mut state_dir = None;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let slot = match flag.as_str() {
            "--dist" => &mut dist,
            "--state-dir" => &mut state_dir,
            _ => return Err(USAGE.into()),
        };
        let value = args.next().ok_or_else(|| USAGE.to_owned())?;
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(USAGE.into());
        }
    }
    match (dist, state_dir) {
        (Some(dist), Some(state_dir)) => Ok((dist, state_dir)),
        _ => Err(USAGE.into()),
    }
}

fn run() -> Result<String, String> {
    let (dist, state_dir) = parse_args(std::env::args().skip(1))?;
    let bundle = deploy_bundle(&dist, &state_dir).map_err(|e| e.to_string())?;
    Ok(bundle.manifest.bundle_sha256)
}

fn main() -> ExitCode {
    match run() {
        Ok(sha) => {
            println!("{sha}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("script-deploy: {err}");
            ExitCode::FAILURE
        }
    }
}
