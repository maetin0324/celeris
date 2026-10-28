---
title: 機械的に解けない統合の衝突を、CoS が integrate task として依頼する
updated: 2026-10-28
status: done
tasks: [01M60AMT97YPAFYRT9H0E3F1Z5]
---
# 機械的に解けない統合の衝突を、CoS が integrate task として依頼する

完了日: 2026-10-28。
基準: `git merge-base HEAD main`。ADR: `agent-docs/adr/2026-10-28-cos-integrate-task.md`。

## やったこと

- ADR `agent-docs/adr/2026-10-28-cos-integrate-task.md` を実装前に書いた（D1〜D5）。
- task-core: `Event::IntegrateTaskRequested` / `IntegrateTaskSettled`、`integration_request::{INTEGRATE_TASK_OPTION, integrate_task_link}`。
- task-ops: `integrate_task`（起票内容 `spec`、結び `link_for`、決着 `settle`、人に回す理由 `escalated_reason`）。受信箱の統合の依頼に
  選択肢 `integrate_task` と native を足し、結んだ integrate task が進行中の間は出さず、失敗した依頼は題と本文で示す。
- task-api: `POST /api/v1/tasks/{id}/integration-requests/{request}/integrate-task`（`create_task_linked_op` で起票と結びを 1 transaction）。
  CoS 登録表 tasks に `integration.integrate_task`。受信箱の answer（人・CoS）の `integrate_task` は native に委ねる。
  `human_required_reason` は失敗して人に回した依頼で理由を返す。
- celeris daemon: stranded sweep の後で `task_ops::integrate_task::settle` を呼ぶ。
- skill: `config/skills/cos-operator/operations.md` の表、`cos-inbox-triage/SKILL.md` の規則、`SOURCE.md` の履歴。
- 文書: `docs/api/v1/api.md`（改訂・表・CoS 表）、ADR 2026-10-09-cos-operations-all-mutations の tasks 表と付記、`docs/architecture-map.md`。
- 生成物: `docs/api/v1/api-v1.schema.json`・`event.schema.json`、`web/api/generated/*`、`web/api/realtime/{event-kinds,invalidation-map}.ts`。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `cargo test -p task-api --test cos_integrate_task`: 3 passed（衝突 → CoS の resolve で起票 → reviewer 合格・main の ff → settle で依頼が閉じる、
  失敗 → 受信箱に戻り CoS の answer は 403 `cos_human_required`・再起票は 409 `integrate_task_escalated`・escalate は可、origin 違い・不明な依頼の拒否）。
- `cargo test -p task-ops --lib integrate_task`: 3 passed。`cargo test -p task-ops --lib human_inbox`: 24 passed。
- `bash scripts/dev/test-affected.sh`: exit 0、6639 tests run: 6639 passed, 21 skipped。web typecheck と unit も通過。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決事項

- integrate task が done でも main に入らない（案件の `auto_integrate = false` など）と、元の依頼は隠れたまま残る。integrate task 自身の
  取り込み待ち（`unintegrated`・統合の依頼）が受信箱に出るので、人はそちらで扱う。
- 元の task の配送（`Blocked` `[needs-human]`）の状態は変えない（人が「統合した」と答えた場合と同じ扱い）。取り込み状態は stranded sweep が
  `integrated` にする。
- 試験は integrate task の worker run を走らせず、merge と遷移を試験の中で行う（fake harness の run までは通していない）。

## 本番への移行

- skill の変更は本番 KB の `skills/` に人が取り込む: `celerisctl skills import config/skills --name cos-operator --name cos-inbox-triage --root <kb_root>`。
  取り込むまで CoS は新しい操作を知らない（API と受信箱の選択肢は release で入る）。
- config の欄・migration の変更は無い。

## 提案

- 統合の依頼が出たとき CoS が既定で `integrate_task` を選ぶかは cos-inbox-triage の規則に書いた。人の承認を挟みたい案件があれば、
  label `human_required` で従来どおり人に回せる。
