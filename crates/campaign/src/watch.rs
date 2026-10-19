//! 継続監視/再開 driver: 既存の `runner::run`（1 回走査）を間隔を置いて繰り返し、台帳に未完了の試行も
//! 待ちも無くなるまで続ける。新しい scheduler ではない。状態の正本は台帳（out ディレクトリ）で、
//! プロセスを落として同じ引数で起動し直せば台帳から同じ試行の追跡を続ける。
//! 予算は `run` の gate がそのまま守る（承認済みの manifest 以上は投入しない）。
//! 同じ out ディレクトリに driver を同時に 2 つ走らせない（投入は冪等キーで重複しないが、監視が重なる）。
use std::{path::Path, time::Duration};

use routing_eval::manifest::Manifest as InputManifest;

use crate::{
    executor::Executor,
    ledger::Ledger,
    manifest::CampaignManifest,
    runner::{self, RunOptions, RunReport},
};

/// 待ちの注入点。試験は実時間を使わず、sleep の呼び出しで外部状態を進める。
pub trait Clock {
    fn sleep(&self, d: Duration);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchOptions {
    pub interval_secs: u64,
    pub max_iterations: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchEnd {
    /// 台帳に未完了の行も、実行待ちの試行も無い（終端）。
    Settled,
    /// 走査回数の上限に達した。未完了が残る。再起動して続けられる。
    MaxIterations,
}

impl WatchEnd {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::MaxIterations => "max_iterations",
        }
    }
}

#[derive(Debug)]
pub struct WatchReport {
    pub iterations: u32,
    pub end: WatchEnd,
    /// 最後の走査の結果。
    pub last: RunReport,
    /// 終了時点で台帳に残る未完了の行数（この campaign の分）。
    pub unfinished: usize,
}

fn unfinished(m: &CampaignManifest, out: &Path) -> Result<usize, String> {
    let hash = m.compute_sha256()?;
    Ok(Ledger::open(out)?
        .unfinished()
        .iter()
        .filter(|r| r.key.manifest_sha256 == hash)
        .count())
}

/// `run` を終端まで繰り返す。1 回の走査が Err（台帳の I/O など）なら、そこで止めて Err を返す。
pub fn watch(
    m: &CampaignManifest,
    input: &InputManifest,
    exec: &dyn Executor,
    opts: &RunOptions,
    w: WatchOptions,
    clock: &dyn Clock,
) -> Result<WatchReport, String> {
    let pin = opts.lock.as_ref().and_then(|l| l.series.clone());
    watch_with_series(m, input, exec, opts, pin.as_ref(), w, clock)
}

/// 副比較（B1/C1）の系列台帳の固定位置を明示して回す `watch`。
pub fn watch_with_series(
    m: &CampaignManifest,
    input: &InputManifest,
    exec: &dyn Executor,
    opts: &RunOptions,
    series: Option<&crate::series::SeriesPin>,
    w: WatchOptions,
    clock: &dyn Clock,
) -> Result<WatchReport, String> {
    if w.interval_secs == 0 || w.max_iterations == 0 {
        return Err("--interval-secs and --max-iterations must be >= 1".into());
    }
    let mut iterations = 0;
    loop {
        let last = runner::run_with_series(m, input, exec, opts, series)?;
        iterations += 1;
        let left = unfinished(m, &opts.out)?;
        // 実行待ち（waiting）が 0 で、台帳にも未完了が無ければ終端。予算切れで打ち切った試行も、
        // 外部の停止を確認して台帳が閉じるまでは unfinished に残るので、ここでは終わらない。
        if left == 0 && last.waiting == 0 {
            return Ok(WatchReport {
                iterations,
                end: WatchEnd::Settled,
                last,
                unfinished: left,
            });
        }
        if iterations >= w.max_iterations {
            return Ok(WatchReport {
                iterations,
                end: WatchEnd::MaxIterations,
                last,
                unfinished: left,
            });
        }
        clock.sleep(Duration::from_secs(w.interval_secs));
    }
}
