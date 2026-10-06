# POST /cos/operations・GET /cos/operations/{o}（path 許可表と共有操作関数への監査 context）

---
tasks: [01M47997XM2GYH4E5J2QNAJZ7R]
status: done
completed: 2026-10-06
---

- `crates/task-api/src/cos/operations.rs`: `POST /api/v1/cos/operations` と `GET /api/v1/cos/operations/{o}`。
  - body `{idempotency_key, expected_revision, reason, policy_version, request:{method, path, body}}`。未知フィールド 400、
    reason 空・policy_version 欠落/空・expected_revision 欠落（null は可）・idempotency_key 空は 422。body・request.body の
    `actor`/`thread_id` は 422 `cos_identity_claim`。CoS run credential 以外（admin token）は 403。
  - id 類（thread_id・run_id）は `CosCaller`（credential）から、operation_id は daemon が ULID で採番。
  - `request_hash` = sha256(`METHOD\npath\n` + key を全階層で整列した body)。同 key 同 hash は既存の O を返し、
    同 key 異 hash は 409 `chat_conflict`（`cos_operation_find` で適用前に照合）。
  - 許可表 `ALLOWED`: `POST /api/v1/tasks`（task.create）・`POST /api/v1/tasks/{id}/comments`（comment.create）・
    `POST /api/v1/decisions/{id}/answer`（decision.answer）。URL/scheme/host・`//`・`..`/`.`/空 segment・query/fragment/`%`・
    `/api/v1/` 外・`/api/v1/cos` 再帰・未登録・method 不一致は 422 `cos_operation_not_allowed`。
  - 拒否（許可表外・request.body の型不一致・領域の検証/状態エラー）は `OperationAudit::reject` が cos_operations の rejected 行と
    `cos_operation` event（reason = `rejected: <理由>; requested reason: <CoS の reason>`）に残す。応答は元の problem。
  - GET: 他 thread の credential には 404（id の探りを防ぐ）。人の認証では読める。
- 共有操作関数（handler の本体を `(store, input, Option<&OperationAudit>)` に切り出し、handler は `None` で呼ぶ）:
  `handlers::tasks::create_task_op`・`handlers::task_actions::create_comment_op`（CoS は author `cos`）・
  `decisions::answer_op`（CoS は `by = cos`）。監査ありの経路は読み取りで計画を立て、書き込みは `cos_operation_apply` の
  closure 内で `_tx` 関数に渡す（領域・cos_operations・監査 envelope event・chat card event が 1 transaction）。
- 下回りの切り出し（挙動は不変）:
  - task-core: `SqliteStore::create_task_tx`・`comment_add_tx`・`decision_resolve_apply_tx`・`set_task_expected_write_paths_tx`、
    `cos_operation_find(thread_id, key)`。既存の `_impl` はこれらを呼ぶ。
  - task-ops: `comment::plan_human_comment`/`finish_human_comment`、`decision::plan_answer`/`finish_answer`。
    既存の `post_human_comment_as`・`answer` はこれらの合成。

検証:
- `cargo test -p task-api --test cos_operations` → 6 passed（cos_chat_ops_api_ 6 件: 許可表外 8 種の 422 と events の reason、
  冪等 same/409、task 作成の領域・O・監査 envelope・card の同時書き込みと失敗時に何も残らないこと、comment・decision 回答、
  body 形と身元詐称、GET の thread 範囲）
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0、`cargo fmt --all --check` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、4053 passed / 0 failed / 13 ignored（既存 handler の試験を含む）

未解決事項:
- 残りの領域（approval・execution・project・knowledge）は許可表に入れていない（ops-domains 葉）。
- 監査ありの後始末は commit 後の別書き込み: blocked task への回答コメントの `settle_pending_approvals`、
  `needed_before: [self]` の取り下げ回答での節点中止（`cancel_node`）。どちらも人の経路と同じ冪等な後続書き込み。
- `expected_revision` は記録するだけで、領域の版との照合はまだしない（領域ごとの版の定義が要る）。
- 範囲: 共有操作関数の切り出しのため task-core（store の `_tx`）と task-ops（plan/finish）にも触れた。

提案:
- O の JSON 形（`OperationView`）と problem code `cos_operation_not_allowed` の schema・gui-api.md 追記は schema 葉で行う。
