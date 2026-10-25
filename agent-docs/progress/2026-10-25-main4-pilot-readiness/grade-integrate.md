---
title: main-4 pilot readiness — 採点と成功契約/human_ops を同じ out で統合確認（件数・分母の照合）
tasks: [01M5T9MBPESHAWJSENDWB4NYB1]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# main-4 pilot readiness — 採点と成功契約/human_ops を同じ out で統合確認（件数・分母の照合）

更新日: 2026-10-25。状態: done（件数・分母は一致）。P0-1 の最終資産での再確認は残件（下）。

- 対象の out: env-nollm の無 LLM 通し `M/nollm/out`（M=`/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/measure`、
  stub harness・実測定 daemon binary。数は性能ではない）。out の生記録は変えていない（sha256 を前後で照合、全件一致）。
- 結果: `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/consistency.json`（`consistent: true`・`counts`・`missing`・`wait_intervals`）。
  中間物は同じ dir の `grade-integrate/`（checker 出力・再生成した human-op report・records の再 export・照合の前後）。
- 基準: `git merge-base HEAD main`。main（K4 公開入力 v2 の採用＝P0-1 を含む）を本 branch に merge 済み。

## 変更
- `scripts/campaign/check-out-consistency.py`（新）: out の生記録（campaign.json・trials.jsonl・grades.jsonl）から成功契約を再計算し、
  checker totals・export 行（table があれば）・human-op report の arm を突き合わせる。欠測一覧・未導出の wait 区間・共有作業の 1 回計上も出す。一致で exit 0。
  試験 `scripts/campaign/test_check_out_consistency.py`（8 件: 一致、export のずれ、未実行を欠測に数える report、grade 未結合の checker と結合する report、
  計画 cell の記録欠落、未実行の消費を観測済みにする checker、共有作業の二重計上、out でない dir）。
- `crates/campaign`:
  - `success::Cell.ran`（`TrialRecord::ran`）と join の `success_class.ran`。report は未実行 cell の消費を `*_not_run_trials`・`not_run_trials` に数え、
    欠測数の分母を checker の `ran_trials` と揃えた（値は null のまま）。
  - export: `--records` が無ければ `consumption.scope = missing` と理由を書き、前回の `final-accounting.json` を消す。
  - 回帰試験 `tests/success_contract.rs::unexecuted_cell_consumption_is_not_run_in_report_and_checker_alike`、`::export_without_records_marks_consumption_missing`。
- `scripts/campaign/measure-launcher.sh`: export で RECORDS が無いと警告する。
- 文書: ADR `2026-10-23-campaign-success-contract.md` 付記 2026-10-25、`docs/ops/eval-raw-record-definitions.md` §7、`docs/ops/measure-run-final.md` §8。

## 照合の結果（同じ out、launcher と同じ引数の checker）
| arm | n | executor passed | validated success | 区分 | ran | tokens 欠測 (checker / report) | 未実行 (report) |
|---|---|---|---|---|---|---|---|
| A | 2 | 2 | 0 | model_failure 2 | 2 | 0 / 0 | 0 |
| B | 2 | 0 | 0 | executor_failed 1・executor_cancelled 1 | 1 | 1 / 1 | 1 |

- checker・human-op report・生記録の再計算で全項目一致（`mismatches` 空）。
- **直したずれ**: 修正前の report は B の arm 予算切れ（未実行）cell も tokens・cost の欠測に数え、checker 1 に対し 2 だった
  （`grade-integrate/consistency.prefix-report.json` に 5 件のずれ）。
- **使い方のずれ（コードは変えない）**: checker を `--measurement-id`・資産の引数なしで呼ぶと（nollm の check 段の形）期待 lock が合わず grade を外し（A ungraded 2）、
  保存 lock だけで結合する report（A model_failure 2）とずれる。launcher の grade は正しい引数で呼ぶ。照合 script はこれをずれとして出す
  （`grade-integrate/consistency.nollm-stage-check.json`）。
- export・final-accounting はこの out を拒否する（件数ではなく pin の違反: manifest の set fixed `initial_info` が観測 materials の sha256 と食い違う。値は consistency.json の `blocked_by_pins`）。
  export の行は checker totals の写し（`export::row`）で、既存の `success_contract` 試験が checker と同じ数を確かめている。表そのものの照合は再固定の後。

## 必須入力の供給・欠測・共有 setup・wait
- grades・observed: launcher の `grade`（collect-observed-out → grade-out → check）が供給する。K2 の collect は not_submitted（stub が job を出さない）。
- RECORDS（decision-log）: launcher の collect/grade では作られない。人の手順（`docs/ops/measure-run-final.md` §8 の `celerisctl --db <写し> campaign export`）。
  nollm 段の `dl/records.jsonl` は 0 件だったが、同じ DB 写しから再 export すると 75 件（usage・decision・run・outcome・trial 各 15）。
  無いときに表が黙って消費を落としていたのを、欠測と明記するよう直した。
- 欠測を 0 にしない: B K2 の trial tokens は null（reviewer run 12 件が token を報告しない stub のため木の合計が欠測）。A の cost は 2 試行とも null。
  どれも欠測数に数え、和に 0 を足していない。人の時間・再作業は評価者記録が無く 4 試行とも欠測。
- 共有 setup: この out には shared scope の記録が無い（`shared_overhead` 空、欠測で 0 ではない）。1 回計上の規則は照合 script と既存の
  `human_ops_report::shared_env_lock_fix_is_counted_once_with_correction_history` が確かめる。
- 未導出の wait 区間: internal_wait（入力源が無い）・human_wait・external_queue_wait・external_run（評価者記録なし）は全試行、celeris_wait は A 2 件と
  B K4（未実行）。B K2 の celeris_wait は records から導出（Question/Approval 0 秒）。一覧は consistency.json の `wait_intervals`。

## 検査（提出時の HEAD で実行）
- `python3 scripts/campaign/check-out-consistency.py --out M/nollm/out …`（launcher と同じ資産の引数）→ exit 0、`consistent: true`。
- `python3 scripts/campaign/test_check_out_consistency.py` → 8 passed。
- `cargo test -p celeris-campaign --test success_contract --test final_accounting` → 4 passed + 11 passed。
- `bash scripts/dev/test-affected.sh` → exit 0、2648 tests run: 2648 passed、7 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。`sh scripts/dev/progress-index.sh --check` → ok。

## 残件
- **P0-1 の最終資産での再確認**: K4 公開入力 v2 は main に入ったが、main-4 bundle（judged-scope-v2）と この out は v1（main-v3-n2-43706.tgz）のまま。
  final-freeze で K4 pin と lock を v2 に作り直した後、その out で同じ照合（export・final-accounting の表を含む）を再実行する。
- manifest の `initial_info` 再固定（人・final-freeze の項目、nollm-pass.json の pins）。これが直るまで export・final-accounting の表は出ない。

## 提案
- launcher に records の段（測定 DB の read-only `--db` から `celerisctl campaign export`）を足し、export で RECORDS を必須にする。実運用の手順が変わるので人の決定とする。
- human-op report に期待 lock の検査を足す（`--manifest`・資産の引数を受ける）か、report に「保存 lock で結合」と明記する。
