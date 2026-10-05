---
title: "Phase 5（p5-estimator）: 外部/学習 estimator の必要性評価と pluggable adapter（shadow → opt-in）"
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# Phase 5（p5-estimator）: 外部/学習 estimator の必要性評価と pluggable adapter

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §10 Phase 5。人の回答 adapter-plus-routellm に従い、
汎用 sidecar adapter（既定 off・`shadow_only = true` 必須・失敗時は heuristic・決定権なし）と RouteLLM sidecar の
shadow 評価の道具立てを実装した。本番の切り替えはしていない。unit 別の記録は [p5-estimator/](p5-estimator/)、
実装と ADR の突き合わせは ADR 末尾の付記「Phase 5 の実装済み範囲（close による全体検査）」。

## 状態

- **実 RouteLLM sidecar の評価は人が実行し、report §3 に転記した（2026-10-05）。** 人の決定 `routellm-weights-use`
  （内部の shadow 評価に限る）に沿って Fable が本番 host で `real-sidecar-check.sh` の start-stop（exit 0）と
  上限付き shadow（exit 0、上限 50 に対し 30 件、completed 30 / failed 0 / timeout 0 / dropped 0、外部呼び出し 0）を
  実行し、原票を `docs/reports/model-routing-routellm-shadow/`（commit `3c1a0f5c`）に置いた。
  Celeris 本体の estimator shadow（coverage・`estimator_version_mismatch`・制約違反数・primary 変更数）は本番 config の
  opt-in が要るため未計測。偽 sidecar の結果で埋めていない（report §3.4）。本番切り替えはしていない。
- 必要性評価（assess）: 外部 estimator を adapter で shadow 比較する価値はある。自動学習・本番選択はしない
  （[assess.md](p5-estimator/assess.md)、ADR 付記「Phase 5 必要性評価」）。

## 証拠（close、統合後 HEAD `18f60cef8b39`、`CELERIS_WU_BASE = 18f60cef8b39bdb43e517ba3622b4c5c47e1e2e4`）

| # | コマンド | 結果 |
| --- | --- | --- |
| 1 | `bash scripts/dev/test-parallel.sh` | exit 0。`Summary [  74.457s] 4034 tests run: 4034 passed (1 slow), 13 skipped`、doc-test 0 failed、`test-parallel: ok`（`CELERIS_TEST_SUMMARY` の `passed: 0` は既知の集計器不具合。Summary 原文を採用） |
| 2 | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 3 | `cargo fmt --all -- --check` | exit 0 |
| 4 | `cargo nextest run --workspace --no-fail-fast -E 'test(/routing_sidecar_/) \| test(/routing_routellm_pair_adapter/) \| test(/routing_estimator_/) \| test(/routing_shadow_/) \| test(/routing_decision_shadow/) \| test(/routing_offline_replay/) \| test(/cheap_local_first_/) \| test(/cheap_only_legacy_mappings_never_route_frontier_or_standard_to_qwen/) \| test(/claude_429_falls_back_to_the_next_account_and_records_a_cooldown/) \| test(/select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one/)'` | exit 0。`38 tests run: 38 passed, 4009 skipped`（Phase 5 の新試験と Phase 4・回帰。0 件実行でない） |
| 5 | `python3 -m unittest discover -s scripts/model-routing -p 'test_*.py'` | exit 0、`Ran 3 tests` OK |
| 6 | `sh scripts/model-routing/check-runbook.sh` | exit 0、`ok (routellm 0b64fdafe049e596a3f5657c219329f24af24198, routellm-weights-use=pending)` |
| 7 | `sh scripts/model-routing/fake-shadow-check.sh` | exit 0、`ok (validated, SIGTERM exit 0, pid gone, port … closed)` |
| 8 | `sh scripts/model-routing/real-sidecar-check.sh` | `not run: set CELERIS_ROUTELLM_REAL=1 and CELERIS_ROUTELLM_WEIGHTS_DIR to approved local weights`（未実行。合格扱いにしない） |
| 9 | `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` | exit 0 |
| 10 | `test -d docs/reports/model-routing-routellm-shadow` | 無い（原票なし → 転記なし） |

### 再検査（2026-10-05、shadow-join 統合を取り込んだ HEAD `3f1faea1`）

- `bash scripts/dev/test-parallel.sh` → exit 0、`4035 tests run: 4035 passed, 13 skipped`
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- 上表 4 の filterset → exit 0、`39 tests run: 39 passed, 4009 skipped`（shadow-join の `routing_estimator_shadow_joins_daemon_records` が増えた）
- `check-runbook.sh` → ok（`routellm-weights-use=pending`）、`fake-shadow-check.sh` → exit 0、`real-sidecar-check.sh` → exit 2（未実行）
- 文書検査 3 本（`check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`）→ exit 0
- 原票 `docs/reports/model-routing-routellm-shadow/` は依然として無い → 転記なし

