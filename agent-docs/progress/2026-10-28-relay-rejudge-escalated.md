---
title: 取り次ぎ後に escalated の件を CoS が再判断する・範囲検査の件 → task の写像
tasks: [01M60JC1QB4K9MD2Z73CYMQB9Y]
status: done
updated: 2026-10-28
---
# 取り次ぎ後に escalated の件を CoS が再判断する・範囲検査の件 → task の写像

完了日: 2026-10-28

ADR: [2026-10-27-relayed-instructions](../adr/2026-10-27-relayed-instructions.md) 付記 A2（R1〜R3）。

## 変更
- `phase 1: 取り次ぎ後に escalated の件を CoS が再判断し、範囲検査は消えた件も task へ写す`
  - task-core `chat/relay_rejudge.rs`: `cos_triage_relay_candidates`（件を判断した一次対応の入力より後の seq の、未取消・期限内の取り次ぎ）と
    `cos_triage_relay_claim`（同じスレッドに live な run があれば claim しない。入力 message の 1 行目は従来の `item_ids=`、2 行目が
    `取り次ぎによる再判断: [{item_id, source_key, relay_message_ids}]`）。claim の書き込みは `triage.rs` の `bind_claim_tx` に共通化。
    `request_ledger` は入力の 1 行目だけを読む。
  - task-dispatch `cos_chat/relay_rejudge.rs`: tick で queued の発言の claim の後・新しい件の claim の前に、候補の件を受信箱の task
    （`item_tasks`）と取り次ぎの `task_ids` で突き合わせ、範囲内の件だけをスレッドごとに 1 run で渡す。範囲外の組は process 内で覚えて
    受信箱を毎 tick 作り直さない。
  - task-ops `human_inbox::item_task_id` と task-api `operation_target`: 生きた受信箱に無い件は件 id から task を引く
    （approvals・decisions・`<kind>-<task id>`）。06:15Z の事例は「applies to project …」の誤った拒否でなく 409 `cos_revision_conflict`（件はもう開いていない）になる。
  - `config/skills/cos-inbox-triage/SKILL.md` に再判断の入力の読み方を追加（policy_version は 6 のまま）。

## 本番で観測した事実（読み取りのみ）
- 本番 DB の `cos_operations` 01M60JC3GW…: 06:15:17Z に rejected。対象の質問は 06:14:33Z に `operator:claude-code` が回答済み
  （task 01M5XDTAQQ の Answered event）。件が受信箱から消えていたため対象の task が引けず、案件の範囲外として拒否されていた。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo test -p task-core --lib relay_rejudge` → 2 passed
- `cargo test -p task-dispatch --lib cos_chat_triage_relay` → 1 passed（fake harness。走っている一次対応 run の間は再判断しない、取り次ぎ自身の run が先、範囲内の件だけを取り次ぎ id つきで 1 回）
- `cargo test -p task-api --test relayed_instructions` → 5 passed（再判断の run が instructed_by=取り次ぎで回答、2 度目は 409、閉じた authorization の件の写像）
- `bash scripts/dev/test-affected.sh` → exit 0（6653 passed, 21 skipped）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 本番への反映後、BenchFS スレッドで escalated の件に取り次ぎが届いたときに再判断の run が起きることは未確認（実機での確認は昇格後）。
- skill の本番 KB への取り込み（`celerisctl skills import`）は人が行う。

## 提案
- なし
