---
title: "main-4 主比較の契約（A0/B0・Jev 無効）と費用区分の文書化"
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: done
updated: 2026-10-25
---
# main-4 主比較の契約（A0/B0・Jev 無効）と費用区分の文書化

完了日: 2026-10-25

WorkUnit `ab-contract`。基準は `$CELERIS_WU_BASE`。成果は `docs/ops/mlsys-main-4-ab-contract.md`（実装の変更なし）。

## 変更
- `phase 1: main-4 の A0/B0 主比較契約と費用区分を文書化` — 次を書いた:
  - 人の決定で後から入る欄（固定モデル名 d-model・Celeris 版 d-release・A の版と実体・測定 ID）。
  - A0・B0・Jev 無効の定義と、課題・公開資産・前置き・モデル・harness 版・環境・認証・tool・turn・役割・token 上限・壁時計・cluster job・予算・共有台帳・成功判定の差の表。
  - retry・timeout・予算停止・再開の意味。
  - token の 4 欄（input / cache read / cache write / output）の A/B 対応と欠測の規則。
  - 定価換算・実支出・harness 報告額・quota の 4 区分と出所、CoS と共有消費の範囲、欠測の扱い、比較可能でない値を削減の根拠にしない規則。
  - 未整合 U1〜U11。

## 照合した現物（読むだけ）
- K4 成果物 root の `revisions/judged-scope-v2/pinned/main/{final.json,pins.json,task-template.json,final-input-manifest.json,lock.json}` と `freeze-status.json`。
- `scripts/campaign/measure-launcher.sh`、`docs/ops/measure-run-final.md`、`docs/ops/experiment-sets.md`、`docs/ops/eval-raw-record-definitions.md` §7、`agent-docs/progress/2026-10-23-success-contract-human-ops.md`。
- 実装: `crates/campaign/src/{harness_exec,celeris_exec,final_accounting,export}.rs`、`crates/task-worker/src/claude_code.rs`（`usage_with_cost`・`--disallowedTools`）、`crates/task-dispatch/src/dispatcher/decision_log_wire.rs`（`list_cost_usd = u.cost_usd`、cash は欠測）、`crates/task-core/src/pricing.rs`、ADR 2026-10-20-mlsys-main2-fixes D1。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
検査結果は下の「検査」節に記録する。

## 未解決
- 文書の未整合 U1〜U11 は実装していない（WU の範囲外）。特に U6（`initial_info` が mlsys-main-1 の材料 hash のままで、lock の材料 hash と違う）は final-freeze で campaign hash を作り直さないと checker が違反にする。
- U1（A の定価換算が無い）と U3（A の subagent 消費）は、費用比較の前に直す実装が要る。

## 提案
- U1: final-accounting で、decision log の無い arm（A）の試行 token 4 欄に B と同じ単価表を掛けた list を出す。export の `cost_micro_usd_sum` は出所（harness 報告 / 単価表）を列で分ける。
- U4: preflight で A の `A_PROGRAM` と測定 daemon の `[adapters.claude_code] command` の sha256・版の一致を検査する。
- U6: final-freeze の段で `initial_info` を `snapshot.materials_sha256` に合わせる。
