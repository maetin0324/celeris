# labels-diff: outcome label の変更規模

tasks: [01M4P7CTS42KTCR8HS4JF0252R]

D3(b) の `changed_lines`・`changed_files_n`・`changed_crates_n` を実装した。`task_ops::routing_labels::outcome_labels_for_run` は対象 run の `run_write_sets` を優先し、WU run に run 固有行がない場合は `work_unit_write_sets` に fallback する。complete 状態と base/head SHA が揃った行だけを数え、`git diff --numstat` の追加＋削除を合計する。binary の `-` は 0 行、変更 path は repo:path で重複除去、crate 数は `crates/<name>/` の異なり数。複数 repo は合計し、repo path 解決や git が失敗した場合は3欄とも null＋missing。

Git 呼び出しと `project_repos` の repo path 解決は task-ops に置いた。task-core の `route_extract::ChangeCount` は3つの数値だけを受け取り label を埋める。遅延計算を選んだため daemon の終了経路は変更していない。API schema と brief template に変更はない。

## 検証

- `routing_outcome_git_numstat_counts_lines_files_crates`: 一時 git repo の2 commitで `crates/a/`・`crates/b/`・`notes/` と binary を変更し、7行・4 path・2 crate を確認。
- `route_extract_outcome_reports_missing_metrics_without_git`: 変更規模が無い場合の missing と、`ChangeCount` を渡した場合の3値を確認。
- `cargo test -p task-ops --lib routing_outcome_git_numstat_counts_lines_files_crates`: pass。
- `cargo test -p task-core --lib route_extract_outcome_reports_missing_metrics_without_git`: pass。
