---
tasks: [01M4X23K84B46SCAHRWVXRS8ZK]
wu: backend
status: running
updated: 2026-10-14
---
# backend: ローカル設定の store・管理 API・config 優先順位・dispatcher 反映

設計は [ADR 2026-10-14-local-machine-settings](../../adr/2026-10-14-local-machine-settings.md)（D2・D3・D5）。

## store（WU store、完了 2026-10-14）

### 変更
- `crates/task-core/migrations/0078_local_settings.sql`: ADR D2 の SQL そのまま（`local_settings`・`task_workspace_roots`・`local_settings_events`）。
  `SCHEMA_VERSION` を 78 に上げ、版数を固定している試験（`assert_eq!(SCHEMA_VERSION, 78)`、`delivery::tests::migration_0042_fills_gaps_in_main_schema_41_database` の版数列）を直した。
  `celerisctl/tests/no_migrate.rs` は `SCHEMA_VERSION` から版数を計算するので変更なし（4 passed）。
- `crates/task-core/src/store/local_machine.rs`（新規）と `TaskStore` の 5 method:
  - `local_settings_get` / `local_settings_set(overrides, actor, now)`（全置換。全欄 `None` で行を削除。同じ transaction で
    `local_settings_events` に before/after の JSON を 1 行 INSERT）/ `local_settings_events`（古い順）。
  - CoS 用に `SqliteStore::local_settings_set_tx(conn, …)`（`cluster_settings_set_tx` と同じ形。呼び出し元の transaction で監査も巻き戻る）。
  - `task_workspace_root_pin`（`INSERT OR IGNORE`、固定済みの root を返す）/ `task_workspace_root_get`。
  - `local_settings_events` と `task_workspace_roots` への UPDATE・DELETE はコードに無い。`max_concurrency` は store でも 1..=256 以外を `StoreError::Invalid` で拒む（監査も書かない）。
  - 型: `LocalSettings`・`LocalSettingsOverrides`・`LocalSettingsEvent`（`task_core::` から re-export）。
- `crates/task-core/src/local_machine.rs`（新規）: resolver `WorkspaceRoots { config_root, cli_root }`。
  - `effective_root(settings)` / `effective_root_in(store)`: CLI > store > config、`Resolved { value, source }`（`SettingSource` = `cli|settings|config`）。
  - `task_root(store, id)` / `task_dir(store, id)`: CLI があれば CLI（固定を無視）、無ければ固定、固定が無ければ設定ファイルの root（store 値ではない＝既存 task 不変）。
  - `pin_task_root(store, id, parent_id, now)`: 既存の固定はそのまま。root task は実効の root、子 task は親の `task_root`（親に固定が無ければ設定ファイルの root）を継ぐ。CLI 時は固定を書かずに CLI の root を返す。
  - `resolve_max_concurrency(config_value, settings)`: store > config。
- 呼び出し側（`config.workspace_root.join(task_id)` の 130 箇所）はこの葉では触っていない（WU roots）。

### 証拠
- `cargo test -p task-core --lib local_machine` → exit 0、`test result: ok. 4 passed`
  （`store::local_machine::tests::{local_settings_store_roundtrip, local_settings_audit_event_appended, task_workspace_root_pin_once}`、
  `local_machine::tests::workspace_root_priority_cli_store_config`）
