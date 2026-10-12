---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: LLM source と harness の runtime 切替 closeout
updated: 2026-10-12
---
# LLM source と harness の runtime 切替 closeout

## 状態

runtime の実行経路は `llm_route` に切り替え済み。provider 固定選択・adapter hint・tier alias は撤去した。全体検査（`test-parallel`・`clippy`）は通った。ADR `2026-10-11-llm-source-harness-selection.md` は「決定（実装済み: runtime）」に更新した。

検査の HEAD は `a3c63b8a`（e2e-flake）。各葉の変更と個別 check は `agent-docs/progress/2026-10-11-llm-source-harness-runtime/<unit>.md` に記録済み。

## 葉の一覧（runtime-v2）

- 経路の切替: `dispatch-select`・`dispatch-switch`・`harness-adapters`・`review-switch`・`run-switch`・`cos-route`・`worker-harness`・`verify-smoke`・`proxy-concrete`。通常 run・reviewer run・CoS・worker・llm-proxy は `select_route` を通る。
- 記録と台帳: `store-migrate`（task の harness 欄・runs の選択欄・account_limits の `(source_id, account_id)`）・`accounts-source`（quota・cooldown・同時実行数を `(source, account)` へ）。
- 試験と例: `e2e-accounts`・`e2e-api`・`examples`（`config_examples_load`）・`edges-fix`。
- remove 段: `prune-select`・`prune-policy`・`api-unprovider`・`snapshot-unprovider`・`catalog-unprovider`・`config-drop`・`hint-drop`。`config-remove` は失敗し、`config-drop` に引き継いだ。`api-remove`・`hint-remove` は計画から外した（`api-unprovider`・`hint-drop` が該当分を消した）。
- 統合後の修正: `store-fix`・`run-fix`・`e2e-fix`（superseded）・`remove-integration-fix`（knowledge_gc・doc_gardener の adapter 残り、smoke role の harness、route_select 試験の反転、worker-protocol schema 再生成）。
- 最後の修正: `e2e-flake`（`task-api` browser_e2e の `ipc::serve` が `Io` で返す揺れ。原因は unix socket の path 長上限 `SUN_LEN`）。

## closeout 検査（この run の実行結果）

HEAD `a3c63b8a`、作業ツリーは本 unit の docs 変更のみ。

- `bash scripts/dev/test-parallel.sh`（TMPDIR を短くした。下の注）
  - 最終 run: `TMPDIR=/local/x` で exit 0。`CELERIS_TEST_SUMMARY`: `passed 4900, failed 0, ignored 14, nextest_exit 0, doctest_exit 0, summary_parsed true`。ログは `artifacts/closeout2-test-parallel.log`。
  - 再実行（closeout2 の同じ検査を 1 回やり直し、HEAD `64541a80`、crates/ 差分ゼロ）: `TMPDIR=/local/x bash scripts/dev/test-parallel.sh` exit 0、`passed 4900, failed 0, ignored 14, nextest_exit 0, doctest_exit 0`。ログは `artifacts/closeout2-test-parallel-rerun.log`。
- `cargo clippy --workspace -- -D warnings`: exit 0（`Finished`、警告 0）。`--all-targets` 付きも exit 0。再実行も exit 0（ログ `artifacts/closeout2-clippy-rerun.log`）。
- `schema_matches`（`task-core`・`task-api`・`task-worker`）は test-parallel の中で通った。
- 残存名検査（`grep -rn --include=*.rs -F`、`crates/` 内）: `ProviderConfig|SourceOrigin|ProviderSpec|LocalProviderSpec|provider_select|in_use_cos|ProviderView` は 0 件。`worker_hint.adapter` と `/api/v1/providers` も 0 件。`tier_models` は 8 件で、すべて task-worker の `with_tier_models`（`adapter.rs`・`browser_specialist.rs`・`tiered.rs`）。config の `tier_models` 欄は消えている。worker 側の `with_tier_models` は未撤去（下の未解決）。
- 試験名の存在: `legacy_providers_rejected_`（1）・`config_examples_load`（1）・`migration_harness_`（2）・`account_limits_source_`（3）・`harness_select_`（5）・`route_select_`（20）。

### TMPDIR の注（環境起因）

