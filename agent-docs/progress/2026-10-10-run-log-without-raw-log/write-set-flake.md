---
task: 01M4HRQBNJCHA35KZP78WW8MM1
wu: write-set-flake
status: done
completed: 2026-10-10
---
# write-set A/B 試験の開始登録競合

## 原因

`phase_effect_ab_write_set_gate_avoids_conflict_and_repair` の `run_variant` は、adapter の `next_end()` が Some になった時点で試験時計を進めていた。dispatcher の `running` への登録は adapter の `run` が初めて poll される前に起きるため、off で同時に spawn された 2 run のうち B が README を読んで `ends` に登録する前に、A の終わりまで時計が進み A が書き終えることがある。すると B は A の書いた版を読み、衝突・repair が消えて off が `(0, 0, 2)` になる（integrate-reclose の失敗、write_set.rs:228）。

## 修正（crates/task-dispatch/src/dispatcher/tests/phase_effect_ab/write_set.rs のみ）

- 時計を進める条件に `adapter.registered_runs() == d.running.len()` を追加。走っている全 run の開始登録（README 読み＋終わりの時刻の登録）が済むまで、既存の出来事待ち（5ms sleep → 次の tick）を続ける。
- 試験専用の遅延 hook `EditAdapter::start_delay`（指定 title の run は README を読む前に実時間で待つ）と、回帰試験 `phase_effect_ab_write_set_waits_for_late_run_start`（off で task B の開始を 50ms 遅らせ、`(1, 1, 3)` と wall_secs = `RUN_SECS_B * 2` を主張）を追加。
- 既存試験の主張（off: 1/1/3、on: 0/0/2、wall_secs の式）は変更していない。
- 兄弟試験（atomic_route・continuation・review_sync・stale_priority）は試験時計を adapter の run 内で自分で進める形で、「全 run の登録前に外から時計を進める」ループを持たないため変更不要。

## 再現と結果

- 修正前の再現: 時計進行条件の追加分を一時的に `|| false` に戻して `cargo test -p task-dispatch --lib phase_effect_ab_write_set` → 新しい回帰試験が `left: (0, 0, 2) right: (1, 1, 3)` で失敗（1 passed; 1 failed）。統合で見えた失敗と同じ値。CPU 負荷は使っていない。
- 修正後: `cargo test -p task-dispatch --lib phase_effect_ab` を 3 回 → 毎回 `9 passed; 0 failed`。
- `cargo clippy -p task-dispatch --tests -- -D warnings` → exit 0。
- `cargo fmt -p task-dispatch -- --check` → exit 0。

## 未解決事項

- なし。全体試験は後続の integrate-reclose / reverify-flake で流す。
