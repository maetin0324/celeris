---
title: 健全性v2 R2 unit stale-with-refs — 消した with_* と EventSink の browser_* を指す説明・試験名を今の名前に直す
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R2 unit stale-with-refs — 消した with_* と EventSink の browser_* を指す説明・試験名の修正

task 01M6TYVJT0Z1CBGNK8VTNK8ZJH の WorkUnit stale-with-refs の記録。基準は `$CELERIS_WU_BASE`。

完了日: 2026-11-11

結論: 消した method（WorkerAdapter の with_* と EventSink の browser_*）を指す doc comment・試験名・変数名を、11 file で今の名前に直した。計画 v5 で `crates/task-worker/src/adapter.rs` が範囲に入ったので、v4 で残った 3 か所も直した。コードの挙動と、試験の本体・assert は変えていない。

## 実装前 brief

1. 根本原因: adapter-settings と browser-port が method を消したが、doc comment・試験名・変数名が消した method の名前を指したまま残った。adapter.rs の 757・758 行の変数 `with_model` は、後の repair-settings-2 が足した試験に入っていた。
2. 責務 owner: 各 doc comment と試験を持つ file。
3. 既存の再利用先: 試験の本体はそのまま使う。doc comment の新しい参照先は `configure`（`AdapterSettings`）と `task_worker::browser_live::BrowserControl::browser_auth_section`。
4. 選択肢: 名前だけを直す（採用。objective の指定どおり）。試験を統合・削除する案は objective の範囲外。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 消した method を指す名前と説明。
7. 不変条件: 挙動と試験の本体・assert を変えない。下の「名前だけであることの確認」で確かめた。

## 変更

commit（件名）:

- `phase 1: rename stale with_* test names and point doc comments at configure and BrowserControl`（v4。下の表の (1)〜(4)）
- `phase 2: record stale-with-refs renames and the adapter.rs scope gap`（v4 の記録）
- `phase 1: rename the remaining with_model variable and with_env_overrides reference in adapter.rs`（v5。下の表の (5)）
- この記録を done にする commit

| | file | 直した箇所 |
|---|---|---|
| (1) | `crates/task-dispatch/src/dispatcher.rs` | `planner_permission_mode` の doc comment（`adapter.with_permission_mode` → `AdapterSettings` の `permission_mode` を `configure` で適用） |
| (2) | `crates/task-dispatch/src/dispatcher/cos_chat/launch/tests.rs` | 1 行目の doc comment（passed to `with_env` → `configure` が適用する `AdapterSettings` の `env`） |
| (3) | `crates/task-worker/src/{acp,claude_code,codex,langmem,local_deep_research,paperqa}/tests.rs` | 試験名 `with_env_overrides_a_same_name_key_already_in_config_env` → `configure_env_overrides_…`、変数 `with_env` → `configured` |
| (3) | `crates/task-worker/src/claude_code/tests.rs` | 試験名 `with_permission_mode_overrides_…` → `configure_permission_mode_overrides_…` |
| (3) | `crates/task-worker/src/langmem/tests.rs` | 試験名 `with_model_replaces_…` → `configure_model_replaces_…` |
| (3) | `crates/task-worker/src/aider/tests.rs` | 試験名 `with_model_returns_…` → `configure_model_returns_…`、変数 `with_model` → `configured` |
| (4) | `crates/task-core/src/browser_control_ops.rs` | `browser_control_auth_section` の doc comment（`EventSink::browser_auth_section` → `task_worker::browser_live::BrowserControl::browser_auth_section`。task-core は task-worker に依存しないので intra-doc link にしない） |
| (5) | `crates/task-worker/src/adapter.rs` | 470 行の doc comment（`with_env_overrides_…` → `configure_env_overrides_…`）と、試験 `adapter_settings_configure_env_reports_whether_env_was_taken` の 757・758 行の変数 `with_model` → `configured`。adapter.rs の他の行は変えていない |

`LocalWorkspace` の `with_env`・`with_container` と helper `adapter_with_env` は変えていない（objective の指定）。`agent-docs` の過去の ADR・進捗は履歴なので変えていない。

## 名前だけであることの確認

試験は改名だけで、本体と assert は変えていない。消した試験・弱めた試験は無い。

- 7 つの `tests.rs`: 変更前の内容に上の置換だけを sed で当てた結果と変更後の file を `diff -q` で比べ、7 file とも一致した。差分の行は試験名の行と、`let with_env = base`・`let outcome = with_env`・`let with_model = adapter`・`assert_eq!(with_model.id(), AiderAdapter::ID)` の変数名だけである。
- `adapter.rs`: `git diff --stat` は 3 行の置き換え（3 insertions、3 deletions）。変更前の内容に 470・757・758 行の置換だけを当てた結果と変更後の file を `diff -q` で比べ、一致した。
- 改名後の 9 試験は `cargo nextest list -p task-worker --lib` の一覧にある。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

コードの最後の commit（v5 の adapter.rs）の後に実行した。その後の commit はこの進捗ファイルだけを変える。

| 検査 | コマンド | 結果 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | exit 0 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 影響試験 | `bash scripts/dev/test-affected.sh` | exit 0。nextest 8197 passed（23 skipped）、web の unit 712 passed、test-ts-verify 20 passed |
| 内容検査の with_* | `grep -rnE '\bwith_(model\|llm_endpoint\|permission_mode)\|let with_env\b\|fn with_env_\|passed to .with_env\|with_env_overrides_' crates/` | 0 件 |
| `EventSink::browser_` | `grep -rn 'EventSink::browser_' crates/ docs/` | 0 件 |
| doc comment の参照先 | `grep -c 'task_worker::browser_live::BrowserControl::browser_auth_section' crates/task-core/src/browser_control_ops.rs` | 1 件 |
| adapter-settings の計測 | `python3 agent-docs/progress/2026-11-10-health-v2-r2/measure.py --check-adapter-settings` | exit 0、`check-adapter-settings: ok` |
| 範囲検査 | 計画 v5 の範囲 check（adapter.rs を含む許可一覧） | 範囲外の path 0 件 |
| 進捗の索引 | `sh scripts/dev/progress-index.sh --check` | ok |

## 未解決事項

無し。v4 で残した 2 件（adapter.rs の 757・758 行の変数 `with_model` と、470 行の doc comment の `with_env_overrides_…`）は、計画 v5 で adapter.rs が範囲に入ったので (5) で直した。

## 提案

無し。
