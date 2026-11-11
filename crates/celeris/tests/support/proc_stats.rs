//! 資源を測る試験（`long_run_soak.rs`・`project_controller_replay.rs`）が使う、自 process の
//! `/proc` の読み取りと百分位。読むだけで、process の状態は変えない。

// `mod support;` で読む試験の多くはこの module を使わない。
#![allow(dead_code)]

/// `/proc/self/status` の `field`（例 `VmRSS:`）の値（kB）。読めなければ 0。
pub fn status_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(field))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

/// 自 process の thread の数（`/proc/self/task` の項目数）。読めなければ 0。
pub fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").map_or(0, |d| d.count())
}

/// `v` の `p` 百分位（並べた後の `(len - 1) * p / 100` 番目）。空なら 0。
pub fn pct(v: &[u64], p: usize) -> u64 {
    let mut v = v.to_vec();
    v.sort_unstable();
    v.get((v.len().max(1) - 1) * p / 100).copied().unwrap_or(0)
}
