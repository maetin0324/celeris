# CoS run credential middleware と checkpoint API

---
tasks: [01M47997XM2GYH4E5J2QNAJZ7R]
status: done
completed: 2026-10-06
---

- 判別: bearer が `celeris-cos-run.` で始まれば CoS run credential（`task_api::cos::COS_BEARER_PREFIX`）。admin token とは比べず、接頭辞の後ろを `cos_run_credential_verify` で検証する（未知・期限切れ・失効は 401、detail で区別。admin token 未設定の構成でも検証する）。発行は `task_api::cos::issue_run_bearer(_at)`（接頭辞を付けて返す）。後段 chat-run はこれを使う。
- `middleware::guard` は既存の検査の後で `cos::authenticate` を呼び、`CosCaller {thread_id, run_id}` を request extension に入れる。ヘッダ（`x-celeris-actor` 等）は読まない。CoS の要求 body に `actor`・`thread_id` があれば 422 `cos_identity_claim`。
- `/api/v1/cos/` 以外への POST/PUT/PATCH/DELETE は 422 `cos_audit_context_required`（`instead: /api/v1/cos/operations`）。拒否は `cos_operation_apply` に失敗する closure を渡して記録する（cos_operations の rejected 行と reason 付き `cos_operation` event。target_kind=`api`、target_id=path、action=method、policy_version=`api-guard-1`）。
- GET は既存の閲覧として通す。`require_admin` は CoS 形の bearer を通す（guard が検証済みで、変更系は `/api/v1/cos/` しか handler に届かない、という不変条件に依る）。
- `POST /api/v1/cos/threads/{t}/checkpoint`: CosCaller が無ければ 403、thread/run が credential と違えば 403。store の ChatError を 409/413/422/404 に写す。
- router: `crates/task-api/src/cos/mod.rs` の `routes()` が checkpoint と `operations::routes()`（ops-api 用の空 stub）を merge する。

検証:
- `cargo test -p task-api --test cos_auth` → 7 passed（cos_chat_ops_auth_ 5 件、cos_chat_ops_checkpoint_api_ 2 件）
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `CELERIS_TEST_JOBS=4 bash scripts/dev/test-parallel.sh` → exit 0、4047 passed / 0 failed / 13 ignored

未解決・提案:
- 接頭辞で始まる admin token は使えない（CoS として扱われ 401）。config 検査で弾く案。
- 新 route と problem code（cos_audit_context_required・cos_identity_claim・cos_credential_required）の schema・gui-api.md 追記は schema 葉で行う。
