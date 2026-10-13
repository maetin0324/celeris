---
tasks: [01M4Q6GQV76NM3EGZ3P6EN378J]
status: running
title: LLM source・harness API の進捗（api-v2）
updated: 2026-10-12
---

# LLM source・harness API の進捗（api-v2）

## 状態

- 全体検査（統合後 HEAD `fa03aa11`）: 通過。test-parallel・clippy・文書検査 3 本は exit 0。
- 完了ではない。brief の API 形のうち、accounts の `(source_id, account_id)` 化と `llm/sources` の catalog・health は HEAD に無い（下の「未解決」）。task 全体の受け入れ条件 1（新 API の route）は一部だけ満たす。

## 各 unit の記録

| unit | commit | 内容 | 状態 |
|---|---|---|---|
| src-acct-api | `32c60bac` | `GET /api/v1/llm/sources/{source_id}` を追加。accounts の内部 limit map は `(source_id, account_id)` の tuple | 一部。HTTP 路の再キーと catalog・health は入っていない（commit title の主張と食い違う） |
| harness-api | `166873d7` | `GET /api/v1/harnesses`・`GET /api/v1/harnesses/{harness_id}`（HarnessesReader 経由。celeris daemon から宣言を渡す） | 完了 |
| harness-run-api | — | 計画で superseded（harness-api と run-selection に分割） | 対象外 |
| run-selection | `779de575` | runs 表の選択欄を task detail の runs の `selection` に出す。旧 run は `legacy_unresolved` | 完了（試験 `task_runs_expose_persisted_selection_and_legacy_unresolved`） |
| assign-drop | `761f9d49` | migration 0069 で `model_role_assignments`・`model_role_scopes` を削除。型・保存 API・daemon の適用・`assigned_tiers` を削除。routing shadow は provider ID を含まない | 完了（記録 `assign-drop.md`） |
| gen-types | `2c69314e` | web・gui の生成型を新 schema から再生成。provider 画面一式を削除 | 完了（記録 `gen-types.md`） |

前段の unit（src-acct-api・harness-api・run-selection）は個別の記録ファイルを残していない。上の表が記録の代わり。

## 証拠コマンドと結果（HEAD `fa03aa11`）

- `TMPDIR=/local/x bash scripts/dev/test-parallel.sh` → exit 0。`CELERIS_TEST_SUMMARY`: nextest 160 binaries と doctest 10 binaries、passed 4903、failed 0、ignored 14、nextest_exit 0、doctest_exit 0。警告: `tests left 2 entries in TMPDIR`（試験の後始末の残り。終了時に削除済み。失敗ではない）。
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`）。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`ok (182 files)`）。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0（`ok`）。
- `sh scripts/dev/check-doc-links.sh` → exit 0（`ok`）。
- 旧 route の除去: `crates/` と `docs/api/v1` に `/api/v1/providers` の route 登録は無い（`GET /providers` の文字列はコメントと gui-api.md の「削除」節だけ）。
- 新 route の存在: `crates/task-api/src/llm_sources.rs:63-64`（list・detail）、`crates/task-api/src/harnesses.rs:48-49`（list・detail）。

注: TMPDIR を `/local/x` に固定した。既定の TMPDIR では unix socket の SUN_LEN で試験が落ちる（既知）。

## 未解決

1. **（実装済み 2026-10-12: accounts-rekey。進捗 `agent-docs/progress/2026-10-12-accounts-rekey.md`）** accounts の HTTP 路が `(source_id, account_id)` になっていない**（brief の要求）。`crates/task-api/src/handlers.rs:106-127` は `{id}` のまま（意味は adapter）。`POST /accounts`・`DELETE /accounts/{id}`・`PATCH /accounts/{id}/{account}`・`check`・`login` も同じ。旧 adapter key の路は消されていない。内部の `handlers/accounts.rs:72` だけが tuple。
2. **`llm/sources` に catalog と health が無い**。`32c60bac` の diff は detail 路の追加だけで、`LlmSourceView`（`crates/task-api/src/types.rs:2082`）に catalog・health の欄は増えていない。
3. **試験が不足**。`crates/task-api/tests/llm_routes_admin.rs` は harness 一覧と run selection の 2 試験だけ。accounts の CRUD、sources の catalog・detail の試験は無い。旧 `accounts_admin.rs`・`llm_sources.rs` は通るが旧 key を前提にしている可能性があり、移し替えは未着手。
4. **web の残り**: `web/api/model-assignments.ts` と `web/features/ops/model-assignments.tsx` が残る。後者は `/api/providers` を呼ぶので実行時は 404（ADR の削除対象）。cache domain の `providers` も残る（gen-types の記録どおり）。
5. **gui の残りはコメントのみ**: `gui/app/routes/accounts.tsx:70,1387`、`clusters.tsx:622`、`lib/routing-catalog.ts:2`、`lib/time-delta.ts:2` の `/providers` は説明文。動作には影響しない。
6. gui の check は既定の TMPDIR で `browser-phase2` の socket 試験が 1 件落ちる（既知。`TMPDIR=/local/x` で通る）。

## 提案

- **実装済み（2026-10-12、unit `accounts-rekey`。進捗 `agent-docs/progress/2026-10-12-accounts-rekey.md`、ADR 2026-10-11-llm-source-harness-selection の 2026-10-12 付記（accounts-rekey））**。旧 adapter key 路と `?adapter=` は消した。以下は提案の原文（記録として残す）: 残りを一つの unit `accounts-rekey` にまとめる: HTTP 路を `/api/v1/accounts/{source_id}/{account_id}` に作り直し、旧 adapter key の路を消す。`cos/ops/admin.rs` の表（ALLOWED/EXCLUDED）と ADR 2026-10-09-cos-operations-all-mutations の D3 表を同時に直す（registry 試験に掛かる）。試験は `llm_routes_admin.rs` に移し、旧 `accounts_admin.rs` の key 試験は消す。
- `llm-sources-catalog-health` を別 unit にする: `LlmSourceView` に catalog（model 一覧と override）と health（到達性・quota・cooldown の要約）を足し、schema を `UPDATE_SCHEMA=1` で再生成する。
- web の `model-assignments`（ts・tsx）と cache domain `providers` の削除は ui-v2 で扱う（既存の提案どおり）。
- 進捗の status は、上の 2 unit が入るまで `running` のまま。task 全体の完了判定は api-v2 の再計画後に行う。

## 本番への移行

本番 config・DB には触れていない。`[[providers]]`・`providers.d`・`[llm_proxy.models]` の書き換えは runtime 進捗（`2026-10-11-llm-source-harness-runtime.md`）の「本番への移行」節を参照（人が実行する）。
