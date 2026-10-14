---
tasks: [01M4X23K84B46SCAHRWVXRS8ZK]
wu: backend
status: done
updated: 2026-10-14
completed: 2026-10-14
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

## api（WU api、完了 2026-10-14）

### 変更
- `crates/task-core/src/local_machine.rs`: 予約 id `LOCAL_CLUSTER_ID = "local"`、`MAX_CONCURRENCY_RANGE`（1..=256。store も共有）、
  保護 path の定義 `HOST_PROTECTED_HOME_DIRS`（`.config/systemd`・`.local/celeris/releases`・`.config/celeris`）と
  `HOST_PROTECTED_ABS_DIRS`（`/local/celeris/state/releases`）、`ProtectedPaths::for_daemon(home, db, config_path, releases_dir)`、
  ADR D5 の検証 `validate_workspace_root`（`~` 展開・絶対 path・`..` 拒否・作る前と後の canonicalize で保護 path と
  同じ／中／含むを拒否、`/` と `$HOME` は同じだけ拒否・`create_dir_all`・一時 file の作成と削除）。
- `crates/task-worker/src/db_guard.rs`: 自前の `HOST_CONFIG_DIRS` を消し、`task_core::local_machine::HOST_PROTECTED_HOME_DIRS` を使う
  （db_guard と path 検証が同じ定義を共有）。
- `crates/task-api`:
  - `Clusters.local: LocalMachineView`（`id`・`hostname`・`in_use`・`workspace_root`/`max_concurrency` = `LocalSetting{value, source, config_value, apply}`・
    `scratch_root`・`updated_at`）。`apply` は `new_tasks` / `next_tick`、`source` は `cli|settings|config`。
  - `PUT /api/v1/clusters/local/settings`（`require_admin`、`LocalSettingsPutBody` の全置換、`deny_unknown_fields`）。検証失敗は 422 `validation`
    と `errors[].field`（`workspace_root` / `max_concurrency`、両方なら 2 件）。直の経路は `local_settings_set(actor = "admin")`。
  - CoS `local.settings_put`: `cluster.settings_put` と同じ `OperationAudit::apply_checked` の中で `local_settings_set_tx`（actor `cos:<operation id>`）、
    target は `local`/`local`。登録は `{id}` の行より前。
  - 書いた後に `AdminRequest::LocalSettingsChanged` を admin チャネルへ `try_send`（返事は待たない）。
  - `ApiSettings.local_machine: LocalMachineSettings { scratch_root, home, protected }`。
- `crates/celeris`: `api_settings` が `[scratch]`（有効時の `scratch_dir()`）・`$HOME`・`ProtectedPaths::for_daemon`（`[db] path`・設定ファイル・
  `[selfdeploy] releases_dir`）を渡す。`daemon/admin.rs` は `LocalSettingsChanged` を受けて log するだけ（dispatcher への反映は WU wire）。
- 文書: `docs/api/v1/api.md`（route 表 223、§3.23 の `local`、§3.126.3 の予約 id、§3.126.23、CoS 表）、
  `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` と `config/skills/cos-operator/operations.md` の CoS 操作表
  （registry と一致させる既存試験 2 件が要求する）。
- schema: `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema` で `docs/api/v1/api-v1.schema.json`、`node web/scripts/gen-types.mjs` で
  `web/api/generated/{schema.json,types.ts}`。web の画面は `Clusters` を読むだけ（`local` の描画は web の葉）。
- 試験 `crates/task-api/tests/local_settings_admin.rs`（名前固定 4 件）。task の受け入れ条件の名前 `--test local_machine_settings` でも
  走るよう、`tests/local_machine_settings.rs` が `#[path]` で同じ file を module として読む（Cargo の `[[test]]` 2 重定義は manifest 警告が出るので不採用）。
  - `local_settings_get_shows_local_row`: スナップショット無し（in_use null・config の値・apply）と有り（hostname・in_use）、`items` に混ざらない。
  - `local_settings_put_validation_errors`: 401、相対・空・`..`・`/`・`~`・`$HOME`・DB dir/file・releases・`~/.config/celeris`・保護 path を含む dir・
    symlink 経由・file の下・書けない dir（root 以外）、`max_concurrency` 0/257/-1、2 欄同時、未知欄。拒否は store・監査・admin 通知なし。
  - `local_settings_put_audit_and_apply`: `~/ws` 展開・dir 作成・監査 actor `admin`・通知 1 回、全置換（省略で消える、全 null で行削除）、
    CoS 経由（直の CoS 呼び出しは 422、封筒で applied、監査 actor `cos:<op id>`、`cos_operations` の target）、CoS の検証失敗 422。
  - `local_settings_store_beats_config_in_response`: store の欄ごとの上書きが応答で config に勝つ（`source`・`config_value`）。

### 証拠
- `cargo test -p task-api --test local_settings_admin -- --exact <名前>` → 4 件とも `test result: ok. 1 passed`。
- `cargo test -p task-api --test local_machine_settings` → `test result: ok. 4 passed`。
- `cargo test -p task-core local_machine` → `test result: ok. 4 passed`。
- `git grep -q 'clusters/local' -- docs/api/v1/api.md` → exit 0。`cargo test -p task-api --lib committed_schema`（UPDATE_SCHEMA なし）→ 1 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 0、`passed: 5121, failed: 0, ignored: 13`（userns: null）。初回は CoS 操作表の試験 2 件
  （`cos_operator_skill_table_matches_allowed`・`cos_ops_registry_matches_adr_domain_assignment_and_exclusions`）が落ち、表に行を足して直した。

