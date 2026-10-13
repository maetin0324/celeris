---
task: 01M4NM332S384YYC4VC5DB7KCY
unit: brief-render
status: done
completed: 2026-10-11
---
# brief-render: brief template task-brief/2

final review の差し戻し（過去の試行が数だけ・性質が JSON・check の種類が表せない・redact が O(n²)）への対応。`crates/task-core/src/route_record.rs` の純関数を ADR D2 の表どおりに直した。

## 変更
- `TASK_BRIEF_TEMPLATE` = `task-brief/2`。`route_record_schema_pinned` の brief・aux（`missing` に `prior_runs`）・hash（`sha256:3574d17f…`）を更新。
- 過去の試行: `FeaturesInput.prior_runs: Option<Vec<PriorRunInput>>`（status・model・wall_s・`failed_criteria`（文と exit code））。出力は数の行（0 回なら「初回」）、run ごとに 1 行、`repair: N 件` の行。
- 性質: `task_features_v69` を `model_policy::TaskFeatures` の型にした（入力・aux）。brief には固定の日本語名と段（低／中／高）で 1 軸 1 行・順固定で出し、無ければ節ごと省く。
- 検査: `acceptance: Option<Vec<AcceptanceInput>>`（`From<&Criterion>`）と `CheckInput`（enum: `command{cmd}`／`artifact_exists`／`knowledge_page`／`reviewer`／`human`。`From<&Check>`・`From<&WorkUnitCheck>`）。command は実行ファイル名と subcommand まで出す。
- `redact_secrets`: byte 列を 1 回走査する。照合しない区間は `&str` の slice で写すので、残りの文字列は作らない。
- `route_extract.rs` は compile を通すための最小の追従だけ（`Criterion`／`WorkUnitCheck`／`TaskFeatures` を serde で読む）。中身の直しは extract-real。
- 他の試験の `task-brief/1` を `/2` に更新（`celerisctl/tests/routing_cli.rs`・`task-dispatch` の `cheap_local_first.rs`）。ADR 付記に版・hash・新しい文面の規則を追記した。

## 証拠
- `cargo test -p task-core --lib route_` → 26 passed / 0 failed。新しい試験は `route_record_prior_runs_render_one_line_per_run`・`route_record_nature_renders_fixed_axis_sentences_or_is_omitted`・`route_record_checks_render_kind_and_command_name`・`route_record_redact_large_input_replaces_every_secret`（20 万字以上の入力）。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 100（5034 passed / 108 failed）。落ちたのは browser・credentiald の socket 試験だけ（celeris-credentiald・celerisctl browser・task-api browser_e2e/live_cross・celeris browser_doctor）。原因は `path must be shorter than SUN_LEN`（この run の TMPDIR が 94 字）。短い TMPDIR で `cargo nextest run -p celeris-credentiald -p celerisctl -p task-api -p celeris` を回し直すと 1410 passed / 0 failed。

## 未解決
- aux の各欄を実データから埋める作業は extract-real（route_extract）と dispatch-ctx（lane・harness・9 軸）が担当する。
- 進捗 `task-features.md` の `task-brief/1` の記述は reverify で更新する。
