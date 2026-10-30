---
title: K4 追試の証跡確定 — WU human-cov（人の時間の欠測・下限と coverage、外部 operator の unknown 区分）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
---
# K4 追試の証跡確定 — WU human-cov

完了日: 2026-10-30

ADR: `agent-docs/adr/2026-10-30-ops-r1-human-time-coverage.md`。

## 変更
- `human-cov: 人の時間の欠測・下限と coverage、外部 operator の unknown 区分を足す`
  - `crates/campaign/src/human_ops_report.rs`: 試行・arm・全体に `human_time_coverage`（`bound` = exact/lower_bound/missing、
    `union_secs`（missing なら null）、記録のある試行の割合 `trial_ratio`、区間のある人の op の割合 `op_interval_ratio`）。
    既存の `lower_bound`・`human_time_missing_trials`・`human_time_lower_bound_trials` は再実装せず、同じ判定の上に足した。
    report.md の arm 表は全欠測の arm の人の時間を `0` と書いていた（`cell()` の既定値）ので「欠測」と書くよう直し、被覆の節を足した。
  - `crates/campaign/src/operator_usage.rs`: `Category::Unknown`（session-map の `"unknown"`）。trial_key は有っても null でもよい。
    shared・shared_unattributable に読み替えない。
  - 文書: `docs/ops/operator-usage.md`（区分 unknown）、`docs/ops/eval-human-ops-runbook.md`・`docs/ops/eval-raw-record-definitions.md`（coverage）。
  - 試験: `crates/campaign/tests/human_ops_report.rs` の `unrecorded_human_time_is_missing_with_coverage_not_zero`、
    `crates/campaign/tests/operator_usage.rs` の `unknown_category_is_kept_apart_from_shared`。

## unknown 区分の扱い（受け入れ 1）
- session-map の区分を検証する crate のコードは `crates/campaign/src/operator_usage.rs` の `validate_map` と `Category`（serde、
  未知の区分はエラー）にある。`ops_r1_attr` は測定 daemon の DB から帰属を出す export で、session-map を読まない。
- そこで実装した: `unknown` を受け付け、`unknown` のまま operator-usage.jsonl に書く。shared に寄せない。空の trial_key は拒否する。
- 実データ（追試 P の `operator/session-map.json`）の各区間を `unknown` に分け直すかどうかは WU support-audit の担当で、この WU では
  session-map も operator-usage.jsonl も変えていない。

## 時刻・主体の無い人の発言（作業 4・objective (3)）
- 既存の挙動を確かめ、変えずに試験で固定した。始点の無い人の op は `missing_start` で長さに入らず、coverage の
  `ops_without_interval` に数える。主体の分からない op は `actor.kind=unknown` で人の時間に入れず `unknown_actor` に残す。
  記録 CLI（`human-op record`）は `--actor-kind` を必須にしており、主体の既定値を持たない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p celeris-campaign --test human_ops_report --test operator_usage` → 15 passed（新規 2 件を含む）
- `bash scripts/dev/test-affected.sh` → exit 0（2862 passed、9 skipped。対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 親 R・追試 P の human-ops への新 binary の適用（coverage の実値）は、この WU では行っていない（WU regrade・report）。
- report.md の試行別の表は、記録のある試行の lower_bound を union の数値だけで示す（bound は report.json の試行の `human_time_coverage` にある）。

## 提案
- 報告第 3 版は人の時間を `human_time_coverage.bound` と `trial_ratio`・`op_interval_ratio` を添えて書き、`exact` 以外の値を
  総作業時間や削減量として書かない。
