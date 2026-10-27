---
title: 修正 task と待っている task を unblocks で結び、修正を含む release の昇格で再開する
tasks: [01M5XXF7QMD919KZ07G80MESYF]
status: done
updated: 2026-10-27
---
# 修正 task と待っている task を unblocks で結び、修正を含む release の昇格で再開する

完了日: 2026-10-27

判断は [ADR 2026-10-27-unblocks-reopen-on-promotion](../adr/2026-10-27-unblocks-reopen-on-promotion.md)。
発端は 2026-10-26 の 01M5WC8PPW（修正 01M5WCJ9ZM・01M5WMNDA8・01M5WTKE9M の昇格ごとに operator が手で reopen）。

## 変更

- `crates/task-core/migrations/0096_task_unblocks.sql`（SCHEMA_VERSION 96）: `task_unblocks`（修正 task・待っている task・発火の記録）。
  SCHEMA_VERSION の assert・`delivery.rs` の版数列・旧 DB へ戻す試験の DROP 列挙を更新。
- `crates/task-core/src/unblocks.rs`・`store/unblocks.rs`: 型と `TaskStore::{unblock_pending, unblocks_of, unblock_fire}`、
  `SqliteStore::unblocks_insert_tx`。`unblock_fire` は `fired_at IS NULL` の条件付き更新・system コメント・`Trigger::Reopen` を
  1 つの transaction で書く。
- `crates/task-ops/src/unblocks.rs`: `validate_unblocks`・`read_promoted_releases`（`promoted.json` の `included.tasks`）・`sweep`
  （修正 task かその祖先を含む最も古い昇格済み release で発火。`failed` → reopen、`blocked` → コメント、他 → skipped）。
- `crates/task-api/src/handlers/tasks.rs`: `NewTaskBody.unblocks`（API と CoS の `task.create` の共通経路）。検証は 422 `validation`
  （`field: "unblocks"`）、結びは task と同じ transaction で書く。
- `crates/celeris/src/daemon/unblocks_sweep.rs`: `active` の daemon が 300 秒ごとに `[selfdeploy] releases_dir` を見て `sweep` する
  （`stranded_sweep` と同じ周期・単発実行）。
- 文書: `docs/api/v1/api.md`（`POST /tasks` の `unblocks`）、`config/skills/cos-operator/operations.md`（起票の節）。
  `docs/api/v1/api-v1.schema.json` と web の生成型を再生成。

## 試験

検査は提出時の HEAD で実行（基準は `git merge-base HEAD main`）。

- `cargo test -p task-ops --lib unblocks`: 4 passed（failed の待ちが 1 度だけ reopen され、二度目の昇格・再走査で重ならない／
  blocked はコメントだけ・子の修正は親の配送で判定／`included: null`・壊れた `promoted.json` を飛ばす／検証）。
- `cargo test -p task-api --test unblocks`: 3 passed（API で結んで昇格 → 1 度だけ reopen、二度目の昇格で重ならない／
  不明・重複 id は 422 で何も作らない／CoS の `task.create` でも結びが書かれる）。
- `bash scripts/dev/test-affected.sh`: exit 0（nextest 6499 passed・21 skipped、web Vitest 682 passed）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決事項

- `promoted.json` の `included` が `null`（preview が取れなかった昇格）や `complete: false` で一覧から漏れた修正は検出しない
  （ADR D2。そのときは人が reopen する）。
- 結びを後から足す・消す API と、task 詳細での結びの表示は無い（作成時だけ）。

## 提案

- task 詳細（`GET /tasks/{id}`）に「この task を待っている／この task が待っている修正」と発火の記録を出すと、
  人が結びを確かめられる。

## 本番への移行

- config の変更は不要。migration 0096 は追加だけ（昇格時に自動で当たる）。