- `cargo test -p task-core` → 940 passed（初回 1 件失敗 = 版数列の固定、直して `delivery::tests` 3 passed）
- `cargo test -p celerisctl --test no_migrate` → 4 passed
- `cargo clippy --workspace -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、`passed: 5101, failed: 0, ignored: 13`（userns: null）

### 未解決事項
- 呼び出し側の置換（WU roots）、`GET /clusters` の `local`・`PUT /clusters/local/settings`（WU api）、dispatcher の `max_concurrency` 反映と予約 id `local`（WU wire）は未着手。
- path 検証（ADR D5 の 1〜3）は API 側の責務。store は文字列をそのまま保存する。

### 提案
- なし。

### store 葉 attempt 2（check の --exact 対応）
- 原因: 計画の check は `cargo test -p task-core --lib -- --exact <名前>` で、--exact は module path 込みの完全名と比べる。試験が `store::local_machine::tests::…` / `local_machine::tests::…` にしか無く 0 件一致で落ちた。
- 対処: 4 試験を crate 直下（`crates/task-core/src/lib.rs`）にも同名で置き、本体は各 module の tests を呼ぶ。
- 証拠: 4 件それぞれ `out=$(cargo test -p task-core --lib -- --exact <名前> 2>&1); printf '%s\n' "$out" | grep -q 'test result: ok. 1 passed'` → exit 0。`cargo clippy -p task-core --all-targets -- -D warnings` exit 0、`cargo fmt -p task-core -- --check` exit 0。

## convert（WU roots: ops → worker → dispatch → bins → tests-close、完了 2026-10-14）

### 変更
- 足回り `<root>/<task_id>` の解決を `task_core::local_machine::WorkspaceRoots`（`task_root`・`task_dir`・
  `effective_root_in`・`pin_task_root`）1 本に寄せた。`config.workspace_root` は `Config::workspace_roots()` の中で
  resolver の基底値（設定ファイルの root）としてだけ使い、`--workspace-root` は `workspace_root_cli` として別に持つ。
  - ops（62a6bdf0）: `task_ops::workspace::{local_dir, prepare_task_dir}`・view（`ViewContext.workspace_roots`）・delivery。
  - worker（ed500cb4・bb0619ae）: task-worker の `workspace_prune`・`workspace_targets`、task-api の files（`task_local_dir`）・
    `marker_of`・docs・timeline・task_io・changes・tree。schema 再生成。
  - dispatch（943a7531）: `DispatchConfig.workspace_roots`。dispatch で root を固定（既存の足回りが固定なしの root に
    あれば固定しない）、`check_log_root`・housekeeping・target_sweep・maintenance・review_spawn・eval_trials・disk_watch・backfill。
  - bins（2b8d2f7f）: celeris の tick_loop（dispatcher の resolver を共有）・doc_gardener・knowledge_curation（用意時に固定）・
    knowledge_gc・knowledge_maint・delivery、celerisctl の workspace/query/worker。
- 名前固定の 3 試験 `crates/celeris/tests/workspace_roots.rs`（`integration_api.rs` と同じ組み立て: `<home>/config.toml` →
  `Config::load` → `celeris::api_settings` → `ApiState` を loopback に立て、tempdir に `worktree.json`・`runs/<run_id>/stdout.jsonl`・
  `artifacts/report.md` と `ArtifactProduced` event を実際に作る。store の root は `local_settings_set` で変える）:
  - `existing_workspace_unchanged_after_root_change`: 固定なし（設定ファイルの root）と固定済み（first）の task が、
    store の root を moved に変えた後も `GET /tasks/{id}`（`workspace_dir`・`worktree.dir`）・`/runs/{run}/stdout`・
    `/artifacts`・`/artifacts/0` で変更前と同じ値を返す。固定なしは固定されず `local_dir` が設定ファイルの root。
  - `running_run_workspace_stable_after_root_change`: 走行中（`Running` + lease）の run が root 変更後に同じ場所へ追記した
    stdout が API から 2 行とも読め、`local_dir` は動かず、再 dispatch の `prepare_task_dir` も固定された root を返す。
    固定なしで走り始めた run も設定ファイルの root のまま。新しい root に足回りは作られない。
  - `new_task_uses_store_workspace_root`: 新 task は store の root に用意され `task_workspace_roots` に固定、
    store の root を later に変えた後の子は親の固定（store-root）を継ぎ、新しい root task は later に固定される。

### 証拠
- `cargo test -p celeris --test workspace_roots` → exit 0、`test result: ok. 3 passed`。
  名前ごとに `cargo test -p celeris --test workspace_roots -- --exact <名前>` → 3 件とも `ok. 1 passed`。
- 旧い直参照: `git grep -nE 'workspace_root\s*\.join\(|workspace_root\(\)\s*\.join\(' -- 'crates/*.rs'`（試験を除く）→ 0 件。
  `unpinned_root().join(task_id)` は 3 箇所（dispatcher `pin_task_root`・knowledge_curation の「固定なしの既存足回りがあるか」の
  判定、`check_log_root` の store 読み失敗時の fallback）で、いずれも resolver の内側の規則（既存 task は設定ファイルの root）。
  root を引数で持ち回る旧 API は ops〜bins の葉で消した（`WorkspaceRoots` を受ける形だけが残る）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 0、`passed: 5113, failed: 0, ignored: 13`（userns: null）。

### 未解決事項
- `cargo clippy --workspace --all-targets -- -D warnings` は `crates/task-dispatch/src/dispatcher/followups.rs` の
  `items_after_test_module`（試験 module の後に関数がある。56b12ec3 由来、この task の変更ではない）で落ちる。
  release gate の clippy（`--all-targets` なし）は通る。
- `GET /clusters` の `local`・`PUT /clusters/local/settings`（WU api）と dispatcher の `max_concurrency` 反映（WU wire）は後続。
  3 試験は store の上書きを `local_settings_set` で直接書いている（API ができたら同じ経路になる）。

### 提案
- followups.rs の試験 module を末尾へ移す小さな修正 task（`--all-targets` の clippy を通すため）。
