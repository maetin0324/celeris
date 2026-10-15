---
title: "campaign runner・隔離・独立 checker・export（mock 実行器）"
tasks: [01M4Z2989XX5XK159CCDNV1HYH]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# campaign-runner（WU runner）

## 実装済み（`crates/campaign`）
- `executor.rs`: `Executor` trait（Celeris 実行経路を包む接合点）と `MockExecutor`（pass/fail/error/timeout/budget 超過 Spend/欠測）。実 LLM・network・子 process の実装は無い（試験が src の禁止語を走査）。
- `runner.rs`: `celeris-campaign run --manifest <m> --out <dir> --executor mock`。開始時に `campaign.json`・`input-manifest.json`・`run-meta.json`（hash・harness/model/policy 版・予算・終了条件）・`snapshot/{repo,materials,kb}` と `snapshot.json`（各 hash）を保存。試行ごとに `trials/<cfg>/<problem>/<n>/{workspace,kb}` を snapshot から複製。`trials.jsonl` に 1 試行 1 行を追記。trial timeout 超過は runner が timeout に直す。`stop_on_budget` で token/wall 上限に達したら残りを `cancelled(budget_stop)` として分母に残す。再起動は trials.jsonl から再開（末尾の不完全行は切り捨て、中断した試行は snapshot から作り直し）。別 campaign hash の out は拒否。C は事前登録 subset のみ。
- `checker.rs`: `celeris-campaign check`。生記録だけから再計算（runner/aggregate の集計コードは不使用）。拒否: campaign/input hash 不一致・split 混在・期待外/重複試行・passed 旗と outcome の矛盾・版の食い違い・偽 timeout/過大 wall・workspace 共有・初期 hash が snapshot と不一致・予算枯渇後の実行・summary.json との数値不一致（記録欠落）。
- `export.rs`: `celeris-campaign export`。checker が違反ゼロの時だけ `table.json`/`table.csv`（設定 A/B/C 別 n/passed/failed/error/timeout/cancelled/missing/failures/pass_rate、token・cost は observed/partial/missing、欠測は「欠測」、主比較 A/B と副比較 B/C の対応表）。
- 試験: `crates/campaign/tests/runner_checker.rs` 16 件（A/B/C 再計算＝export＝summary 一致、隔離・snapshot、予算停止、再開、改ざん 10 種、CLI 一括）。

## 証拠
- `cargo test -p celeris-campaign`: 全通過（lib 11 / cli_dryrun 2 / routing_eval_identity 3 / runner_checker 16）。
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 5180, failed 0）。`cargo clippy --workspace -- -D warnings`: exit 0。

## 未検証・限界
- 実 Celeris 経路（task 作成・dispatch・RunMetrics）の実行器は未実装。trait の接合点だけ。有料測定はしていない。
- mock は逐次実行。`max_parallel` は dry-run の見積りだけで使い、runner では並列化していない。
- `materials_sha256` の manifest pin 検証は `--materials-src` 指定時のみ。
- summary.json は runner が書くため、記録と summary を両方改ざんする攻撃は hash/split/版/隔離など他の検査でしか捕まらない（records の外部署名は未実装）。
- 再試行（max_attempts>1）は全試行を実行し、判定は各問題の trial_no 最小の記録（既存 aggregate と同じ規則）。

## 提案
- 実行器 trait の Celeris 実装（既存 task 作成経路＋RunMetrics から tokens/cost を採る）を次段で作る。
- trials.jsonl の hash を run 終了時に別媒体（進捗/ledger）へ固定する。
