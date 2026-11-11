---
title: CoS 不在の直接通知（CosFallback）を一次対応の後だけ送る（全種類）
tasks: [01M756VNCZAB8DSPKB1RCPJXTB]
status: done
updated: 2026-11-11
---
# CoS 不在の直接通知（CosFallback）を一次対応の後だけ送る（全種類）

完了日: 2026-11-11

ADR: [2026-11-11-cos-fallback-timing](../adr/2026-11-11-cos-fallback-timing.md)

## 変更
- `phase 1: CoS 不在の直接通知を一次対応の後だけにし、送信前の判定で取り下げる` — 次をまとめた。
  - D1: `fallback.rs` の `FirstResponseWait` で、全種類の件の安全網 `fallback_after_secs` を run の開始（件の run、
    または後ろで待つ同じ thread の run）か、件を拾えるようになった時刻から数える。到着から数える分岐を消した。
  - D2: `task-core` の `cos_triage_resolve_tx`・`cos_triage_outbox_claim` が、未送信の fallback を取り下げて CoS の判定を採る。
    API の resolve は未送信の fallback の件を受け、送信済みなら 409 で判定を記録する。
  - D3: `triage_launch` が待ちのある thread を毎 tick SQLite から読み、拾わない理由（`TriageSkip`）を log に残す。`dirty` を消した。
  - D4: root 完了のまとめも同じ規則。
  - D5: config の `unavailable_after_secs`・`authorization_unavailable_after_secs` を消して `fallback_after_secs` にした。
  - D6: web のブラウザ通知（未読の増加で出していた）を消した。
  - ADR の付記（cos-chat-home D6・cos-inbox-triage・cos-inbox-triage-fallback・cos-authorization・root-done-cos-summary）、
    skill cos-inbox-triage §7・SOURCE.md、`config/celeris.example.toml` を合わせた。

- `phase 1: 待ちの判定を task-ops に、取り下げの SQL を task-core の cos_fallback に移し、repo health の行数の上限に合わせる` —
  repo health（`size.responsibility-growth`）が dispatcher module（35103 → 35194）・task-core chat module（8026 > 8000）・
  task-api `cos::resolve`（268 → 278 行）で落ちたので、`FirstResponseWait`・`TriageSkip` を `task_ops::cos_first_response` に、
  D2 の SQL を `task_core::cos_fallback` に、resolve の判定を `terminal_conflict` に移した。挙動は変えていない。

- `phase 1: fallback の取り下げを書き込みから始め、script_generation_e2e の digest を全 page で数える` —
  `withdraw_unsent` が読みから始めていたので、deferred transaction の resolve が別 process の書き込みと競って
  `SQLITE_BUSY_SNAPSHOT` で落ちた（`script_generation_e2e` の 1259 行の `expect("resolve")`）。UPDATE を先にした。
  daemon が待ちのある thread を毎 tick 拾う（D3）ので general thread の message が 1 page（200 件）を超え、試験の
  `digests()` が古い digest を見落とした（1033 行）。`after_seq` で全 page を読むようにした。単独で 3 回続けて合格。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 対象の試験: `cargo nextest run -p task-dispatch -p task-core -p celeris -p task-api -E 'test(/cos_chat_triage|cos_fallback_timing|cos_chat_root_done|root_done_summary|cos_triage|cos_chat_authorization|cos_reload|cos_inbox/)'` → 結果は下の「検査」に書く。
- 受け入れ条件 3 の試験:
  - (a) `cos_fallback_timing_no_fallback_while_its_own_run_is_live`・`cos_fallback_timing_safety_net_for_a_hung_run_of_any_kind`
  - (b) `cos_fallback_timing_item_behind_a_live_run_is_picked_when_it_ends`・`cos_fallback_timing_behind_a_hung_run_uses_the_run_start`
  - (c) `cos_fallback_timing_standby_to_active_picks_on_the_next_tick`・`cos_fallback_timing_idle_thread_after_a_foreign_run_is_picked`・
    `cos_fallback_timing_pickable_clock_starts_at_the_end_of_the_last_run`
  - (d) `task-core` の `cos_triage_judgment_before_the_send_withdraws_the_fallback`
  - (e) `root_done_summary_waits_for_the_safety_net_when_cos_cannot_start`・`root_done_summary_waits_for_its_live_run_up_to_the_safety_net`
- `bash scripts/dev/test-affected.sh` → exit 0（attempt 2、load average 48、約 27 分）。web の vitest（Test Files 103 passed・6 passed）、repo-health（all checks passed）も合格。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0。
- `cargo nextest run -p celeris --test script_generation_e2e` → 3 回続けて exit 0。
- `bash scripts/dev/check-health.sh` → exit 0（verdict=pass）。

## 未解決
- `git_workspace::review_records_each_repositories_own_definition_and_worktree_results` は負荷で落ちる（tick 数の上限待ち）。
  G4（01M6TYVJSXBZMSBC0YGAT2TTX0）の branch に出来事待ちへの直しがあり、main に入れば解消する見込み（未確認）。
- 本番の「thread も枠も空いているのに 120 秒以上拾われない」8 件の原因は決めきれていない。推測（未確認）: 他の process の run が
  SQLite に残っている間に claim が拒まれ、`triage.dirty` が消えて新しい件が来るまで拾われなかった。この型は
  `cos_fallback_timing_idle_thread_after_a_foreign_run_is_picked` で再現し、`dirty` を消して毎 tick SQLite から読むようにした。
  拾わない理由は log（`CoS triage launch skipped a thread with pending items`、`reason=`）に残る。
- 送信中（POST の最中）の fallback は取り下げられない。CoS の escalate がその間に来ると 2 通になりうる（窓は POST 1 回分）。
- fallback が書いた hand-off の行と統括 CoS への要約依頼は、取り下げの後も残る。

## 本番への移行
人が実行する手順（worker は本番に触らない）:
1. 本番 config（`~/.config/celeris/celeris.toml`）の `[cos.triage]` から `unavailable_after_secs = 120` の行を消す。
   `authorization_unavailable_after_secs` を書いていれば `fallback_after_secs` に名前を変える（既定でよければ消す）。
   旧い欄が残っていると新しい release は読み込みエラーで起動しない。
2. 確認: `grep -n "unavailable_after_secs" ~/.config/celeris/celeris.toml` が何も出さないこと。release の切り替え後に
   `/api/v1/health` が ok で、`journalctl --user -u 'celeris@*'` に config の読み込みエラーが無いこと。
3. KB の skill cos-inbox-triage の写しを使っているなら、`config/skills/cos-inbox-triage/SKILL.md` の §7 に差し替える
   （policy_version は 11 のまま）。

## 提案
- 未読の知らせを web で知りたい人には、画面の未読の badge で足りるかを人が確かめる。
