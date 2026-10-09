---
tasks: [01M4F5KS8E1MXZDNJTRAVFJESW]
wu: ops-admin-config
status: done
completed: 2026-10-09
---
# ops-admin-config: admin の残り全部を監査付きで登録

`crates/task-api/src/cos/ops/admin.rs` の `PENDING` は空（29 行を ALLOWED へ）。commit 23636397〜115e96ac。

## 登録した操作

- A（domain 書き込みと監査を 1 transaction）: `model_assignment.preview`・`model_role.replace/preview`
  （下見は書き込まず影響を `result` に記録）、`model_override.put/delete`、`org.create/update/delete`・
  `org.browser_settings`・`org.skill_mount/unmount`、`repo.create/update/delete`、`cluster.settings_put`。
  task-core に `model_catalog_set_override_tx`・`model_catalog_delete_override_tx`・`model_role_members_replace_tx`・
  `org_upsert_tx`・`org_delete_tx`・`repo_create_tx/update_tx/delete_tx`（`repo_get_tx` を pub に）・
  `cluster_settings_set_tx` を足し、既存の store 関数も同じ `_tx` を使う。handler と CoS は `plan_*`/`*_op` を共有。
- A（file 効果）: `provider.create/update/delete`・`account.create`。SQLite に入らない file 書き込みは
  `OperationAudit::apply_checked` の closure の最後で行い（`cos::operations::file_op`）、失敗は rollback して
  rejected 行になる。commit 自体の失敗で file だけ残る窓は残る（下の未解決）。
- C（外部効果。pending → applied、再送は再実行しない、変更前の拒否は rejected、結果不明は pending）:
  `provider.check`・`account.check`・`account.delete`（daemon が退避するので C）・`model_catalog.discover`・
  `notify.test`・`daemon.reload`・`daemon.replay`・`release.promote`（人の決定で許可）、`skill.put/delete`
  （KB の git 書き込みなので objective の A ではなく `knowledge.page_put` と同じ C にした）。
  daemon channel の handler 本体は `Effect<T>` を返す共有 async 関数（`reload_effect` 等）にし、CoS は
  blocking dispatch の中で `Handle::current().block_on` で同じ関数を呼ぶ。
- 秘密: provider の `env` に `CREDENTIAL_KEYS` があれば 422 `secret_operations`、記録の body は redacted
  （秘密の値を監査行に残さない。人の決定 secrets=exclude）。account の `?adapter=` は body `{"adapter"}`。
- 共通: `OperationAudit::apply_checked`（domain の ApiProblem を同じ status で返し rejected 行を残す）・`record`
  （書き込まない操作）・`Effect`/`effect_result`/`effect_value`/`file_op`。ALLOWED は先に一致した行が勝つので
  `assignments/roles/{tier}` を `{source}/{tier}` より前に置いた。
- celerisctl（CoS credential）: `models discover|assign|unassign`（`models::request_of` を直接実行と共有）、
  `replay`（`--check/--apply` なし）、`approve|reject|accept|cancel|rereview` を `cos_mapped` で包む。`models list` は読み取り。
- 文書: skill `operations.md` の表（lib 試験で ALLOWED と一致）・登録待ちの節（surface だけ）・`SKILL.md`・
  `production.md`（promote は人の依頼と verify 確認の後だけ）。`docs/api/v1/gui-api.md` §3.129 の表を ALLOWED と
  一致させ（tasks の gate・通知の欠けも追記）、`scripts/sync-gui-docs.sh` で gui 写しを同期。

## 証拠

- `cargo nextest run -p task-api --test cos_ops_admin_config` → 5 passed（llm/models、org+skills、repos+cluster、
  providers+account、daemon/外部効果を fake の admin channel・release 置き場・発見 hook で。各効果 1 回、再送で再実行なし、
  直接呼び出し 422、拒否は rejected 行）
- `cargo nextest run -p celerisctl`（短い TMPDIR）→ 167 passed
- `cargo clippy --workspace -- -D warnings` → exit 0（task-api・task-core・celerisctl の `--all-targets` も exit 0）
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0（passed 4941、failed 0、ignored 14、doctest exit 0）
- `bash scripts/sync-gui-docs.sh --check` → up to date

## 未解決

- file 効果（provider・account 作成）は SQLite commit の失敗時に file だけ残り得る（記録は無い）。
- `celerisctl skills import`（ディレクトリから複数 skill）は包んでいない。CoS は `PUT /api/v1/skills/<name>` を api-request で送る。
- `gate.rs` の `run_approve` の CoS 分岐は `cos_mapped` が先に処理するので到達しない（ops-closeout で整理）。

## 提案

- 外部効果の pending を人が確かめる画面（`needs_remediation`）に、daemon channel の操作種別を出すとよい。
