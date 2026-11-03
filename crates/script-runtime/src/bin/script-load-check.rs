//! bundle の manifest を検証して（hash・Host API・schema に加え、登録と同じ manifest の検証
//! `generation::validate_manifest`: 既知の領域だけ・capability の許可表・checkpoint の版・selftest の宣言）load し、試験用の領域を 1 回呼んで結果の JSON を stdout へ出す。失敗は非 0。
//!
//! 使い方: `cargo run -p celeris-script --bin script-load-check -- --bundle-dir <dir>`

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use celeris_script::generation::validate_manifest;
use celeris_script::manifest::load_bundle_dir;
use celeris_script::selftest::{SELFTEST_DOMAIN, check_declared, check_output, selftest_call};
use celeris_script::{ScriptRuntimeHandle, dto::HOST_API_VERSION};
use serde_json::json;

const CALL_TIMEOUT: Duration = Duration::from_secs(30);

fn parse_args() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--bundle-dir"), Some(dir), None) => Ok(PathBuf::from(dir)),
        _ => Err("usage: script-load-check --bundle-dir <dir>".into()),
    }
}

fn run() -> Result<String, String> {
    let dir = parse_args()?;
    let bundle = load_bundle_dir(&dir).map_err(|e| e.to_string())?;
    check_declared(&bundle.manifest)?;
    validate_manifest(&bundle.manifest).map_err(|e| e.to_string())?;
    let runtime = ScriptRuntimeHandle::start(bundle.source()).map_err(|e| e.to_string())?;
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| e.to_string())?;
    let result = tokio_rt.block_on(runtime.call(selftest_call(CALL_TIMEOUT)));
    runtime.shutdown();
    let reply = result.map_err(|e| e.to_string())?;
    check_output(&reply.output)?;
    let report = json!({
        "bundle_sha256": bundle.manifest.bundle_sha256,
        "host_api": HOST_API_VERSION,
        "domain": SELFTEST_DOMAIN,
        "domains": bundle.manifest.domains,
        "generation": reply.generation,
        "output": reply.output,
    });
    serde_json::to_string(&report).map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    match run() {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("script-load-check: {err}");
            ExitCode::FAILURE
        }
    }
}
