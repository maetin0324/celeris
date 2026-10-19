---
task: 01M59KY5FNM0K4XGPPS61D0CA2
wu: celeris-drv
status: done
completed: 2026-10-19
---
# celeris-drv: 既存 Celeris 通常経路で同一 scenario を流す薄い driver

- 実装: `crates/campaign/src/dagbench/celeris_side.rs`（`RunSpec`・`CelerisSide::drive`・台本 worker `StubWorker`・ext-sim poller）。
  設計判断は `agent-docs/adr/2026-10-19-dagbench-celeris-driver.md`（C1〜C4）。再利用先は module 先頭のコメントに R 番号で明記。
- 依存追加: `celeris-campaign` に `task-dispatch`・`async-trait`（本体 crate は無変更）。
- 試験 `cargo test -p celeris-campaign --test dagbench_celeris`: 2 passed（k2-mini camp-1 makespan 1980 = 理論値、center 2 campaign）。
  三者（worker ledger・ext-sim ledger・SQLite）の整合、物理起動 = 論理 job = 消費、DAG 順序、worker 同時数 ≤ 2。
- 本番に触れない: 一時 DB（tempdir）・この process 内の Dispatcher / ext-sim のみ。daemon・userns・network なし。

## 証拠
- `cargo test -p celeris-campaign --test dagbench_celeris --test dagbench_ext --test runner_checker` → 2 / 4 / 16 passed
- `cargo clippy -p celeris-campaign --lib --bins --test dagbench_celeris --test dagbench_ext --test runner_checker -- -D warnings` → 0
- `cargo fmt --all -- --check` → 0

## 未解決・提案
- faults WU: 障害 (a) は `CelerisSide` を drop して同じ SQLite・ledger で作り直す形（in-process 再起動）が最小。実 SIGKILL が要るなら
  driver を子 process 化する（src/dagbench 内に置く）。(b)/(c) は `ExtConfig.faults` を `CelerisSide::new` に渡す口を足す。
- 位相レベル stage は k2-* では DAG と等価。等価でない scenario を足すときは stage 障壁の待ちを制御遅れとして数える（ADR C2）。
- metrics WU: 制御 overhead は in-process なので、driver process 全体ではなく Dispatcher の tick の CPU を分けて計る工夫が要る。
