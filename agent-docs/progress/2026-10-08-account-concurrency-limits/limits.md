---
title: dispatch をアカウントごとの上限で止め、プロバイダ concurrency を deprecate する（pool 行）
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
status: done
updated: 2026-10-08
---
# limits: pool 行のアカウント上限

2026-10-08、run `01M4DAA9VTRJG0HJ905Z9E5CRG`。WorkUnit `limits` の実装。

## 実装

- `ProviderConfig.concurrency` は `Option<usize>`。明示された行ごとに tracing warn を出す。`providers.d` も同じ経路で検証する。省略時の互換表示・非 pool 行の旧上限は 1。値は後続 `implicit` の引き継ぎ用に保持する。
- **0 も警告付きで読む**。ADR (a) の「0 は設定エラー」と異なるが、この WorkUnit の Objective (1) の明示指示を優先した。pool 行では 0 を含め旧値は容量の意味を持たない。非 pool 行の旧判定は後続 `implicit` まで維持する。
- migration `0062_account_limits.sql` に ADR の正本設定表を追加。`TaskStore::account_limits` / `set_account_limit` が読み込み・設定・解除を行う。保存値の範囲は `1..=32`。鍵は `(adapter, account_id)` で、`implicit` も保存可能。
- Dispatcher が起動時に表を読み、設定・解除は SQLite 書き込み成功後にメモリへ反映する。daemon の reload は表を読み直す。読み込み失敗時はアカウントの dispatch を止め、黙って既定に戻さない。
- 実アカウントは保存された `max_runs`、無ければ `[accounts] max_runs_per_account` を使う。CoS はその実効上限 +1、通常 run の容量には CoS を数えない。観測値の `account_in_use` は全 run を数えるまま。
- pool 行は中央の `provider_full` で旧値を参照しない。provider selection・routing shadow・sticky・review/dispatch 経路・CoS chat が個別上限を使う。routing のモデル候補にも pool の旧 concurrency を渡さない。非 pool の `provider_full` は後続移行まで残す。
- アカウント削除では保存値も消す。保存失敗時はディレクトリの復元を試み、エラーを返す。
- migration 試験の schema 版数と旧 DB fixture、`ProviderConfig` を直接作る celerisctl の試験も更新した。

## 試験

一時ファイルは workspace 内の `test-tmp/` を使用。run の長い `$TMPDIR` は既存 browser_doctor 試験の UNIX socket の `SUN_LEN` を超えるため、試験コマンドだけ短いパスへ切り替えた。既存 db_maintenance の 300ms 待機に合わせ、試験並列数は 2。

- `TMPDIR=<workspace>/test-tmp cargo test -p task-core -- --test-threads=2`: 成功（lib 881 件、統合試験・doc-test を含む）。
- `TMPDIR=<workspace>/test-tmp cargo test -p celeris -p task-dispatch -- --test-threads=2`: 成功（celeris lib 410 件、task-dispatch lib 873 件、各統合試験・doc-test を含む）。
- `cargo clippy --workspace --all-targets -- -D warnings`: 最終変更を含め成功。
- `cargo clippy --workspace -- -D warnings`: 成功。
- `cargo fmt --all -- --check` / `git diff --check`: 成功。

受け入れ条件に対応する試験:

| 条件 | 試験 |
| --- | --- |
| 0: pool が旧 concurrency で止まらない | `pool_ignores_provider_concurrency_and_uses_account_overrides`（旧値 1 でも 2 アカウントに計 4 本。個別上書きの設定・解除で動的に変わる） |
| 1: CoS +1 | `cos_run_bypasses_pool_concurrency_on_the_least_loaded_account_up_to_max_cos_runs`、`cos_account_usage_does_not_consume_leaf_capacity`、`cos_chat_pool_uses_account_override_plus_one_and_ignores_provider_zero` |
| 2: 互換読み込み・警告 | `legacy_provider_concurrency_is_optional_and_warns_including_zero_and_include`（省略・明示値・0・pool・providers.d と tracing の出力） |
| 3: 保存・解除・削除・再読み込み | `account_limits_persist_clear_and_keep_adapters_separate`、`dispatcher_account_limits_load_on_start_and_reload`、既存アカウント削除試験と daemon reload 試験への永続値の確認追加 |
| 4: crate 試験・clippy | 上記の全試験・workspace clippy |

ログと受け入れ条件ごとの証拠は workspace の `artifacts/` に置く。本番 config・systemd・daemon・本番 DB は変更していない。

## 後続 unit への引き継ぎ

`implicit` は非 pool 行を暗黙アカウントへ移し、残した旧判定を置き換える。`api` は管理要求・PATCH・GET の表示欄を追加し、`Dispatcher::set_account_limit` / `account_max_runs` を利用できる。API・UI はこの unit の範囲外。

先行 ADR run の `pi-opencode-go` / `opencode-go/grok-4.6` のモデル存在・protocol の実行時確認は行っていない。本 run では別 LLM CLI/API を起動せず、harness のモデル経路も変更していない。その経路の再使用前の確認は harness 管理側に残る。

## 再チェック（2026-10-08、run 01M4DBFX9ZXV7WJGEGNPQXR94P）

前回の範囲チェック不合格を同じ cwd とコマンドで再現した（exit 1）。対象は `crates/celerisctl/src/commands/worker_tests.rs` だけ。このファイルの変更は公開 `ProviderConfig.concurrency` の `Option<usize>` 化に合わせた fixture 5 件の型追従であり、変更だけを戻すと `cargo check -p celerisctl --tests` が E0308 5 件で exit 101。修正を復元すると exit 0。必要な修正は保持した。

check はこの run では変更できないため、許可対象へこの 1 ファイルを追加する `yield.plan_issue` を `artifacts/result.json` に記録し、done を返さず replan に引き継ぐ。検証ログは `artifacts/scope-fixture-compile.log` と `artifacts/scope-fixture-restored.log`。前回の全試験・clippy の記録は上記のとおりで、本 run では全体試験を再実行していない。

## 最終確認（2026-10-08、run 01M4DBXPJCMGK3B9TRMXDWR1Z3）

replan で `crates/celerisctl/src/commands/worker_tests.rs` の fixture 修正が許可範囲に入った。範囲チェック（許可対象にこの 1 ファイルを加えたもの）に掛かる変更は無い。`cargo check -p celerisctl --tests` は exit 0、`cargo test -p task-dispatch -p celeris` は exit 0（1387 passed）、`cargo clippy -p task-dispatch -p celeris --all-targets -- -D warnings` と `cargo clippy --workspace -- -D warnings` も exit 0、`cargo fmt --all -- --check` も exit 0。

既知の flaky: `db_maintenance::tests::spawned_tasks_run_on_their_interval_and_stop_cleanly` は 300ms 以内のバックアップ書き込みを前提としていて、host の負荷が高い（load 5〜10、24 並列）と落ちることがある。変更を stash した HEAD でも 2 回とも同じように落ち、単独実行と `--test-threads=4` では通るので、この unit の変更とは関係無い。上記の最終実行では通った。
