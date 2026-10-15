---
task: 01M4ZRKYMX6FWH0VCWY3GXPX94
wu: jev-tick
status: done
completed: 2026-10-15
---
# jev-tick: tick 側の連携（ready WU を分類し ClassificationBoard へ）

- `crates/celeris/src/daemon/classify_tick.rs` `ClassifyTick`（ADR jev-wire D3）。`tick_loop` が `Dispatcher::tick` の直前（active のみ）に呼ぶ。`[jev] enabled = true` のときだけ `JevClassifier`（Config の timeout/retry/cache/breaker）＋ `SecretsDirKeyProvider` で構築。reload で `[jev]` が変われば作り直す。
- 対象: `ready_tasks` と `running_tasks_with_runnable_work_units` の task。計画ありは `runnable_work_units` の WU（subject = WU id）、計画なしは task id。分類済み entry は再分類しない（冪等）。鍵未登録・認証失敗の entry だけは次に ready になったとき再分類（登録後に同じ WU が分類される）。429・5xx 等は再分類しない・鍵要求にならない。
- `fixed` / 無効時は呼ばず board は空（従来どおり）。dispatcher・store は無変更（board を読むだけ）。`daemon.rs` の `#[allow(dead_code)]` を外した。
- 試験 5 件（classify_tick::tests）: 有効で board が埋まり冪等 / runnable な WU だけ WU id で分類 / fixed は不呼出・空 / 無効は構築しない / credential 失敗のみ再分類。
- 証拠: `cargo fmt --all -- --check` exit 0 / `cargo clippy --workspace --all-targets -- -D warnings` 警告なし / `test-parallel.sh` passed 5363 failed 0。
- 未解決: fake server での一連 dispatch tick 試験と「本番への移行」節は jev-e2e。
