---
tasks: [01M4MAZNNYT4JVRYKCVZ1TF1VF]
---
# stranded-sweep daemon 配線

完了した root task の repo ごとに `celeris/<task-id>` の head を読み、`task_ops::stranded::sweep` へ渡す daemon job を追加した。起動直後に 1 回実行し、その後 300 秒ごとに再実行する。active role のときだけ走る。タスク・repo の読み取りが終わってから git を実行し、git 判定と state 更新は `spawn_blocking` 上で行うため、git 実行中に store lock を保持しない。

- `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings`: 成功。
- `cargo test -p celeris stranded_sweep_runs_immediately_and_repeats_on_period`: 成功（1 件）。tokio の pause/advance で即時実行と周期再実行を確認。
- `git diff --check`: 成功。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 成功。変更 4 path が全て許可範囲内。

未解決事項: なし。
