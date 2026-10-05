---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: cli
status: done
completed: 2026-10-05
---

# Phase 5 cli: celerisctl routing evaluate の estimator 比較出力（--policy estimator）

## 変更

- `crates/celerisctl/src/commands/routing.rs`:
  - `EvalPolicy::Estimator` を追加（`policies` に入らない比較欄の key。`report_key` は `Option` に）。
  - `RoutingEvaluateArgs` に `--estimator <json>`（pin した `EstimatorDescriptor`）と
    `--pair <json>`（pin した `RouteLlmPairV1`）を追加。`--estimator`/`--pair` は `--policy estimator`
    が無いと拒否（dataset の読み込み前に、DB も HTTP も使わない）。
  - `run_evaluate` は `routing_replay::evaluate_with_estimator` を使う（従来 `evaluate` ≒ estimator=None。挙動不変）。
    `--policy estimator` を指定したのに dataset に estimator shadow が無く pin も無ければエラー。
    `--policy estimator` のみなら `policies` は空、`estimator_comparison` 欄だけが入る。
  - `render_report_markdown` に `## Estimator shadow comparison` 節を追加
    （`render_estimator_comparison_markdown`: pin した descriptor/pair・calibrated・観測 estimator versions、
    target/evaluated/coverage、completed/failed/timeout/dropped/prompt_required、overhead mean/p50/p95、
    heuristic primary との一致/差・unavailable、primary との一致、quality observed/unknown/acceptance success、
    unknown 理由と incomparable reasons の内訳表。JSON と同じ値だけを並べる）。
- 変更は読み取り専用: DB は `routing export` の読み取り専用接続のみ、evaluate は DB を開かず HTTP も呼ばない。

## 証拠

- `cargo test -p celerisctl --test routing_cli` → 3 passed
  （`routing_cli_estimator_report_is_read_only_and_reproducible`: 2 回の export/evaluate で report.json と
    report.md がバイト一致、DB と -wal/-shm/-journal の中身・mtime 不変、`--estimator` なし `--policy estimator`
    なしは拒否、policies 空 + estimator_comparison の値・markdown の欄を検証。
    `routing_cli_export_evaluate_is_read_only_and_reproducible` は従来どおり pass）。
- `cargo test -p celerisctl` → 131 passed（既存の routing_cli 3 件を含む）。
- `cargo clippy -p celerisctl -p task-ops --all-targets -- -D warnings` → exit 0。

## 未解決事項

- `--estimator`/`--pair` の JSON は `EstimatorDescriptor` / `RouteLlmPairV1` をそのまま serde で受け取る
  （スキーマは task-core の sidecar module。runbook unit が pin の作り方を docs/ops に書く）。
- estimator の「候補 policy 扱い」（例: pair 校正後に estimator 選択を policy として評価）は本 unit 的范围外。
  shadow → opt-in の切り替えは ADR 通り本 Phase では行わない。
