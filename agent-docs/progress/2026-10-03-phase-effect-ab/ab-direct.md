---
tasks: [01M412JERQ4T85RDY32TZAHE9M]
wu: ab-direct
status: done
completed: 2026-10-03
---
# atomic_route: planner 経由と direct route の A/B 試験

## 実装

同じ単一 repo・担当あり・coding harness の task を 2 回走らせる。off には既存の explicit compound hint を設定し、`direct_route::evaluate` が planned を返す経路で planner が 1 unit の計画を出してから WU を実行する。on は hint を外し、direct route で planner を通らず実装する。どちらも command check と reviewer を完了まで通す。

偽 adapter は各 run の新 session に 40,000 input tokens と 120 秒、resume に 4,000 tokens と 80 秒を割り当てる。秒数は `SimClock` に加算するため実時間に依存しない。各 run の `Terminal::Done` にも固定 token usage を返す。このシナリオでは resume は起きず、planner・実装・reviewer のすべてが新 session となる。

## 証拠

- `cargo test -p task-dispatch --lib phase_effect_ab::atomic_route -- --nocapture` → exit 0、1 passed。
  - `ab-metric atomic_route off runs=3 wall_secs=360 input_tokens=120000 fresh_sessions=3`
  - `ab-metric atomic_route on runs=2 wall_secs=240 input_tokens=80000 fresh_sessions=2`
- `on.runs < off.runs`、経路・planner run 数・WU 数・task 完了を試験内で assert。
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0。
- `git diff --check` → exit 0。

## 未解決事項

- なし。
