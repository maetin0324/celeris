---
title: 子 task が Failed のとき、再実行を待つ指示がある間は親を起こさない
updated: 2026-10-27
status: done
tasks: [01M5XXF7QF5831M38E81S63RP1]
---
# 子 task が Failed のとき、再実行を待つ指示がある間は親を起こさない

完了日: 2026-10-27

## 変更
- ADR `agent-docs/adr/2026-10-18-parent-waits-for-children.md` に付記（2026-10-27）A1〜A4 を足した。
- `task_ops::delegate::child_wait`（新規）: `awaited_children` と `is_wait_instruction`。
- `SqliteStore::cos_triage_failure_escalated`: その子の `failed` 項目が `escalated` か `fallback` で未解決か。
- dispatcher: `parent_waits_for_children` を `child_tasks.rs` に移し、`awaited_children` を使う。待ちは `DelegatedChildren` として見せる。
  run の終わりの子待ち判定（`worker_finish.rs`）も `awaited_children` を使う。
- 試験: dispatcher `parent_stays_asleep_for_a_failed_child_while_told_to_wait_for_its_rerun`、task-ops 3 件、task-core 1 件。
  旧い挙動（印があっても failed の子で親を起こす）を固定した試験は無かった。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `bash scripts/dev/test-affected.sh`: 結果は下の「検査」に書く。
- `cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`: 同上。

## 未解決事項
- 「待て」の判定は語の照合で行う。語の一覧に無い言い回しの指示は拾わない。その場合、親は従来どおり起きる。
- escalated 項目の判定は、CoS chat が有効な daemon（`cos_chat_launch` あり）でだけ働く。

## 提案
- 回答・comment に構造化した「子の再実行を待つ」の意図（例: `AnswerIntent::WaitChildren`）を足せば、語の照合をやめられる。
