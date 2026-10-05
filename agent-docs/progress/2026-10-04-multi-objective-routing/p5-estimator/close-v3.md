---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: close-v3
status: done
updated: 2026-10-05
---

# Phase 5 close（再配置）: close branch の取り込みと実 sidecar 原票転記の確定

旧 unit `close` は範囲 check の基点が join-fix 前の `18f60cef` のまま固定され通らなかったため、
新 key `close-v3` で置き直した。新しい実装はしていない。close branch
`celeris-wu/01M4651ZZP8FJKGG6W2WPFNKBH/close`（`3687abbb` の子孫。`a2f43c39`・`3f1faea1`・`5c9930b0`・
人の原票 `3c1a0f5c`・転記 `8f42739e`）を `git merge --ff-only` で取り込んだ（差分は
`agent-docs/`・`docs/reports/`・`docs/ops/model-routing-migration.md` のみ）。取り込み後 HEAD `8f42739e`。

## 人の回答（`routellm-weights-use`）の転記の確認

原票（`docs/reports/model-routing-routellm-shadow/`、入口 `run-manifest.txt`）と照合し、report §3・
ADR 付記「Phase 5 の実装済み範囲（close による全体検査）」・[../p5-estimator.md](../p5-estimator.md)・
`docs/ops/model-routing-migration.md` §10 に正しく転記されていることを確認した。欠けは無く追加編集はしていない。
人の注意 4 点の行方:

1. 制約違反数・coverage・`estimator_version_mismatch` は本番 config の opt-in（手順書 §10.7）が要り**未計測**。
   偽 sidecar の値で埋めていない（report §3.2・§3.4）。
2. HF repo に Apache-2.0 本文の `LICENSE` file があるが model card に宣言なし。内部評価に限る人の決定は変えない
   （report §3.3、手順書 §10.2）。
3. checkpoint は `XLMRobertaForSequenceClassification`・`model.safetensors` 約 1.1 GB で、手順書 §10.5 の
   旧目安（BERT-base・0.45 GB）と違う。§10.5 は実測値（XLM-RoBERTa・検証済み構成）に直してある。
4. 手順書 §10.3 の状態は **approved**（2026-10-05）。旧い「pending（2026-10-05 時点）」行は無いことを grep で確認。

## 証拠（統合後 HEAD `8f42739e`、close branch を ff-only で取り込んだ後）

| # | コマンド | 結果 |
| --- | --- | --- |
| 1 | `git merge --ff-only celeris-wu/01M4651ZZP8FJKGG6W2WPFNKBH/close` | `Updating 3687abbb..8f42739e Fast-forward`（差分は agent-docs/・docs/reports/・docs/ops/model-routing-migration.md のみ） |
| 2 | `bash scripts/dev/test-parallel.sh` | exit 0。`Summary [73.679s] 4035 tests run: 4035 passed (1 slow), 13 skipped`、doc-test 0 failed、`test-parallel: ok`（`CELERIS_TEST_SUMMARY` の `passed: 0` は既知の集計器不具合。Summary 原文を採用） |
| 3 | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 4 | `cargo fmt --all -- --check` | exit 0 |
| 5 | `cargo nextest run --workspace --no-fail-fast -E 'test(/routing_sidecar_/) \| test(/routing_routellm_pair_adapter/) \| test(/routing_estimator_/) \| test(/routing_shadow_/) \| test(/routing_decision_shadow/) \| test(/routing_offline_replay/) \| test(/cheap_local_first_/) \| test(/cheap_only_legacy_mappings_never_route_frontier_or_standard_to_qwen/) \| test(/claude_429_falls_back_to_the_next_account_and_records_a_cooldown/) \| test(/select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one/)'` | exit 0。`39 tests run: 39 passed, 4009 skipped`（Phase 5 の新試験と Phase 4・回帰。0 件実行でない） |
| 6 | `python3 -m unittest discover -s scripts/model-routing -p 'test_*.py'` | exit 0、`Ran 3 tests` OK |
| 7 | `sh scripts/model-routing/check-runbook.sh --require-approved` | exit 0、`ok (routellm 0b64fdafe049e596a3f5657c219329f24af24198, routellm-weights-use=approved)` |
| 8 | `sh scripts/model-routing/fake-shadow-check.sh` | exit 0、`ok (validated, SIGTERM exit 0, pid gone, port … closed)` |
| 9 | `sh scripts/model-routing/real-sidecar-check.sh` | exit 2（weights なし = 未実行。合格扱いにしない。実行の証拠は人の原票 `3c1a0f5c`） |
| 10 | `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` | 各 exit 0（doc-links ok・adr-numbers ok 137 files・progress-index ok） |

## 未解決事項

- Celeris 本体の estimator shadow（本番 config の opt-in、手順書 §10.7）での coverage・
  `estimator_version_mismatch`・hard constraint 違反数・primary 変更数は未計測。opt-in は人の別判断。
- 本番 `~/.config/celeris`・daemon・DB には触れていない。外部ネットワークに出た試験は無い（worker 内は偽 sidecar のみ）。
