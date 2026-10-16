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

## 冪等な試行台帳（ADR 追補 D8）
- **実装済み**: key は (manifest sha256, problem id, arm, repetition)。外部へ渡す冪等キーは `campaign-<sha256 hex>`。投入前に planned（routing 設定込み）を fsync 付きで台帳へ追記し、冪等キーで照会（lookup）→ 無ければ投入 → task/run ID を追記（submitted）→ 外部待ちなら waiting、の順。台帳は追記専用。投入成功後に応答が失われても、再起動後の照会で同じ task に辿り着き、重複投入しない。task_id が無い planned 行は照会から再開する。
- task-api に冪等キー欄が無いため label（先頭 64 文字）で代用し、競合時は最古の task へ寄せて新しい方を cancel する（提案: `idempotency_key` 欄）。
- **mock 検証済み**: 投入直後の応答喪失・アダプタ再起動・二重起動で同じ試行が 1 つの task/run にだけ対応する試験（`celeris_exec.rs`、`ledger_async.rs`、`harness_exec.rs`）。
- **実経路検証済み**: なし。**未実施**: 実 task-api に対する冪等性の通し。

## キャンセル方針（ADR 追補 D11・D12）
- **実装済み**: 打ち切り（試行 timeout・予算停止）では、終端でない子孫 task を深い順 → 根の順に `POST /tasks/{id}/cancel` する（409=既に終端は成功扱い、全対象を試してからエラーを集約）。`track`/`poll` は `GET /tasks/{id}` の `children` を幅優先で辿り、子 task ID と cluster job（`scheduler:cluster:job_id`）を台帳へ積み増す。根が done でも子孫と cluster job が全て終端になるまで Wait。
- クラスタ job は自動でキャンセルしない。`scancel`/`qdel` の人手手順を `out/cluster-cancel.jsonl` に追記する（`executed:false`）。
- Wait 中の試行の workspace は台帳が参照している間 runner が消さない（`Ledger::is_referenced`）。
- A（harness）は試行 timeout で process group を終了する。再起動後も台帳の pid・start ticks で同じ process を追跡する。
- **mock 検証済み**: Wait 中の workspace 保持、子 task・cluster job の追跡と task のキャンセル。
- **実経路検証済み**: なし。**未実施**: 実 task-api でのキャンセル、実クラスタ job の人手キャンセル手順の実行。

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
