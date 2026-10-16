---
task: api-ident
status: done
completed: 2026-10-16
---

# api-ident: POST /tasks で campaign 識別を受ける

## 実装済み
- `NewTaskBody` に `campaign`（`CampaignIdentity`）と `campaign_idempotency_key` を追加（`crates/task-api/src/handlers/tasks.rs`）。
  識別は task 作成と同じ transaction で `task_campaign_set_tx` により保存する。
- 冪等キーは task の label として照合する（呼び出し側が付け忘れてもサーバが label に足す）。
- 再送判定は `SqliteStore::task_campaign_resend(_tx)`（`store/decision_log.rs`）: 同じキー、または同じ試行
  （campaign/case/arm/repetition）の task があり識別が全欄一致なら 200 で既存 task、欄が違えば 409
  `campaign_identity_conflict`。別の case/arm/repetition は衝突せず 201。transaction 内でも再確認する。
- `campaign_idempotency_key` だけで `campaign` が無い、不正な `routing_setting` は 400。
- schema 再生成（`UPDATE_SCHEMA=1`、`web/api/generated` は `node web/scripts/gen-types.mjs`。この worktree に `gui/` は無い）。

## mock検証済み
- `crates/task-api/tests/campaign_ident.rs`: HTTP 経由のみ（識別の直挿入なし）。保存・label・再送 200・キー無し再送・
  別 rep は 201・キー違い識別/manifest 違いは 409・子は親の識別を継ぐ・campaign 無しは NULL・400 系。

## 実経路検証済み
- なし（一時 DB の HTTP 試験まで。本番 SQLite/KB は触っていない）。

## 承認待ち
- なし。

## 既存成果の確認
- 着手時 main=0e73be29。POST /tasks に識別の経路は無かったので新規実装。`task_campaign_set_tx` は既存を再利用。
- `CelerisExecutor::body` への送出と子/再試行/review の run への継承は後続 WU（executor / propagate）。

## 提案
- executor は `campaign` と `campaign_idempotency_key`（既存の label と同じ文字列）を body に入れる。