`test-parallel.sh` は `$TMPDIR/celeris-test-parallel.XXXXXX/tmp` を試験の TMPDIR にする。長い TMPDIR を渡すと、試験が作る unix socket の path が `SUN_LEN`（108 byte）を超える。

| TMPDIR | 結果 |
|---|---|
| run の `runs/…/tmp`（長い） | exit 100。104 件失敗。`path must be shorter than SUN_LEN` |
| `/local/celeris/data/scratch/cl2` | exit 100。30 件失敗。同じ SUN_LEN 系 |
| `/local/celeris/data/t` | exit 100。4 件失敗（下記）。2 回とも同じ 4 件 |
| `/local/x` | exit 0。4900 passed、0 failed |

`/local/celeris/data/t` で落ちた 4 件は、単独（`cargo test -p celeris-credentiald --test broker`・`-p task-api --test browser_waits`・`-p task-worker --lib action_socket_dir_must_be_private_to_this_user`）では pass。全体実行の負荷下と path 長の組み合わせで落ちたと見ているが、path 長が直接の原因だとは確かめていない。

失敗は branch の問題ではなく環境（TMPDIR の長さ）の問題と判断した。この unit は crates/ を変えていない。browser_e2e の揺れは e2e-flake で直した後も、TMPDIR が長いと再現する。

## 未解決

- task-worker の `with_tier_models`（`crates/task-worker/src/tiered.rs`・`adapter.rs`・`browser_specialist.rs`、試験 `with_tier_models_replaces_only_the_bindings` を含む）は tier 別 model の差し替えで、ADR の撤去対象（legacy tier の焼き付け）に当たるかをまだ確かめていない。closeout は crates/ を変えない方針のため未撤去。次段で、tier を worker の model 選択から外せるかを決める。
- 旧い comment 4 か所（動作は無い）: `crates/task-api/src/admin.rs` 1 行目・13 行目の `providers.d/`、`crates/task-api/tests/common/mod.rs` 47 行目、`crates/task-api/src/cos/operations.rs` 88 行目。`config/mod.rs` と `config/tests.rs` の `providers.d` は読み込み拒否の判定と拒否試験なので残す。
- web の生成型と画面に旧型が残る: `web/api/generated/types.ts`・`web/api/generated/schema.json` に `ProviderView`・`ResolvedLlmSource` が残り、`web/features/ops/providers-*.ts(x)` と `gui/app/routes/providers.tsx`・`accounts.tsx`・`gui/app/celeris/types.ts` が旧型を使う。`web/` と `gui/` は本 unit の範囲外。画面は ui-v2、生成型の再生成は api-v2 で行う。
- README と `gui/docs/DESIGN.md` に旧い provider の記述が残る可能性がある（未確認）。
- 旧 ADR の本文（`2026-10-02-provider-llm-source` 系）は履歴として残す。新しい決定は `2026-10-11-llm-source-harness-selection.md` の付記にある。
- `cos` の旧 provider 経路の comment と `providers.d` の文言は api-v2 の作業に含める。

### 別工程

- 画面側の provider UI と旧型の追随: ui-v2。
- 新しい source/account/harness API（`/api/v1/providers` の後継）: api-v2。

## 本番への移行（人が実施）

本番 daemon・DB・config は変更していない。以下は人が行う。

- `[[providers]]`・`providers.d`・`[llm_proxy.models]` を新しい `[llm_sources.*]`・`[harnesses.*]` の宣言へ書き換える。旧 block が残れば読み込みエラーになる（`legacy_providers_rejected_` の動作）。
- account_limits の移送のうち残った行を確認する（`(source_id, account_id)` へ移ったかを DB で照合）。
- `harness_migration_unresolved` の event を確認し、解決できなかった task の harness を手で決める。
- 書き換えの前に config と DB のバックアップを取る。失敗時は backup を戻して旧版 daemon を再起動する。

## 提案

- `test-parallel.sh` で TMPDIR の長さを先に検査し、長すぎれば短い base を使うか、警告を出す。今回の run で 100 件以上が SUN_LEN で落ちたため。
- 試験で作る unix socket の path は、`tempfile` の TMPDIR に依らず短くなるよう、試験 helper で `/tmp` 直下の短い dir を使う案を検討する（ADR の範囲外。別 task）。
- web の生成型は api-v2 の完了時に `UPDATE_SCHEMA=1` と `pnpm gen:types` で一括再生成する。
