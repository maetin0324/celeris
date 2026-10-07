---
title: 葉 d2-kb-inbox — POST /knowledge/inbox と CoS operation knowledge.record、CLI は CoS credential で API
tasks: [01M4APB5FP8T3TAAE51Z20E3M1]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 d2-kb-inbox: 監査つきの KB 候補作成（ADR 2026-10-07-cos-live-fixes D2）

## やったこと

- `crates/task-api/src/knowledge.rs`: `POST /api/v1/knowledge/inbox`（管理系、201 `KnowledgeRecordResult`）。本文
  `KnowledgeRecordBody`（`title`・`scope`・`body`・`sources[]`・任意の `tags[]`・`confidence`・`path`・`attachment_ids[]`、
  `deny_unknown_fields`）。本体 `record_op` を handler と CoS operation が共有する。案件の一覧つきの layout で
  `record_prepare` を呼び（不正 scope・未知の案件は 422 `validation` field `scope`）、添付の pin（`task_pin_problem_tx` ＋
  `add_ref_tx`、owner_kind `knowledge_inbox`。CoS は credential の thread の添付だけ）と CoS の監査を SQLite に commit してから
  候補ファイルを `_inbox/<id>.md` に rename して git commit する。pin・監査が失敗すれば一時ファイルは drop で消える。
- `crates/task-api/src/cos/operations.rs`: `ALLOWED` に `("POST", "/api/v1/knowledge/inbox", "knowledge.record")`、
  `dispatch` に `knowledge.record`（`target_kind` `knowledge`・`target_id` 候補 id、`result` は `sha` を除いた応答と同じ形）。
- `crates/task-ops/src/knowledge.rs`: `record_in` を `record_prepare`（検証・置き場のガード・id 決定・`_inbox/.<id>.md.tmp`
  への書き込み）と `PreparedRecord::commit`（rename ＋ git commit）に分けた。`record` / `record_in` の挙動は同じ。
  id の重複回避は一時名も見る。
- `crates/celerisctl`: CoS run credential（`CELERIS_COS_RUN_CREDENTIAL`）があるときの `knowledge record` は KB を直接書かず
  `POST /api/v1/knowledge/inbox` を `/cos/operations` で送る（`main.rs` の分岐、`knowledge::run_record_cos` /
  `record_via_cos`）。理由は `--reason` → `CELERIS_COS_REASON` → 既定文、冪等キーは題名と本文の FNV-1a。`--attachment-id`
  を追加（直書きの経路では拒否）。`--json` の出力は候補 `id` を最上位に置き `operation` を添える（前置きの「出力の id」が通る）。
  これまでの「CoS credential の record は対応なしで拒否」はこの API 経路に置き換わった。
- scope の書式: skill（`config/skills/cos-operator/SKILL.md` §3 の表と §3a の KB 手順）を `project:<slug>` に揃え、
  KB への引き渡しを「候補の作成に `attachment_ids` を入れる 1 回の operation」に直した。CLI の help も同じ形。
  `projects/<slug>` は既存の `kb::place` が `project:<slug>` に正規化する（API・CLI 共通）。
- schema: `knowledge_record` / `knowledge_record_result` を足し、`UPDATE_SCHEMA=1` で `docs/api/v1/api-v1.schema.json`、
  gui（pnpm@11.27.0 `gen:types`）・web（`node web/scripts/gen-types.mjs`）の生成型を更新。新しい Event は足していない
  （監査は既存の `cos_operations` と監査 event）。
- `docs/api/v1/gui-api.md`: 表の 93b、§3.104a（新 route）、§3.129 に登録済み KB 操作の段落。

## 証拠

| 条件 | コマンド | 結果 |
|---|---|---|
| cos_live_fix_d2 試験 | `cargo test -p task-api --test cos_live_fix_d2` | exit 0、3 passed（候補＋provenance 一覧/詳細、不正 scope・不正添付 422 で候補なし、CoS operation の監査行・再送・他 thread の添付・不正 scope の rejected 行） |
| 同（CLI） | `cargo test -p celerisctl --bin celerisctl cos_live_fix_d2` | exit 0、3 passed（偽 server に `/cos/operations` で `knowledge/inbox`、冪等キー、直書きは `--attachment-id` 拒否） |
| schema・生成型 | `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema` → gui `pnpm gen:types`・web `node web/scripts/gen-types.mjs` | 差分は新 2 型のみ。全体 nextest の `committed_schema_matches_generated` も pass |
| 全体試験 | `bash scripts/dev/test-parallel.sh` | exit 0、passed 4657 / failed 0 / ignored 14 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| fmt | `cargo fmt --all -- --check` | exit 0 |

## 未解決事項

- CoS 経路で SQLite（監査・pin）の commit の後に rename / git commit が失敗すると、`applied` の operation と候補ファイルの
  無い状態が残る（API はエラーを返す）。ADR D2 の順序（DB の commit → rename）どおりで、逆順の失敗（pin の無い候補）より
  見つけやすい側に倒した。
- `crates/task-worker/src/cos_chat.rs` の前置き（添付があるときの手順）は「候補を作ってから pin」のままで、範囲外のため
  直していない。CLI の `--json` が候補 `id` を最上位に出すので手順はそのまま動く。

## 提案

- task-worker の CoS 前置きの KB 手順を、skill §3a と同じ「`attachment_ids` つきの `knowledge.record` 1 回」に直す
  （close か後続の葉で）。
