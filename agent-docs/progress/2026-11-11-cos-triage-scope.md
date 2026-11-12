---
title: 受信箱の一次対応で CoS が答える範囲を「人に残す件の列挙以外すべて」にし、Celeris の外の操作は operator 宛てに分ける（skill v12）
tasks: [01M75722SX158YRMFYPQRVEEDY]
status: running
updated: 2026-11-12
---
# 受信箱の一次対応で CoS が答える範囲を広げ、operator 宛てを分ける（skill v12）

完了日: 未完（attempt 1 は途中で区切った）

設計判断は [ADR 2026-11-12 cos-triage-scope](../adr/2026-11-12-cos-triage-scope.md)。基準は `git merge-base HEAD main`。

## 変更（attempt 1）
- `phase 1: CoS の一次対応の範囲を人に残す件の列挙以外に広げ、operator 宛ての経路を足す（ADR・basis・照合・skill v12）`
  - ADR 2026-11-12-cos-triage-scope（D1 人に残す件の列挙、D2 operator 宛て、D3 task_scope と根拠、D4 決定的な照合、D5 常設の節、D6 計画の承認と決定、D7 撤去）。
  - task-api `cos::basis`: `BasisCategory::DocumentedDefault`・`WorkGuidance` を消し `TaskScope` を足した（refs に根拠が要る。`BasisRefKind::Task` を足した）。
    認可（authorization）への回答は `job_wait_extend`・`within_approved_frame` だけにした。監査の reason は「CoS の判断（<category>）」。
  - task-ops `triage_match`: 件の種類と本文・ledger の grant から `reserved`（人に残す件）と `candidates`（定型の候補と grant）を決定的に出す。LLM なし。
  - task-api `cos::inbox`: escalation packet に必須の `to`（`human`・`operator`）。`operator` は経路だけ取り、通知（Discord）と統括の札を作らない。
    `reserved` の件への answer と operator 宛ては 422 `cos_reserved_for_human`。`GET /api/v1/cos/inbox` の item に `addressee`。
  - task-core migration 0124: `cos_notification_routes.route` に `operator` を足した（表の作り直し、行は写す）。
  - skill cos-inbox-triage を version 12、`[cos.triage] policy_version` の既定を "12"（cos.rs・example.toml・docs/ops/cos-chat.md）。

## 証拠（attempt 1。提出時の HEAD ではない）
- `cargo test -p task-ops --lib triage_match` → 2 passed。
- `cargo test --no-fail-fast -p task-api --test cos_routine_answers --test cos_triage --test cos_chat_triage_scenarios --test cos_operations --test cos_review_rework --test cos_integrate_task --test cos_plan_conforms --test cos_job_wait_extend --test authority_ledger --test single_human_window --test cos_operations_domains --test project_wake_human_chat` → 12 本すべて合格（22・3・4・4・8・7・4・4・5・17・13・1 件）。
- test-affected・clippy・fmt・progress-index は未実行（次の run で最終の HEAD で回す）。

## 未解決（次の run）
- 常設の節（task-worker `cos_chat::standing`）に v12 の要点を置き、prompt の件ごとに照合の行を出す（task-dispatch `inbox_context_item` で `triage_match::match_item`）。
- `/cos/operations` の答え（instructed_by なし）にも `reserved` の拒否を入れる。
- scenario 試験 (a)〜(f) を `crates/task-api/tests/cos_chat_triage_scenarios.rs` に足す（(f) は operator 宛てで outbox・notifications が 0 件）。
- 計画の承認の再判断（D6）: daemon の「決定の回答による計画の再判断」を CoS の回答でも起こす。
- 依頼のコメントの 2 件: dependents_reopened に「取り消さない」の答え方を持たせる／release の切り替えで一次対応が途切れた件の拾い直しを確かめる。
- API の JSON schema と web の生成型の更新（EscalationPacket.to・CosInboxItem.addressee・BasisRefKind.task）。

## 本番への移行
1. 配備の後、`~/.config/celeris/config.toml` の `[cos.triage] policy_version` を `"12"` にする（今は "11"）。
2. KB の skill を v12 に差し替える: `celerisctl skills import config/skills --name cos-inbox-triage --root <kb_root>`。
3. `GET /api/v1/system/health`（または CoS の run の policy_version）で 12 になったことを確かめる。版が違うと resolve は 422 `cos_policy_version_mismatch`。
4. migration 0124 は daemon の起動で当たる（`cos_notification_routes` の作り直し。行は消えない）。

## 提案