### 未解決事項
- dispatcher が `LocalSettingsChanged` で store の `max_concurrency` を読み直す結線・`[[clusters]] id = "local"` の読み込みエラーは WU wire。
- web の型検査（`pnpm -C web typecheck`）は node_modules が無いので走らせていない。生成型で `Clusters.local` が必須になったが、
  web の TS は `Clusters` を組み立てない（読むだけ）。

### 提案
- なし。

## wire（WU wire、完了 2026-10-14）

### 変更
- `crates/task-dispatch`: `DispatchConfig.max_concurrency` を `config_max_concurrency`（設定ファイル値）に改名し、dispatcher は
  直接読まない。`Dispatcher` に実効値 `max_concurrency`（store > 設定ファイル、`resolve_max_concurrency`）を持たせ、
  `Dispatcher::new`・tick の頭・`refresh_max_concurrency`（admin 通知）・`set_config_max_concurrency`（reload）で解き直す。
  枠の判定（`run_load`・work_units・review_spawn・cos_chat launch・取得窓）は全て `self.max_concurrency` を見る。
  下げても走行中の run は止めない。store が読めなければ直前の値のまま（warn）。
- `crates/celeris`:
  - `daemon/admin.rs`: `AdminRequest::LocalSettingsChanged` → `local_settings_changed(dispatcher)`（store から解き直し）。
    `reload_providers` は設定ファイルの `max_concurrency` を `config` と dispatcher に入れ直す（store があれば store が勝つ）。
    両関数を `celeris::{local_settings_changed, reload_providers}` として公開（結線試験が daemon と同じ関数を通すため）。
  - `config/cluster.rs`: `[[clusters]] id = "local"`（`task_core::local_machine::LOCAL_CLUSTER_ID`）は読み込みエラー。
  - `daemon/tick_loop.rs`: 起動ログの `max_concurrency` は実効値。
- 旧い直参照: `git grep -n 'config\.max_concurrency' -- crates/task-dispatch crates/celeris/src` → 0 件
  （残る `config.max_concurrency` は celeris の `Config` の設定ファイル値で、`dispatch_config`・`ConfigView`・reload・検証だけ）。
- 試験（名前固定）:
  - `crates/celeris/tests/local_settings_wire.rs`（daemon と同じ組み立て: `Config::load` → `build_dispatcher` / `api_settings` → loopback HTTP）
    - `effective_max_concurrency_store_beats_config`: config 2 → `PUT /clusters/local/settings {"max_concurrency":5}` の応答
      （value 5・source settings・config_value 2・apply next_tick）→ admin チャネルの `LocalSettingsChanged` を `local_settings_changed` に渡すと 5。
      config を 3 に書き換えて `reload_providers` → `config.max_concurrency == 3` でも実効 5、tick 後も 5。`PUT {}` で 3 に戻る。
      store を直接 7 にすると次の tick で 7、`build_dispatcher` し直しても 7。
    - `cli_workspace_root_ignores_store_and_pins`: `--workspace-root`（`Overrides`）があると store の root と既存の固定を無視して
      `effective_root_in`（source Cli）・`task_root` が CLI の root、新 task の `pin_task_root` は CLI の root を返し固定を書かない、
      既存の固定行は変えない。`GET /clusters` の `local.workspace_root` も CLI の値・source `cli`。
    - `cluster_id_local_is_reserved_config_error`: `id = "local"` は `Config::load` がエラー（"reserved"）、`local2` は読める。
  - `crates/task-dispatch/src/dispatcher/tests/cos_capacity.rs`:
    `store_max_concurrency_applies_next_tick_without_stopping_running_runs`（config 1 → store 3 で次の tick に +2、store 1 に下げても
    3 本は `Running` のまま・新規 0、上書きを消して reload 値 4 に戻ると +1）。
  - 既存試験の `d.config.max_concurrency = N` は `d.set_config_max_concurrency(N)` に置き換えた。

### 証拠
- `cargo test -p celeris --test local_settings_wire -- --exact <名前>` → 3 件とも `test result: ok. 1 passed`。
- `cargo test -p task-dispatch --lib -- --exact dispatcher::tests::cos_capacity::store_max_concurrency_applies_next_tick_without_stopping_running_runs` → `ok. 1 passed`。
- 既存 workspace・走行中 run 不変: `cargo test -p celeris --test workspace_roots` → `ok. 3 passed`。
- task の受け入れ条件: `cargo test -p task-api --test local_machine_settings` → `ok. 4 passed`、`cargo test -p task-core local_machine` → `ok. 4 passed`、
  `git grep -q 'clusters/local' -- docs/api/v1/api.md` → exit 0、`cargo test -p task-api --lib committed_schema` → `ok. 1 passed`（API の型は変えていないので schema 再生成なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 0、`passed: 5125, failed: 0, ignored: 13`（userns: null。CELERIS_USERNS_TESTS=1 は不要の指示）。
- doc 検査: `sh scripts/dev/check-adr-numbers.sh` → ok（204 files）、`sh scripts/dev/check-doc-links.sh` → ok、
  `python3 scripts/dev/check-architecture-map.py` → OK（363 件）、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok。

### 未解決事項
- `GET /clusters` の `max_concurrency.config_value` は API 起動時の `ConfigView` の値で、reload で設定ファイルの値を変えても API 側は再起動まで
  古い値を出す（dispatcher の実効値は reload 後の値で正しい）。`ConfigView` は reload で差し替わらない既存の仕組み。
- web の画面（`local` 行の描画・PUT）は web の葉。

### 提案
- `ConfigView`（`GET /config` と clusters の `config_value`）を reload で差し替える仕組み（watch チャネル）を別 task で。

### 本番への移行
- 本番 config に `[[clusters]] id = "local"` があると起動できない。昇格前に人が `grep -n 'id = "local"' ~/.config/celeris/config.toml` で 0 件であることを確かめる（あれば別の id に書き換える）。
