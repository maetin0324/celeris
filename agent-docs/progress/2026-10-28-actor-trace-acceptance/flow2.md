---
title: 行為者と消費の通し試験 流れ2（人へ回した要求の判断者と実行者の分離）
tasks: [01M60AKVZEB61PRBKX5AX44H3A]
status: done
updated: 2026-10-28
---
# 行為者と消費の通し試験 流れ2

完了日: 2026-10-28

## 変更

- `phase 2: 流れ2（人へ回した要求の判断者と実行者の分離・取り次ぎ）の受け入れ試験と台帳の欠落修正`
- `crates/celeris/tests/actor_trace_flow2.rs`: 一時 DB・固定 UTC・既存の偽 HTTP（決まった CoS 応答と usage）で、3 件の decision を受信箱 → CoS の一次対応 run → escalation（人待ちの開始）へ流す。
  - D1: 人が web で答える（`human:web`）。
  - D2（B3）: 人が operator に伝え、operator が `chat_relay_post`（`operator:claude-code`、task 範囲、期限付き）で取り次ぐ。取り次ぎを入力にした CoS run が D3 の検査（期限切れ・別 task は拒否）を通り、`instructed_by` 付きの operation で答える。
  - D3: 人が operator に伝え、operator が自分の principal で答える。
- 照合: decision log export の介入は human 1・cos_project 1・external_operator 1（人の判断 1 を実行者の件数に置き換えない）。人待ちの区間（export の wait 行と台帳の human_waits）の終了主体は各実行者。台帳の D2 行は CoS operation と `instructed_by:<message>`・`relay:external_operator:claude-code` を持つ。human-ops report では 3 要求とも人の判断（decided_by=human、介入 3）で、cos/operator の op は `counts_as_human=false`（non_human: external_operator 2・cos_project 1）。人の時間は 9 秒、人待ちは 16 秒で、別々に数える。共有 helper `assert_actor_trace` で 3 出力の要求・operation・実行者を結ぶ。再投影しても 3 出力は変わらない。
- 製品側の欠落（小さいので同じ unit で修正）: 人へ回した item は escalated のまま operation を持たない。そのため、後から CoS が取り次ぎを根拠に答えた operation と `instructed_by`・`relay` の参照が request ledger から落ちていた。`task-core` の `request_ledger` で、対象の decision か受信箱の件が一致する operation を同じ要求に寄せる。ADR 付記: `2026-10-28-actor-trace-acceptance.md`（WU の範囲 check が他の ADR の変更を許さないため、付記はこの ADR に置いた）。修正を外すと流れ2が `operations: []` で落ちることを確かめた。
- `task-api` に D3 の検査に時刻を渡す入口 `cos::operations::inbox_answer_instruction_at` を追加した（route は壁時計のまま。内容は不変）。ADR 付記: `2026-10-28-actor-trace-acceptance.md`。
- `support.rs` は `human_record_base` を `pub` にしただけ（意味は不変）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `cargo test -p celeris --test actor_trace_flow2` → exit 0、1 passed。
- `cargo test -p task-core --lib request_ledger` → exit 0、3 passed（追加した `later_operation_on_an_escalated_request_is_linked_by_its_target` を含む）。
- `bash scripts/dev/test-affected.sh` → exit 0。Rust 6,636 passed・21 skipped、web 689 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決

- 試験の境界はドメイン処理から評価出力まで。HTTP の認証（operator だけが relays を呼べる 403 など）は既存の `crates/task-api/tests/relayed_instructions.rs` が受け持つ。
- decision log export（`decision_log_export_campaign`）自体は wait 行を含まない。`celerisctl campaign export` が `decision_log_human_waits` を足すので、試験も同じ関数で読む。

## 提案

- 台帳 ADR（`2026-10-27-eval-request-ledger.md`）と取り次ぎ ADR（`2026-10-27-relayed-instructions.md`）から、この ADR の流れ 2 の付記へ参照を足す。WU の範囲外なので統合後に人か closeout が判断する。

## 本番への移行

設定・DB migration・本番操作は不要。