### 原票の転記後の再検査（2026-10-05、HEAD `3c1a0f5c` + 記録の変更のみ）

- `bash scripts/dev/test-parallel.sh` → exit 0、`4035 tests run: 4035 passed (1 slow), 13 skipped`、`test-parallel: ok`
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- 上表 4 の filterset → exit 0、`39 tests run: 39 passed, 4009 skipped`
- `python3 -m unittest discover -s scripts/model-routing -p 'test_*.py'` → OK
- `sh scripts/model-routing/check-runbook.sh --require-approved` → exit 0、`routellm-weights-use=approved`
- `sh scripts/model-routing/fake-shadow-check.sh` → exit 0。worker 内の `real-sidecar-check.sh` → exit 2（weights なし。実行の証拠は人の原票）
- 文書検査 3 本 → exit 0
- 人の原票（`run-manifest.txt`・`start-stop.log`・`shadow.log`・`real-sidecar-results.json`）: start-stop exit 0、shadow exit 0・30/30 completed

### close-v3（再配置）: close branch を取り込んだ統合後 HEAD `8f42739e` で再検査（2026-10-05）

旧 unit `close` は範囲 check の基点が join-fix 前の `18f60cef` のまま固定され通らなかったため、
新 key `close-v3` で置き直し、close branch（`a2f43c39`・`3f1faea1`・`5c9930b0`・原票 `3c1a0f5c`・転記 `8f42739e`）を
`git merge --ff-only` で取り込んだ（差分は `agent-docs/`・`docs/reports/`・`docs/ops/model-routing-migration.md` のみ）。
新しい実装はしていない。人の回答 `routellm-weights-use`（start-stop exit 0・pair score 1 件・SIGTERM で exit 0・
port 閉鎖・15.0s・peak RSS 約 2.6 GB、shadow は上限 50 に対し合成 dataset 30 件で completed 30 / failed 0 /
dropped 0 / timeout 0 / 外部呼び出し 0、latency mean 44.6ms・p95 42.9ms・max 426ms（初回）、weights revision
`86237e3df400762178ea98379477b8296e66d5e4`）の転記が report §3・ADR 付記・手順書 §10.1/§10.3/§10.5（approved・
XLM-RoBERTa 実測値）に揃っていることを確認し、欠けは無し。統合後 HEAD で全体検査し直した:

- `bash scripts/dev/test-parallel.sh` → exit 0、`4035 tests run: 4035 passed (1 slow), 13 skipped`、`test-parallel: ok`
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- 上表 4 の filterset → exit 0、`39 tests run: 39 passed, 4009 skipped`（Phase 5 の新試験と Phase 4・回帰）
- `python3 -m unittest discover -s scripts/model-routing -p 'test_*.py'` → OK（3 tests）
- `sh scripts/model-routing/check-runbook.sh --require-approved` → exit 0、`routellm-weights-use=approved`
- `sh scripts/model-routing/fake-shadow-check.sh` → exit 0。worker 内の `real-sidecar-check.sh` → exit 2（weights なし = 未実行。合格扱いにしない）
- 文書検査 3 本（`check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`）→ 各 exit 0

## 未解決事項

1. Celeris 本体の estimator shadow（本番 config の opt-in、手順書 §10.7）での coverage・`estimator_version_mismatch`・
   hard constraint 違反数・primary 変更数は未計測。opt-in は人の別判断。
2. license: HF repo に Apache-2.0 本文の `LICENSE` file があるが model card に宣言なし。外部発表前に人が再判断。
3. 推移依存の lock と CPU/GPU wheel の選択（今回の freeze は原票 `venv-freeze.txt`）、sidecar 推論の実資源費
   （daemon は名目 `SIDECAR_NOMINAL_CALL_USD` で数える）、raw score の v1 DTO 専用欄、校正（`calibration_version`）。
4. 手順書 §10.5 の旧目安（BERT-base・0.45 GB）は実測（XLM-RoBERTa・1.1 GB、peak RSS 約 2.6 GB）で直した。GPU 構成は未検証。

## 提案

- 実利用の判断材料を得るなら、人が別 Phase で §10.7 の opt-in（`shadow_only = true`・日次上限付き）を短期間入れ、
  `celerisctl routing evaluate --policy estimator` の coverage・制約違反数を測る。本番判断への反映（opt-in の primary 化）は
  さらに別の決定とする。
- raw_pair_win_rate は難易度の平均順に並ぶが範囲が重なり未校正。校正なしに primary の判断材料にしない。
