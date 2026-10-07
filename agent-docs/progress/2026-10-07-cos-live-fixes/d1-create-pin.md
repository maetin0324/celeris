---
title: 葉 d1-create-pin — POST /tasks と task.create の attachment_ids（作成と pin を同じ transaction で）
tasks: [01M4APB5FP8T3TAAE51Z20E3M1]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 d1-create-pin: 作成時の attachment_ids（ADR 2026-10-07-cos-live-fixes D1）

## やったこと

- `crates/task-api/src/handlers/tasks.rs`: `NewTaskBody` に `attachment_ids: Vec<String>`（`#[serde(default)]`）。
  `create_task_op` は人の経路も CoS の経路も「組み立て → 1 transaction で task 行・`Created` event・書き込み範囲の予告・
  各添付の pin」に揃えた。pin の前に同じ transaction の中で検証し、1 件でも不正なら 422 `invalid_attachment` で何も書かない。
  CoS 経路（`/cos/operations` の `task.create`。`dispatch` は同じ `NewTaskBody` を decode するので欄はそのまま通る）は
  `cos_operations.result` を `{"task_id","attachment_ids"}` にし、拒否は `rejected` 行と監査 event に残る（既存の
  `cos_operation_apply` の失敗経路）。CoS 経由では credential の thread に属さない添付も 422。
- `crates/task-core/src/chat/attachments.rs`: `task_pin_problem_tx`（ULID 形・重複・上限 `MAX_TASK_CREATE_ATTACHMENTS = 20`・
  `ready`・未期限切れ・thread 一致）を追加。pin 自体は既存の `add_ref_tx`。
- `crates/task-core/src/store/tasks.rs`: `SqliteStore::create_task_with(task, extra_events, also)`（`create_task_impl` と同じ
  IMMEDIATE transaction の中で `also` を走らせる）。
- 試験 `crates/task-api/tests/cos_live_fix_d1.rs`（3 本）:
  `cos_live_fix_d1_created_task_already_has_its_pins`（201 直後に `list_for_owner("task", id)` — dispatcher が最初の run の
  入力 manifest を作るのと同じ読み — に 2 件、期限が外れる、欄なしは pin なし）、
  `cos_live_fix_d1_invalid_attachment_creates_no_task`（存在しない・期限切れ・重複・非 ULID で 422、tasks も refs も 0 行）、
  `cos_live_fix_d1_cos_task_create_pins_and_audits`（他 thread の添付で 422・rejected 行・task 0、正しい添付で applied・
  result に attachment_ids・pin あり）。
- schema 再生成（`UPDATE_SCHEMA=1 cargo test -p task-api`）→ `docs/api/v1/api-v1.schema.json`、
  `gui/app/celeris/types.ts`（pnpm@11.27.0 gen:types）、`web/api/generated/{schema.json,types.ts}`（pnpm@12.6.0 install --offline 後 gen-types.mjs）。
- `docs/api/v1/gui-api.md` §3.4 に `attachment_ids`（と未記載だった `expected_write_paths`）を追記。
- `config/skills/cos-operator/SKILL.md`: §3 の起票行と POST /tasks の最小例（`acceptance` 必須・要素の形・human だけは 422）、
  §3a を「task へは起票の request に `attachment_ids` を入れる 1 回の operation」「KB へは候補を作ってから references で pin」に直した。

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo test -p task-api --test cos_live_fix_d1` | exit 0、3 passed |
| `cargo test -p task-api --lib committed_schema_matches_generated` | exit 0、1 passed |
| `cargo test --workspace --no-run` | exit 0（`NewTaskBody` を直組みする他 crate は無い） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh` | exit 0、4646 tests run: 4646 passed, 13 skipped |

## 未解決事項

- 人の API 経路の pin は専用の event を積んでいない（ADR D1 の「人の API 呼び出しでも pin の event は積む」）。
  `Event` に variant を足すと `task-api/src/query.rs`・web の event-kinds/invalidation-map まで波及するので、この葉では
  `chat_attachment_refs.created_at`（同じ transaction）を記録とした。CoS 経路は `cos_operations.result` と監査 event に残る。
- `crates/task-worker/src/cos_chat.rs` の `attachment_pin_rules`（CoS chat run の前置き「### 添付の引き渡し (pin)」）は
  まだ「owner を先に作り → references で pin」と書いており、§3a の新しい形と食い違う。この葉の範囲（task-worker は範囲外）では直せない。

## 提案

- 後続の葉（live-script か close）の範囲に `crates/task-worker/src/cos_chat.rs`・`crates/task-worker/src/cos_chat/tests.rs` を足し、
  前置きの手順 1 を「画像は起票の request に `attachment_ids` を入れる（1 回の operation）」に、references の例を KB 候補向けに直す
  （試験 `cos_chat_attach_handoff_cos_preamble_explains_pin` の文言 assert も合わせる）。前置きと skill が食い違うと live の (d) が再び
  「起票してから pin」で走りうる。
- pin の event が要るなら、`Event::AttachmentPinned { attachment_id, owner_kind }` を足す別葉（schema・web event-kinds 込み）にする。
