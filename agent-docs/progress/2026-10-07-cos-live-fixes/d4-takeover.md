---
title: 葉 d4-takeover — 終端済みの CoS run を orphan takeover の対象から外す
tasks: [01M4APB5FP8T3TAAE51Z20E3M1]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 d4-takeover: 終端済みの CoS run を orphan takeover から外す（ADR 2026-10-07-cos-live-fixes D4）

## 原因（live2 の証跡から特定）

live2 の試験用 DB（`01M46VVAD0ZAVZ9C4Q0KJM9ESV/wu/live-check2/artifacts/live2/full/celeris.sqlite3`）で、triage thread の
2 run はどちらも **worker の終了（`result.json` 07:10:35.18）の約 0.2 秒後**に `orphan takeover; continuing in a new run`
になり、続きの run が同じ時刻に始まった。daemon は 1 つだけ（`holders_gone` は常に真）。

- triage run は**出力 message を持たない**（`output_message_id` が null）。worker が本文（`kind: text`）を流すと
  `chat_run_append_text` が `Conflict("run … has no output message")` を返す。
- `ChatRunSink::write` は Conflict を「run は既に終端」と読み、`finished = true` にしていた。そのため `finish` が
  `chat_run_finish` を呼ばずに `Ok(None)` を返し、**run は store 上で running のまま handle だけが終わった**。
- 次の tick で `running` から handle が外れ、`control_hook` が持ち主の居ない run として takeover した
  （続きの message `cos-recover:<run>` → 新しい run。前の run の操作は既に適用済みなので、続きの run は「処理済み」とだけ返した）。

## 直したこと

- `crates/task-dispatch/src/dispatcher/cos_chat/sink.rs`: Conflict の後に store で run の状態を読み、**終端のときだけ**
  以後の書き込みを止める。非終端（出力 message の無い run への本文）は、その書き込みだけを捨て、以後の本文は送らない
  （`text_rejected`）。`finish` は必ず終端を書く。
- `crates/task-core/src/chat/run_store.rs`: `chat_run_takeover(run_id, continuation, reason, now)` を足した。run が非終端で
  あることの確認・続きの message の投入・run の interrupted 化を **1 つの write transaction** で行い、終端なら何も書かず
  `Ok(None)`。`chat_run_finish` の本体は `finish_conn`、`chat_message_post` の本体は `store.rs` の `message_post_conn` に
  切り出した（公開の挙動は同じ）。schema は変えていない。
- `crates/task-dispatch/src/dispatcher/cos_chat/control.rs`: `recover_orphan` は最初に run を読み直し、終端なら何もしない
  （review card・receipts の message も出さない）。続きの run の投入は `chat_run_takeover` に置き換えた
  （以前は `chat_message_post` → `chat_run_finish` の 2 transaction で、間に終端になると続きだけが入った）。
- 順序: `run_claimed` は終端（`sink.finish` → `chat_run_finish`。credential の revoke も同じ transaction）を commit してから
  task を終える。handle・account / provider の枠は、次の tick が handle の終了を見てから外す。この順序は元から正しく、
  試験で固定した（下の `..._terminal_is_recorded_before_slot_release`）。

## 試験（`crates/task-dispatch/src/dispatcher/tests/cos_live_fix_d4.rs`。偽ハーネス・注入時計、sleep なし）

- `cos_live_fix_d4_finished_triage_run_is_not_taken_over` — 本文を流してから終わる triage run が completed を記録し、
  lease の期限を過ぎて tick しても interrupted にならず続きの run も起きない（sink の修正を外すと失敗することを確認した）
- `cos_live_fix_d4_terminal_run_with_expired_lease_is_left_alone` — 終端記録済みで lease の切れた chat run は tick で触られない
- `cos_live_fix_d4_takeover_of_terminal_run_writes_nothing` — list の後に終端になった run への `chat_run_takeover` は何も書かない
- `cos_live_fix_d4_true_orphan_is_still_taken_over_once` — 本当の孤児は従来どおり interrupted ＋ 続きの run 1 本
- `cos_live_fix_d4_terminal_is_recorded_before_slot_release` — handle の終了時点で終端は記録済み、枠の解放はその後の tick

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo test -p task-dispatch --lib cos_live_fix_d4` | exit 0、5 passed |
| 同上（sink の修正を一時的に外して） | `..._finished_triage_run_is_not_taken_over` が FAILED（回帰を再現） |
| `cargo test -p task-dispatch cos_chat` | exit 0、lib 67 passed・tests/cos_chat_triage 11 passed |
| `cargo test -p task-core chat` | exit 0、61 passed |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh` | exit 0、4648 passed、13 skipped |

## 未解決事項

- `run_claimed` の早期 return（完了時の state 読み取り・pending 確認の DB エラー）は終端を書かずに終わり、次の tick で
  takeover される。DB が読めない時の回収経路として残した（ADR D4 の「持ち主の sink が記録するのを待つ」は、handle が
  生きている間はそのとおり。handle が終わった後に記録の失敗を待ち続けると run が永久に残るため）。
- triage run の最終本文（`final_text`）は出力 message が無いので残らない（従来どおり。triage の結果は card と操作で残る）。

## 提案

- triage run にも出力 message を持たせるか、sink が run の `output_message_id` を最初に読んで本文を送らないかを決めると、
  Conflict の往復が無くなる。
