---
title: "campaign 実実行アダプタ（executor）: CLI --executor・ADR 追補・全体検査"
tasks: [01M51QR9CCRP89GWXZDR4XB3S6]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# executor（WU cli-adr）

## 実装済み
- `--executor mock|harness|celeris`（`crates/campaign/src/main.rs`）。未知・未指定はエラー。`celeris` は `--api-url` と `--token-env`（環境変数名）のみ必須、任意で `--task-template`。秘密の値は出力・記録しない。
- `dispatch.rs`（`ArmDispatch`: A→HarnessExecutor、B/C→CelerisExecutor）、`curl_transport.rs`（`curl` 子 process、token は stdin の設定で渡し argv に出さない。新しい依存なし）。
- run 出力に `ledger: {rows, by_state}` を追加。
- ADR `2026-10-15-mlsys-min-campaign` に追補 D8〜D13（台帳キーと永続化順序、routing 固定、A 無制限、Wait と workspace、子 task/cluster job と cancel、秘密）。
- 先行 unit の成果: ledger / task-routing / adapter-a / adapter-bc（各 progress 参照）。

## mock 検証済み
- `tests/cli_executor.rs`（3 件）: mock の台帳要約、未知 executor の拒否、celeris の引数不足・環境変数欠落で秘密の値が出ないこと。
- 先行 unit の試験: 投入直後の応答喪失・アダプタ再起動・二重起動で 1 task のみ（`celeris_exec.rs` 7 件、`ledger_async.rs`、`harness_exec.rs`）、Wait 中の workspace 保持、子 task・cluster job 追跡と cancel。
- `curl_transport.rs` の unit 試験（名前・URL 検査、引用符エスケープ）。

## 実経路検証済み
なし（実 API・LLM・クラスタは呼んでいない）。`CurlTransport` の実 HTTP 往復も未実施。

## 未実施
- 実 task-api に対する `--executor celeris` の通し（認証のある環境で、承認済み予算の範囲）。
- `claude` ハーネスの実起動（`--executor harness`）。

## 未解決事項・注意
- `harness_exec::runner_drives_three_arms_with_stub_and_resumes_without_restart` は全体並列実行の高負荷時に一度落ちた（単独実行は合格）。タイミング依存の疑い。再発するなら別途調べる。

## 提案
- task-api の `POST /tasks` に `idempotency_key` 欄を足せば、label 代用と事後の最古寄せが不要になる。

## 証跡
末尾の「検査結果」参照。

## 検査結果
- `bash scripts/dev/test-parallel.sh` → exit 0、passed 5419 / failed 0 / ignored 13
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 範囲外の既知事項
- `scripts/dev/check-doc-links.sh` は `crates/task-core/src/execution_plan/tests.rs:1544` の `docs/ops/x.md` を壊れ参照として報告する（この WU の base 由来）。FOREIGN_DOCS への 1 語追加で直るが scripts/ は本 WU の範囲外なので触らない。integrate-wire で直すこと。
