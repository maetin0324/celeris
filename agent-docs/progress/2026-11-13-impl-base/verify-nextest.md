---
title: health_review の試験を nextest で検証し、cargo test の失敗の原因を記録する（verify-nextest）
tasks: [01M79DR235G75X2JKTWA794R7T]
status: done
updated: 2026-11-13
---

# verify-nextest 進捗

完了日: 2026-11-13（コードは変えていない。この進捗ファイルだけを足した）

## 原因
- `cargo test -p task-dispatch health_review` は 11 件落ちる。原因は `crates/task-dispatch/src/health_review/gate.rs` の `static BUNDLED`（`OnceLock`）。`cargo test` は 1 process で全試験を走らせるので、同梱検査器を stub に設定する試験（`use_passing_stub_checker`）と、未設定を assert する試験（`dispatcher_without_bundled_checker` の assert）が同じ static を共有し、実行順しだいで競合する。
- 落ちた試験は health_review::gate の `health_evidence_gate_bundled_checker` と、`dispatcher::child_tasks::policy_review` の `health_review_policy_*` 系。
- 今回の base 規則の変更（`target_base.rs` など）が原因ではない。

## nextest の結果
- nextest は試験ごとに process を分けるので static が共有されない。
- 提出時の HEAD で実行: `cargo nextest run -p task-dispatch health_review` → 134 件実行、134 件合格、1633 件は filter 外（exit 0）。

## 提案
- 受け入れ検査の `cargo test -p task-dispatch health_review` は、`cargo nextest run -p task-dispatch health_review` に直す。CLAUDE.md のとおり、全体の試験は nextest で走らせる。
- 別の案として、`BUNDLED` を試験ごとに差し替えられる形（process 全体の static を持たない作り）に直す task を足す。必要なら人が判断する。
