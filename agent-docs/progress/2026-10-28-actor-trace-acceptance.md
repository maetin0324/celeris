---
title: 行為者と消費の追跡を通しの流れで確かめる受け入れ試験
tasks: [01M60AKVZEB61PRBKX5AX44H3A]
status: done
updated: 2026-10-28
---
# 行為者と消費の追跡を通しの流れで確かめる受け入れ試験

完了日: 2026-10-28

5 つの流れの試験が統合後の branch にそろい、`bash scripts/dev/test-affected.sh` の通常の celeris integration test として走って通る。
設計は [ADR 2026-10-28-actor-trace-acceptance](../adr/2026-10-28-actor-trace-acceptance.md)（状態: 実装済み）。
WorkUnit ごとの詳細は `agent-docs/progress/2026-10-28-actor-trace-acceptance/` の harness.md・flow2.md〜flow5.md にある。

## 変更（commit の件名）

- `phase 1: 行為者と消費を評価出力まで照合する受け入れ試験を追加`、`phase 1: 共有照合helperと受け入れ試験の検証結果を確定` — ADR、共有 helper `crates/celeris/tests/actor_trace/support.rs`、流れ 1。
- `phase 2: 流れ2（人へ回した要求の判断者と実行者の分離・取り次ぎ）の受け入れ試験と台帳の欠落修正` ほか流れ 2 の記録 3 件。
- `phase 1: 案件横断の停滞から統括CoSと人の判断を照合する試験を追加` — 流れ 3。
- `phase 1: 配備とrollbackの監査を判断記録まで照合する試験を追加` — 流れ 4。
- `phase 1: 再配送と担当の再起動で操作も集計も増えないことを確かめる流れ5の試験を追加`、`phase 1: 流れ5の進捗を書き、clippy の指摘を直す` — 流れ 5。
- closeout: ADR の状態を実装済みにし、`docs/architecture-map.md` に通しの試験の入口を 1 行足し、この進捗を書いた。統合で壊れた所は無く、コードは変えていない。

## 5 つの流れの結果

| 流れ | 試験（`crates/celeris/tests/`） | 結果 |
|---|---|---|
| 1 認可内の要求を案件 CoS が処理 | `actor_trace_flow1.rs`（1 件） | pass。人の介入 0、cos_project の操作 1、usage の thread・案件・token・定価換算・壁時間と共有小計を照合 |
| 2 人へ転送し判断者と実行者を分離（B3 取り次ぎ） | `actor_trace_flow2.rs`（1 件） | pass。判断者は 3 要求とも human。実行者は human 1・cos_project 1・external_operator 1。人の時間 9 秒と人待ち 16 秒を別に数える |
| 3 案件横断の停滞 | `actor_trace_flow3.rs`（2 件） | pass。統括 CoS の根拠付き対応と、人への escalation・人の回答の両方。消費は cos:general で案件へ割り振らない |
| 4 operator の配備・rollback | `actor_trace_flow4.rs`（1 件） | pass。promote・rollback 各 1 回成功と start 失敗 1 回の deployment_audit を、人の判断・task・要求と照合 |
| 5 再配送・担当の再起動 | `actor_trace_flow5.rs`（1 件） | pass。再取り込み・取り次ぎ再送・回答再送・観測記録再送・store 再オープン後の再実行で、操作・介入・usage・要求・report の件数と全体が不変 |

本物の LLM・外部ネットワーク・sleep・負荷試験は使っていない（固定 UTC・loopback の偽 HTTP・shell fixture）。

## 各 unit が直した製品側の欠陥

- harness（流れ 1）: event 追記・decision transaction・CoS 操作保存が内部で壁時計を読み、固定時計で試験できなかった。時刻を受け取る `_at` 入口を足し、従来の入口は壁時計を渡して同じ実装へ委譲する（ADR 付記「固定時計の入口」）。
- flow2: 人へ回した後に CoS が取り次ぎ（`instructed_by`）を根拠に答えた operation が、request ledger の `operations` と `instructed_by:`・`relay:` から落ちていた。`request_ledger` で対象の decision か受信箱の件が一致する operation を同じ要求に寄せる（ADR 付記、試験 `later_operation_on_an_escalated_request_is_linked_by_its_target`）。あわせて取り次ぎ検査（D3）に時刻を渡す `inbox_answer_instruction_at` を足した。
- flow3・flow4・flow5: 製品側の欠陥は見つからず、製品コードは変えていない。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `bash scripts/dev/test-affected.sh`（main 基準。WU 基準では docs だけの差分になるので `env -u CELERIS_WU_BASE` で実行）→ exit 0。Rust 6,640 passed・21 skipped、web 689 passed と 105 passed、doc test 通過。文書検査（check-architecture-map・check-doc-links・check-adr-numbers・progress-index --check）も ok。
- `cargo test -p celeris --test actor_trace_flow1` 〜 `--test actor_trace_flow5` → exit 0。5 target・6 試験が合格（1・1・2・1・1 件）。
- `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh --list` → 対象に celeris を含む 13 crate と web。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決

- HTTP の認証（operator だけが relays を呼べる等）と実 LLM harness の起動は、この試験の境界外。既存の task-api・task-worker の試験が受け持つ。
- 流れ 5 の human-ops report には人の操作の観測記録を入れていない（CoS 操作の記録だけ）。人の介入の report 側の重複なしは流れ 2 の素材で export・requests では確かめたが、report の人の観測記録の再送は流れ 5 では未確認。

- closeout の最初の check（WU 基準の test-affected.sh の log を `actor_trace_flowN` で grep する）は通らなかった。`CELERIS_WU_BASE` が流れの統合後の snapshot なので差分は文書だけになり Rust 試験が走らないこと、`.config/nextest.toml` の `status-level = "fail"` で合格した試験名が log に出ないことの 2 つが理由。plan_issue を返し、check は「`cargo test -p celeris --test actor_trace_flow1`〜`5`」と「main 基準の `test-affected.sh --list` に celeris が入り本体が exit 0」の 2 本に差し替わった。

## 提案

- 台帳 ADR（`2026-10-27-eval-request-ledger.md`）と取り次ぎ ADR（`2026-10-27-relayed-instructions.md`）から、この ADR の流れ 2 の付記へ参照を足す（WU の範囲外なので人が判断する）。
- 流れ 5 に人の操作の観測記録の再送を足し、human-ops report 側の人の介入の重複なしも固定する。

## 本番への移行

設定・DB migration・本番操作は不要。
