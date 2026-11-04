# T1-cap-background（旧 Rust の経路）の記録

- 変更: `crates/task-core/src/chat/digest.rs` に `clip_background`（300 code point を超えたら先頭 300 ＋ `…`）と単体試験 1 件を足した。patch は 1 回作っただけで、3 回の trial で同じ patch を使った（直した点はない）。
- trial は 3 回（`trial.jsonl` の 3 行）。
  1. ok=false。test-run 段で exit 100（2378 件中 1 件 fail）。scope 0.03 秒・check 45.0 秒・clippy 40.8 秒・test-build 100.6 秒・test-run 124.8 秒、計 313.7 秒。
  2. ok=false。同じ試験 1 件の fail で、test-run 段で exit 100。scope 0.07 秒・check 157.7 秒・clippy 95.6 秒・test-build 177.1 秒・test-run 137.4 秒、計 589.1 秒。
  3. ok=true。scope 0.11 秒・check 40.3 秒・clippy 67.3 秒・test-build 108.9 秒・test-run 58.8 秒・build 87.1 秒・release-build 212.9 秒・score 27.5 秒、計 627.6 秒。
- 1・2 回目に落ちた試験: `task-dispatch dispatcher::tests::work_units::an_interrupted_integration_is_redone_idempotently_after_restart`（work_units.rs:2993 で `left: Blocked, right: Done`）。digest とは関係のない統合 WU の再起動の試験で、時間に依存する（sleep 200 ms・20 ms と lease の更新）。
  - 同じ patch を当てた別の worktree で確かめた。この試験は単独で 3 回とも pass し、`cargo nextest run -p task-core -p task-dispatch --no-fail-fast` も 2378 件すべて pass した。負荷に依存する flaky と判断し、patch を変えずに trial をやり直した。
  - この試験の決定化は課題の範囲外なので、行っていない。
- 段の dir（`T1-cap-background-rust/`）のログは 3 回目のもの（trial.sh が毎回上書きする）。
- 採点の file（spec.json・spec.cases.jsonl・inputs.jsonl）は読んでいない。
