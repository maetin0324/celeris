---
title: 一時停止した子 task を同時の子 task の枠に数えず、枠の待ちを waits に出す
tasks: [01M87M5AECGN6ABZTS10S5WE0X]
status: done
updated: 2026-11-24
---
# 一時停止した子 task を同時の子 task の枠に数えず、枠の待ちを waits に出す

完了日: 2026-11-24

状態: done（ADR・実装・試験・schema と web の生成型の再生成まで済んだ）。基準は `git merge-base HEAD main`。

## commit（件名）

- adr: 自分で一時停止した子 task を同時の子 task の枠に数えず、枠の待ちを waits に出す
- tree: 自分で一時停止した子 task を同時の子 task の枠に数えず、枠の待ち child_slot を waits に出す
- progress: 一時停止した子 task の枠の扱いの証拠（生存確認の試験の並びの修正を含む）

## 決めたこと（ADR `agent-docs/adr/2026-11-24-paused-children-free-child-slots.md`、ADR-0079 に付記）

- D1: 枠を占めるのは `running` の kind task の unit のうち、子 task 自身に `paused_at` が無いものだけ。祖先の一時停止で止まっている子は数える
  （親も同じ一時停止の下にあり、新しい子も走らない。祖先の再開で一斉に走るので、数えないと上限を超える）。規則は
  `task_core::wait::child_slot_holders` と `child_creation_candidates` の 1 か所で、dispatcher の `reconcile_tree_units`・`plan_waits`・
  `task_ops::tree::node_liveness_facts` が同じ関数を呼ぶ。
- D2: 一時停止を解いた子が上限を超えるときは、再開を待たせず一時的に超えてよい。超えている間は新しい子を作らない。
- D3: `TaskWait::ChildSlot`（kind `child_slot`、`unit_keys`・`task_ids`・`limit`）。上限は `MemoryWaits::with_child_slot_limit` で渡す
  （API は `ApiState.tree_limits`、dispatcher は自分の config。`celerisctl` と受信箱の draft の判定は渡さず、推測しない）。
- D4: 待ち合いを受信箱に知らせる経路は足さない（D1 で今回の待ち合いが起きなくなり、残る祖先の一時停止は人の一時停止〈`Paused`〉の名指しの
  待ちのため）。生存確認の「非終端の子が 1 件でもあれば枠待ち」の近似を消し、D3 と同じ材料（`NodeLivenessFacts.child_slot_waiting`）で
  `Waiting(child_slot)` と `Runnable(child_creation)` を分ける。

## 証拠（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh`: exit 0。nextest 8913 件が通った（34 skipped）。web の tsc -b、vitest（108 files / 774 件）、
  node --test（110 件、fail 0）、test-ts-verify（20 件）が通った。最初の実行（試験の修正前の commit）は 8912/8913 で、新しい生存確認の試験 1 件が
  名指しの待ちの並び（最初の 1 件が理由になる）を見落として落ちた。試験の並びを直して再実行した。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- 新しい試験:
  - `crates/task-dispatch/src/dispatcher/tests/tree_child_slots.rs`（3 件）: 自分で一時停止した子 2 件で上限 2 のまま次の照合で子が作られる、
    再開で 3/2 になっても再開した子は `ready_tasks` に入り新しい子は作らず `child_slot` で待ち、2 を下回ったら作る、祖先（親）の一時停止の子は
    数える。`task_ops::view::task_detail`（`GET /tasks/{id}/execution` の `waits` と同じ材料）に `child_slot` が出る。
  - `crates/task-core/src/wait/child_slot/tests.rs`（2 件）: 規則・出る条件・上限 `None` で出さない。
  - `crates/task-core/src/tree/r3b_tests.rs` の `ready_task_unit_waits_on_a_child_slot_only_when_the_slots_are_full`: 生存確認の分類。
  - `web/features/tasks/wait-reason.test.ts`: `child_slot` の表示と移動先。
- schema: `UPDATE_SCHEMA=1 cargo nextest run -p task-api -E 'test(committed_schema_matches_generated)'` で `docs/api/v1/api-v1.schema.json` を
  再生成し、`pnpm gen:types` で `web/api/generated/` を再生成した。`docs/api/v1/api.md` に改訂を 1 行足した。

## 本番への移行

- config の書き換えは要らない（設定欄は足していない・消していない）。
- 運用者が 2026-11-24 20:31Z に一時的に 3 にした本番の `execution.tree.max_parallel_child_tasks` は、この変更の昇格の後なら既定（2）に戻してよい
  （MemT の E2・E3 は自分で一時停止しているので枠に数えない）。戻すのは運用者（人の手順。`PATCH /api/v1/config/fields`）。

## 未解決事項

- 親（または祖先）が一時停止されている間も `reconcile_tree_units` は子を作る（作った子は祖先の一時停止で走らない）。従来どおりで、この task では
  変えていない（ADR D5）。
- 計画の `adopt` の unit は計画の spec にしか印が無いので、`child_slot` の `unit_keys` には入る（dispatcher は作成の直前に見送り、枠は使わない）。

## 提案

- 無し。
