---
task: preflight (監査修正5)
status: 実装済み・mock検証済み（実経路は未・本番昇格と実測は承認待ち）
completed: 2026-10-16
---
# 本測定の全件 preflight（preflight）

## 実装済み
- `celeris-campaign preflight`（`crates/campaign/src/preflight.rs`）: manifest・版（`check_resolved`）・入力 manifest・全 case 入力（`CaseSet::load`、control/split、`--materials-src` の hash 固定）・grader（`grade_case_ref` の固定 hash、`GradeCase::load`、stage と検査の対応、公開/隠しテスト実体の sha256）・候補構成（A/B/C）・認証/環境・予算/時間を検査し、不足を `missing: [{check, detail}]` で全件返す。一件でも不足なら exit 1。
- run 側の gate: `run --executor harness|celeris` は executor/transport を作る前に `preflight::gate` を通る。不足なら A を含め task 作成・台帳・out dir 作成を 0 件で停止（mock は無課金なので対象外）。ライブラリでは `run_gated`。
- 認証: 環境変数の「名前」の有無・非空だけを `Host` 経由で見る（値は読まない）。URL に資格情報（`@`）があれば拒否し、URL は出力に出さない。
- 予算/時間: 承認枠（全体 300 万・case 30 万・arm 130 万 tokens、並列 1、token_basis=all）を定数で固定し、超過・不一致は不足。`trial_timeout_secs ≦ per_case.max_wall_secs`、case の experiment walltime ≦ trial timeout ≦ per_case（K2/K4 の 30 分整合）、`dry-run` の見積りが枠内。
- `curl_transport::validate_target`（URL/環境変数名の形だけの検査）を `CurlTransport::new` から切り出して共用。runbook `run-final.md` §2.5 に手順を追記。

## mock 検証済み（偽 Host・偽 transport。外部実行なし）
- `cargo test -p celeris-campaign --test preflight` 6 件: 完備で通る／版 placeholder・alias・C routing 欠落／予算超過・並列 2・token_basis／時間不整合／認証欠落（環境変数・A の実行ファイル・URL path・URL 欠落・template placeholder）／材料 hash 不一致・材料欠落／grade-case 欠落・改変・隠しテスト写し欠落が、それぞれ不足として出る。
- 「不足 1 件（B の版が placeholder）で transport 呼び出し 0 回・out/台帳を作らない」（`one_gap_means_zero_ledger_and_transport_calls`）。
- CLI: 完備で exit 0・`paid_calls_made` 0、環境変数欠落で preflight と `run --executor celeris` が exit 1 で out を作らず、秘密値（試験用の文字列）が stdout/stderr に出ない。
- 既存 `cargo test -p celeris-campaign` は全 pass。

## 実経路未
- claude-code の実際の版と manifest の `harness_version` の一致、token が API に受理されるか、C の evidence table id が本番に実在するか、Jev の鍵は見ていない（ネットワーク・LLM CLI を起動しない方針）。本測定の初回 run で確認する。
- 修正 1 の grader 結合キー（`definition_sha256` 等）は本 branch に無く、manifest への結合検査は未実装（grade-case の固定 hash とテスト実体の sha256 までは検査）。

## 承認待ち（人）
- H1 本番昇格（release sha12 の確定）、A の claude-code 版・C の evidence table id の置換、`CELERIS_CAMPAIGN_TOKEN` の設定、そして本測定そのもの（有料実行）は人が行う。preflight 通過は実行の許可ではない。

## 提案
- 修正 1 の結合キーが入ったら、`check_grader` に manifest 側の定義 hash との一致検査を足す。

## 証拠コマンド
- `cargo test -p celeris-campaign --test preflight` → 6 passed。
- `bash scripts/dev/test-parallel.sh` → exit 0（passed 5544, failed 0, ignored 15）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
