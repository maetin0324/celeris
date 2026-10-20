---
title: "人の介入・時間・再作業・消費の比較表とタイムラインの再生成（研究評価接続 4）"
tasks: [01M5BT8CQ5E85VN108BTA1Y9WN]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# human-op report

`human_ops_join.rs` を再利用し、`crates/campaign/src/human_ops_report.rs` に `celeris-campaign human-op report` を追加した
（`report.json` / `report.md` / `timeline.jsonl`）。join に TrialRecord 由来の outcome・failure_reason・tokens・cost_micro_usd を足した。
手順は `docs/ops/eval-human-ops-runbook.md` §6.1。

## 証拠
- `cargo test -p celeris-campaign --test human_ops_report --test human_ops_join`: 4 passed（重複・重なり・待機・再開・途中終了・cutoff を含む fixture と byte 一致の再生成）
- `bash scripts/dev/test-parallel.sh` exit 0（passed 6189, failed 0）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0

## 未解決・実データ未確認
- 3（実 pilot）の記録では通していない。実装/fixture 合格のみ。
- 失敗原因は暫定 schema。2 の分類確定後に `failure_cause` の写像を合わせる。
- internal_wait（ready_not_running）は export に無く欠測。共有 CoS overhead は identity が無ければ配分しない。

## 提案
- ready_not_running を export に出すか決める（現状は欠測）。
