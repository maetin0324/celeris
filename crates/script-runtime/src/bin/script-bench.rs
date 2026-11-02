//! warm の判定 1 回の遅延と isolate 1 個・2 個の VmRSS 増分を測り、JSON に書く。
//!
//! 使い方: `script-bench --bundle-dir <dir> --iters <n> --out <json>`
//!
//! 判定 1 回は試験用の領域 `host_selftest` の呼び出し（async op を 1 回含む）。isolate ごとに warm up を
//! 捨ててから測る。RSS の増分は最初の isolate を作る前からの差で、`two` は 2 個の合計。

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use celeris_script::manifest::{VerifiedBundle, load_bundle_dir};
use celeris_script::selftest::{check_declared, check_output, selftest_call};
use celeris_script::{ScriptError, ScriptRuntimeHandle, ScriptSource};
use serde_json::json;
use tokio::runtime::Runtime;

const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const WARMUP: usize = 100;
const KIB_PER_MIB: f64 = 1024.0;

struct Args {
    bundle_dir: PathBuf,
    iters: usize,
    out: PathBuf,
}

const USAGE: &str = "usage: script-bench --bundle-dir <dir> --iters <n> --out <json>";

fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let (mut bundle_dir, mut iters, mut out) = (None, None, None);
    let mut args = args;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(USAGE)?;
        match flag.as_str() {
            "--bundle-dir" => bundle_dir = Some(PathBuf::from(value)),
            "--iters" => {
                let n: usize = value.parse().map_err(|_| USAGE)?;
                iters = Some(n).filter(|n| *n > 0);
            }
            "--out" => out = Some(PathBuf::from(value)),
            _ => return Err(USAGE.into()),
        }
    }
    match (bundle_dir, iters, out) {
        (Some(bundle_dir), Some(iters), Some(out)) => Ok(Args {
            bundle_dir,
            iters,
            out,
        }),
        _ => Err(USAGE.into()),
    }
}

/// `/proc/self/status` の `VmRSS`（KiB）。
fn vm_rss_kib() -> Result<f64, String> {
    let status = std::fs::read_to_string("/proc/self/status").map_err(|e| e.to_string())?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().strip_suffix("kB"))
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| "VmRSS not found in /proc/self/status".to_owned())
}

/// 昇順に並んだ値の nearest-rank の百分位。
fn percentile(sorted: &[f64], p: usize) -> f64 {
    let rank = (sorted.len() * p).div_ceil(100).max(1);
    sorted.get(rank - 1).copied().unwrap_or(f64::NAN)
}

fn start(source: ScriptSource) -> Result<ScriptRuntimeHandle, String> {
    ScriptRuntimeHandle::start(source).map_err(|e: ScriptError| e.to_string())
}

/// 判定を `n` 回呼び、1 回ごとの遅延（ms）を返す。
fn calls(rt: &Runtime, handle: &ScriptRuntimeHandle, n: usize) -> Result<Vec<f64>, String> {
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let call = selftest_call(CALL_TIMEOUT);
        let begin = Instant::now();
        let reply = rt.block_on(handle.call(call)).map_err(|e| e.to_string())?;
        samples.push(begin.elapsed().as_secs_f64() * 1000.0);
        check_output(&reply.output)?;
    }
    Ok(samples)
}

fn measure(bundle: &VerifiedBundle, iters: usize) -> Result<serde_json::Value, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| e.to_string())?;
    let baseline = vm_rss_kib()?;

    let first = start(bundle.source())?;
    calls(&rt, &first, WARMUP)?;
    let mut samples = calls(&rt, &first, iters)?;
    let one = vm_rss_kib()? - baseline;

    let second = start(bundle.source())?;
    calls(&rt, &second, WARMUP + iters)?;
    let two = vm_rss_kib()? - baseline;
    second.shutdown();
    first.shutdown();

    samples.sort_by(f64::total_cmp);
    Ok(json!({
        "bundle_sha256": bundle.manifest.bundle_sha256,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "iters": iters,
        "warmup": WARMUP,
        "latency_ms": { "p50": percentile(&samples, 50), "p99": percentile(&samples, 99) },
        "rss_baseline_mib": baseline / KIB_PER_MIB,
        "rss_mib": { "one": one / KIB_PER_MIB, "two": two / KIB_PER_MIB },
    }))
}

fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;
    let bundle = load_bundle_dir(&args.bundle_dir).map_err(|e| e.to_string())?;
    check_declared(&bundle.manifest)?;
    let report = measure(&bundle, args.iters)?;
    let mut text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(&args.out, text).map_err(|e| format!("{}: {e}", args.out.display()))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("script-bench: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::percentile;

    #[test]
    fn percentile_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 50), 50.0);
        assert_eq!(percentile(&v, 99), 99.0);
        assert_eq!(percentile(&[7.0], 50), 7.0);
        assert_eq!(percentile(&[7.0], 99), 7.0);
        assert_eq!(percentile(&[1.0, 2.0], 99), 2.0);
    }
}
