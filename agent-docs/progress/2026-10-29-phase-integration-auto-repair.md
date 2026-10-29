---
title: 段の統合の衝突を人を待たずに自動で解消する
tasks: [01M6320TM5XCF2QA8XXTT57BRE]
status: done
updated: 2026-10-29
---
# 段の統合の衝突を人を待たずに自動で解消する

完了日: 2026-10-29

ADR: [2026-10-29-phase-integration-auto-repair](../adr/2026-10-29-phase-integration-auto-repair.md)

## 変更
- `phase 1: 段の統合の衝突を merge の repair WU で自動に解き、尽きたときだけ人に回す`
  - `task-dispatch` `phase_integration.rs`: 衝突（auto_resolve の `NeedsHuman` を含む）は統合の依頼にせず merge の repair WU に渡す。
    repair の objective に依頼の markdown（両側の commit・理由）を足す。上限は衝突した WU ごとに 2 回（`MAX_MERGE_REPAIRS_PER_UNIT`）。
    上限に達したときだけ、理由「自動の解消が失敗: …」を付けた統合の依頼を記録する。
  - `phase_integration/merge_repair.rs`（新）: merge の repair WU の数え方と、失敗時に replan せず `cancelled` にして統合 WU を
    `pending` に戻す `retire_if_failed`（`worker_finish.rs` から 1 行で呼ぶ）。
  - `task-ops` `integrate_task/phase.rs`（新）: 依頼で止まった統合 WU の特定・再開（受信箱の回答 API と共有）、古い依頼を自動の
    解消へ戻す `retry_unrepaired_requests`。`celeris` の stranded sweep から呼ぶ。
  - `task-api` `inbox_notifications.rs`: 段の依頼の回答を共有関数に置き換えた。
  - ADR 新規 1 本、ADR 2026-10-28-cos-integrate-task と 2026-10-02-parallel-integration-auto-resolve に付記、`cos-inbox-triage` の規則。
- `phase 1: 長い TMPDIR と共有 process で落ちる celeris の試験を直し、進捗を書く`
  - `browser_doctor_tests.rs`: unix socket を短い dir（`/tmp` の下の tempdir）に置く（run の TMPDIR は 93 字で `SUN_LEN` を超える）。
  - `classify_stage/tests.rs`: `set_default` の後に `tracing::callsite::rebuild_interest_cache()`（同じ process の他の試験が
    callsite の interest を無効で覚えていると log が出ない）。

## 試験
- `dispatcher::tests::work_units::phase_integration_conflict_is_resolved_by_a_merge_repair_without_a_person`（新）: 内容衝突 →
  repair WU が両側を残して解く → 統合再開 → 最終 review（検査と reviewer の合格）→ done。統合の依頼は出ない。
- `phase_integration_request_is_the_only_inbox_item`（改）: repair WU が 2 回失敗 → replan せず統合の依頼 1 件、理由と受信箱の本文に
  「自動の解消が失敗」と repair WU の key が出る。
- `task-ops` `sweep_hands_unrepaired_phase_requests_back_to_the_automatic_repair`・
  `sweep_leaves_requests_after_the_automatic_repair_and_delivery_requests`（新）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 5448 passed、web の tsc・vitest も通過）
- `cargo clippy --workspace -- -D warnings` → exit 0（`--all-targets` も exit 0）
- `cargo fmt --all -- --check` → exit 0
- `cargo test --workspace --no-fail-fast`（run の TMPDIR、93 字）→ exit 101、6672 passed / 104 failed。失敗は全て unix socket の
  `SUN_LEN` 超過とその連鎖（credentiald・task-worker の browser・task-api の browser 試験・celerisctl）。
- 同（TMPDIR を workspace 直下の短い dir にした）→ exit 101、6746 passed / 30 failed。残りは `celeris-credentiald` の
  broker・injection 系、`task-worker` の browser launcher と `process_group`、`task-api` `browser_waits` の 1 件で、この変更が触れていない
  crate の sandbox 由来の失敗（KB: credentiald broker 試験は sandbox でのみ失敗、worker sandbox は userns 不可）。main でも同じかは未確認。

## 本番への移行
- config・migration の変更は無い。配備（release・promote）は人が行う。
- 昇格後、最初の stranded sweep（起動直後）が依頼 01M62ZEN0WPZ0YY64N8YGFGPPF（task 01M62S8XWCZDCY2ZBVQ35176MH の `integrate-fix`）を
  `retry`（by daemon）で閉じ、統合をやり直す。衝突は repair WU `merge-fix-<unit>` が解く。確認: task の events に
  `IntegrationAnswered { answer: "retry", by: "daemon" }` と `RepairScheduled { class: "merge_conflict" }` が出ること。
- `config/skills/cos-inbox-triage` の変更は本番 KB の `skills/` に人が取り込む。policy_version は 6 のまま。

## 未解決
- 木の親 task（子が kind task の unit）の段の統合で repair WU が走る経路は、コードを読んで同じ経路と判断した。実 daemon での確認は未実施。
- `cargo test --workspace` を sandbox の外で exit 0 にできるかは未確認（上の 30 件）。

## 提案
- run の TMPDIR を短くする（例 `runs/<run>/t`）か、unix socket を bind する試験を共通の短い dir の helper に寄せる。
