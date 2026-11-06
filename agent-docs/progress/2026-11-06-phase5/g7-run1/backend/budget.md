---
title: 案件予算に起動前除外数と fail closed を反映する（budget）
tasks: [01M6RJTQQ629D0NP953708JMS4]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# budget（Phase 3）

完了日: 2026-11-06

正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md) D1。新しい設計判断はなく、ADR は足していない。

## 変更（commit は件名で列挙）
- `phase 3: 案件予算に起動前除外数と fail closed を反映する`

変更ファイル: `crates/task-ops/src/project_budget.rs`、`project_budget_tests.rs` のみ。
- `BudgetUsage.not_launched_runs`（新欄）: `project_not_launched_runs_between` で数える。費用不明にも USD の和にも入れない。
- 和は usage-query の `project_run_usage_between` をそのまま使う（起動前に終わった行は返らない）。判定は store の 1 つの述語で、ここに複製していない。
- attempt 欠測の run は `usage=None` で費用不明のまま（guard が出る）。読めた分は `known_usage` から tokens と USD（下限）に足し、失わない。
- task run の扱い、価格表、cache の換算、日付範囲、`exceeded`/`severity` の順序は変えていない。公開 controller DTO（`budget_today`）は次 unit。

## 修正前の回帰
- コマンド: `cargo test -p task-ops --lib attempt_with_missing`（試験だけを足した状態）
- exit: 101。assertion: `既知の USD を失わない（下限として足す）  left: 3000000  right: 6000000`（attempt 欠測の run の読めた USD が和から落ちる）。
- 起動前除外の試験は `not_launched_runs` 欄が無くコンパイルできない状態から始めたため、assertion の記録は上の 1 件。

## 修正後（受け入れ条件 0・1）
- `cargo test -p task-ops --lib project_budget`: 11 passed, 0 failed。追加した 4 試験が固定するもの:
  - 起動前 6 reason だけなら unknown guard なし・`exceeded=None`・severity Daily、既知 USD・token は保持。
  - 起動後 usage 無し・usage 破損・未知 reason・reason 無し・attempt 一部欠測・旧行（launch_tracked=0）は、1 件で `UsdUnknown`・Dangerous。除外数は変わらない。
  - `not_launched_runs` は案件ごと・UTC 日ごと（`00:00:00Z` を含み翌日を含まない）。別案件を数えない。
  - task run の除外規則（全部無しは除外、model ありで usage 無しは不明）は不変。
- 提出時の HEAD で実行（`git merge-base HEAD main` 以後、最後の同期の後）:
  - `bash scripts/dev/test-affected.sh`: exit 0（約 13 分。14 crate の nextest・doctest・web 型検査/unit を含む）。
  - `cargo clippy --workspace -- -D warnings`: exit 0。
  - `cargo fmt --all -- --check`: exit 0。
  - `cargo nextest run -p task-ops`: 646 passed, 1 skipped。

## 未解決・提案
- `budget_today.not_launched_runs` の公開は api-contract WU（task-api の `handlers/project_controller.rs` はまだ `usd_unknown_runs` までしか写さない）。
- attempt 欠測 run の USD を下限として足す扱いは、上限判定を早める方向（fail closed 側）。反対の意図なら ADR で決めたい。
