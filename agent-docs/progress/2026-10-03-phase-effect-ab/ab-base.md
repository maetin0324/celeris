---
task: phase-effect-ab
wu: ab-base
status: done
completed: 2026-10-03
---
# phase_effect_ab: 土台と scenario continuation（ab-base）

## やったこと
- `crates/task-dispatch/src/dispatcher/tests/phase_effect_ab.rs` を新設し、`tests/mod.rs` の末尾に `mod phase_effect_ab;` を登録。
  - `AbMetric { runs, wall_secs, input_tokens, fresh_sessions }`（u64）、`Variant { Off, On }`
  - `print_ab_metric`（1 行 `ab-metric <scenario> <off|on> runs=… wall_secs=… input_tokens=… fresh_sessions=…`）、`ab_metric_line`
  - `assert_on_reduces_runs_or_fresh_sessions`
  - `mod continuation; mod atomic_route; mod review_sync;` を宣言済み
- `phase_effect_ab/atomic_route.rs`・`phase_effect_ab/review_sync.rs`: doc comment だけの stub（後続の葉は mod 行を触らずに中身を埋める）。
- `phase_effect_ab/continuation.rs`: implement WU 1 枚の fixture 計画で 予算切れ → yield → 完了 の 3 run を、
  `continuation_session_resume=false`（off）と `true`（on）で走らせる。偽アダプタ `TokenScriptAdapter`（id `claude-code`）は
  新しい session なら 40000 token・120 秒、resume なら 4000 token・80 秒を数え、時計は偽アダプタが進める `SimClock`（実時間に依らない）。
  session_resume.rs の `ClaudeScriptAdapter` は token 勘定を持たないので共有せず、小さな専用アダプタにした（mod.rs の `new_task`・`wu_spec`・`dispatcher_with_adapter_id`・`run_until_idle` は再利用）。

## 証拠
- `cargo test -p task-dispatch --lib phase_effect_ab::continuation -- --nocapture` → exit 0、1 passed
  - `ab-metric continuation off runs=3 wall_secs=360 input_tokens=120000 fresh_sessions=3`
  - `ab-metric continuation on runs=3 wall_secs=280 input_tokens=48000 fresh_sessions=1`
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0
- `cargo test -p task-dispatch` → lib 582 passed / 0 failed（他 4 passed）

## 未解決事項
- `cargo test --workspace` は本葉では走らせていない（変更は task-dispatch の試験だけ。統合段で走る）。
- tokio の `test-util`（`start_paused`）は dev-dependencies に無いので使わず、注入した `SimClock` で wall_secs を決めた。

## 提案
- なし
