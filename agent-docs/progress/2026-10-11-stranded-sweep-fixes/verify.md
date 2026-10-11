# verify: 統合後 HEAD の全体検証（2026-10-11）

- 対象 HEAD: `00e3dcb0`（統合後の task branch `celeris-wu/01M4NRXEBW3RM655RZXN7K52CX/verify`）
- 実装は変えていない。`crates/` と `web/` の差分は無い（`git diff --quiet` で base・HEAD とも exit 0）。
- 落ちた検査は無い。原因の調査は不要。

## 結果

| # | コマンド | exit | 結果 |
|---|---|---|---|
| 1 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 0 | nextest: 5170 tests run, 5170 passed, 13 skipped（Summary [108.126s]）。doctest: 3 passed, 0 failed。JSON 集計 `passed: 5173, failed: 0`（= 5170 + 3）。tmp_leftovers 6（終了時に削除） |
| 2 | `cargo clippy --workspace -- -D warnings` | 0 | `Finished dev profile`。警告・エラー無し |
| 3 | `cargo fmt --all -- --check` | 0 | 差分無し |
| 4a | `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok` |
| 4b | `sh scripts/dev/check-adr-numbers.sh` | 0 | `check-adr-numbers: ok (194 files)` |
| 4c | `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |

- 注記: test-parallel の要約は nextest の `passed (N slow)` を数え落とす既知の問題がある。上の件数は原票の Summary 行と doctest 行から数えた。JSON の 5173 はそれと一致した。
- 注記: 全件合格のログは `/tmp/verify-test.log`（run の artifacts 外、この WU の一時）。

## stranded 試験の合格

- 完全な workspace 実行で、接頭辞 `stranded_` の試験はすべて合格に含まれる。
- 単独の確認: `cargo nextest run --workspace -E 'test(stranded_)'` は exit 0、`23 tests run: 23 passed, 5160 skipped`。
- 接頭辞ごとの試験名（`cargo nextest list` で確認）:
  - `stranded_default_branch_*`（3 件は `crates/celeris/tests/stranded_default_branch.rs`、3 件は `crates/task-ops/src/changes/tests.rs`）
    - `stranded_default_branch_null_with_main_is_integrated`
    - `stranded_default_branch_null_with_origin_head_is_integrated_for_repoless_task`
    - `stranded_default_branch_unresolvable_is_unintegrated_with_reason`
    - `stranded_default_branch_is_configured_then_detected`
    - `stranded_default_branch_prefers_origin_head`
    - `stranded_default_branch_unresolvable_is_an_error_with_reason`
  - `stranded_remote_*`（`crates/task-ops/src/stranded/tests.rs` ほか）
    - `stranded_remote_migrates_unintegrated_row_to_not_applicable`
    - `stranded_remote_actions_reject_not_applicable_rows`
    - `stranded_remote_not_applicable_state_is_hidden_from_inbox`
  - `stranded_identity_*`（`crates/task-ops/src/inbox/tests.rs`）
    - `stranded_identity_preserves_attention_item_id_and_at_until_head_changes`

## 実装と ADR 付記の突き合わせ用の事実（record 段が使う）

- 関数: `task_ops::changes::resolve_default_branch`（`crates/task-ops/src/changes.rs:404`）。唯一の解決経路。順序は 設定 → origin/HEAD → main → master → Err。
- sweep 側の薄い包み: `default_branch_for`（`crates/celeris/src/daemon/stranded_sweep.rs:133`）。
- migration: `crates/task-core/migrations/0070_task_integration_not_applicable.sql`（`task_integrations_new` を作り直して行を写す）。`0067_task_repo_integration_states.sql` が前段。
- 旧経路の残りは確認した範囲で無い: `changes::default_branch`（参照 0 件）、checkpoint は `resolve_default_branch` を使う（`crates/task-dispatch/src/checkpoint.rs:43`）。
- `crates/task-api/src/docs.rs:344` の `"main"` は `create_docs_repo`（新規の docs repo を作る時の初期 branch）。解決の fallback ではない。

## 未解決事項

- 無し（検査はすべて合格）。
- 本件の範囲外の既知事項: test-parallel の要約の数え落とし（上記、今回未修正）。

## 本番への移行・ADR 付記・進捗の完了記録

- この WU（verify）は agent-docs/ の本ファイルだけを書いた。`adr/2026-10-11-stranded-done-branches.md` の付記（実装突き合わせ）と、本 task 進捗ファイル `2026-10-11-stranded-sweep-fixes.md` の「本番への移行」節・完了日は、並列の record 段の担当。ここでは変更していない。
