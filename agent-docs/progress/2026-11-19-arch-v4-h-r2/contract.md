---
title: アーキ v4 H R2 contract — account の HTTP と CoS の契約を移す前のコードで試験に固定する
tasks: [01M7T5KM3FW3430R5DT4QG5QKV]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# contract（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- `crates/task-api/src/handlers/accounts.rs` と `crates/task-api/src/cos/ops/admin.rs` の account の処理を移す前のコードで、
  欠けていた場面の試験を 6 件足した（CoS 2 件・HTTP 4 件）。全件が今のコードで通る。`crates/task-api/src` は変えていない。
- 足した試験は、後の unit（http-tools・accounts）が処理を移した後も同じ status・problem の `type`・拒否の理由・監査の記録・
  応答の JSON の形を返すことを確かめる。
- HTTP と CoS で値が違う場面が 4 つある（§3）。どちらも今の値のまま固定した。上限の解除の key の違い（§4）は不具合の疑いがあるので、
  試験に固定せず未解決に書いた。

## 1. 足した試験

各試験の doc comment に、守る不変条件と既存の試験で検出できない理由を書いた（規約 10）。

| file | 試験 | 範囲 | 固定する内容 |
| --- | --- | --- | --- |
| `crates/task-api/tests/cos_delivery_suite/cos_ops_admin_config.rs` | `cos_ops_admin_config_account_refusals_are_recorded_with_their_reason` | (a) | CoS の 4 操作の拒否 17 場面。各場面で problem（status・`code`・`type`）・detail の文・rejected の行 1 件（idempotency key ごと）・行の `target_kind`/`target_id`・同じ target の監査 event が rejected で終わること・applied の event が無いこと。daemon に届いたのは in-use の削除 1 件だけ |
| 同上 | `cos_ops_admin_config_account_success_results_keep_their_shape` | (b) | `account.create` の result が `AccountView` の全欄（17 欄の値）、作り直しで古い上限を解除する（daemon へ `claude-code` の key で送る）、`account.limit_set` の pool と暗黙 account の view、`account.check` の `{result, checked_at, detail}`（`usage` の欄が無い）、`account.delete` の `{id, adapter: "claude-code", removed: true}`。daemon に届いた要求の列（5 件・各 1 回） |
| `crates/task-api/tests/llm_accounts_admin.rs` | `management_routes_refuse_missing_unknown_mcp_and_origin_callers` | (c) 認可 | token を設定した構成で、作成・削除・確認・上限の 4 路が bearer 無しと未知の bearer を 401 `unauthorized`、MCP 専用 token を 403 `forbidden`、`Origin` 付きを 403 `origin_forbidden` で拒む。ディレクトリも daemon も触らない |
| 同上 | `management_success_responses_keep_their_shape` | (b)(c) | HTTP の作成は 201・`Location`・`AccountView` の全欄、確認は 200 と `{result, checked_at, detail}`、削除は 200 と `{}` |
| 同上 | `implicit_source_with_another_account_name_is_not_found_for_delete_and_check` | (c) | 暗黙 source に別の account 名を付けた削除・確認は 404 `account_not_found`（detail は account 名）で、daemon に届かない |
| 同上 | `unknown_body_fields_are_bad_requests` | (c) | 作成の本文の未知の欄は 400 `bad_request`（`invalid JSON body:`）でディレクトリを作らない。上限の本文の未知の欄は 400 `bad_request` |

CoS の拒否の 17 場面と、同じ場面の HTTP（既存の試験か今回の試験で固定済み）:

| 操作 | 場面 | CoS（今回固定） | 記録の target | HTTP | HTTP を固定した試験 |
| --- | --- | --- | --- | --- | --- |
| create | 未知の source `nope` | 400 `bad_request`「source does not have a managed account pool」 | `account`/本文の id | 404 `account_not_found` | `unknown_source_is_not_found`（既存） |
| create | 暗黙 source | 400 `bad_request`（同じ文） | `account`/本文の id | 409 `implicit_account` | `implicit_accounts_are_listed_and_patchable_by_source_and_refused_otherwise`（既存） |
| create | root の無い pool（`codex_oauth`） | 409 `accounts_unavailable` | `account`/本文の id | 404 `account_not_found` | `codex_source_without_codex_root_is_not_found`（既存） |
| create | 不正な id | 400 `bad_request` | `account`/`../escape` | 400 `bad_request` | `create_account_makes_a_directory_and_rejects_duplicates_and_bad_ids`（既存） |
| create | 重複 | 409 `account_exists` | `account`/`dup` | 409 `account_exists` | 同上（既存） |
| create | 本文の未知の欄 | 422 `validation`（`request.body:`） | `api`/path | 400 `bad_request` | `unknown_body_fields_are_bad_requests`（今回） |
| limit_set | 未知の source | 404 `account_not_found` | `account`/source | 404 | `unknown_source_is_not_found`（既存） |
| limit_set | root の無い pool | 404 `account_not_found` | `account`/source | 404 | `implicit_accounts_…`（既存、`claude_oauth` に root が無い構成） |
| limit_set | 暗黙 source に別の名前 | 404 `account_not_found` | `account`/account 名 | 404 | `implicit_accounts_…`（既存） |
| limit_set | 範囲外の `max_runs` | 400 `bad_request`（pending の後に rejected） | `account`/account 名 | 400 | `implicit_accounts_…`（既存） |
| delete | 未知の source | 404 `account_not_found`（detail は source） | `account`/account 名 | 404 | `unknown_source_is_not_found`（既存） |
| delete | root の無い pool | 404 `account_not_found` | `account`/account 名 | 404 | `pool_without_root_lists_nothing_and_management_is_not_found`（既存） |
| delete | 暗黙 account | 409 `implicit_account`「… cannot be deleted」 | `account`/source | 409 | `implicit_accounts_…`（既存） |
| delete | 暗黙 source に別の名前 | 404 `account_not_found` | `account`/account 名 | 404 | `implicit_source_with_another_account_name_is_not_found_for_delete_and_check`（今回） |
| delete | daemon が使用中で断る | 409 `account_in_use`（pending の後に rejected） | `account`/account 名 | 409 | `delete_account_in_use_is_conflict`（既存） |
| check | 暗黙 account | 409 `implicit_account`「… cannot be checked」 | `account`/source | 409 | `implicit_accounts_…`（既存） |
| check | root の無い pool | 404 `account_not_found` | `account`/account 名 | 404 | `pool_without_root_…`（既存） |

## 2. 既に覆われていた場面（今回は足していない）

- HTTP（`llm_accounts_admin.rs`）
  - token を設定しない構成で、管理系 7 路（作成・削除・確認・login・login 取消・login code・上限）が 401 `unauthorized`。一覧の GET は 200（`management_routes_all_require_a_token`）
  - root の無い pool の作成・削除・確認が 404。未知の source の作成・削除・上限が 404（`pool_without_root_…`・`unknown_source_is_not_found`）
  - 作成: 不正な id の 400、201 と `Location`、0700 のディレクトリ、重複の 409（`create_account_makes_a_directory_and_rejects_duplicates_and_bad_ids`）
  - 削除: daemon への委譲と `.removed/` への移動、無い account の 404、`admin_tx` が無いときの 409 `accounts_unavailable`、使用中の 409（`delete_account_*` の 3 件）
  - daemon の `Unavailable` が 409 `accounts_unavailable` になる（`account_admin_unavailable_error_maps_to_409_not_500`）
  - source から内部の adapter 名（`codex`）への変換と、codex の login code の 409（`management_endpoints_forward_the_source_as_its_adapter`・`login_code_is_not_supported_for_codex`）
  - 上限: pool と暗黙 account の PATCH の成功、一覧への即時の反映、暗黙 source の名前の不一致の 404、範囲外の 400、`null` で既定へ戻す、暗黙 account の作成・削除・確認・login の 409（`pool_account_limit_is_patchable_by_source_and_account`・`implicit_accounts_…`）
- CoS（`cos_ops_admin_config.rs`）
  - `account.check` と `account.delete` の成功: 直接の呼び出しが 422 `cos_audit_context_required`、envelope の監査 event が pending → applied、同じ envelope の再送で effect が 2 回走らない、delete の `removed == true`（`cos_ops_admin_config_daemon_and_external_effects_run_once`）
  - 未知の source の `account.check` が 404 で rejected の行を残す（同上。行の数だけを見る）
- 2 file の外: standby の構成で account の管理系が 503（`crates/task-api/tests/unmapped_suite_b/standby.rs`）

今回の CoS の成功の試験は `run_domain`・`run_external` を使うので、`account.create`・`account.limit_set` の直接の呼び出しが
422 `cos_audit_context_required` になることも新たに確かめる。

## 3. HTTP と CoS で値が違う場面（今の値のまま固定した）

1. 作成で source が managed pool でないとき: CoS は 400 `bad_request`（未知・暗黙を区別しない）、HTTP は未知が 404 `account_not_found`・暗黙が 409 `implicit_account`。
2. 作成で pool の root が無いとき: CoS は 409 `accounts_unavailable`（`create_account_op` の中で判定）、HTTP は 404 `account_not_found`（handler が先に root を見る）。
3. 削除の成功の応答: CoS の result は `{id, adapter, removed}`（adapter は内部名 `claude-code`）、HTTP の本文は `{}`。
4. 作成の本文の未知の欄: CoS は 422 `validation`、HTTP は 400 `bad_request`。

account の処理を共有の module へ移す unit（accounts）は、この 4 つの違いを変えない（task の「response・拒否理由・認可は変えない」）。
変えるなら API の契約の変更なので、実装の前に人の承認が要る。

## 4. 未解決

- 作成のときの古い上限の解除の key が HTTP と CoS で違う（コードを読んだ所見。試験では未確認）。CoS の `account.create` は
  `clear_stale_limit` に内部の adapter 名（`claude-code`）を渡す。HTTP の `create_account` は `view.source_id`（`claude_oauth`）を渡す。
  上限の保存値は adapter 名の key で読み書きされる（`set_account_limit_effect` が adapter 名を送り、`account_items` が
  `(adapter.as_str(), id)` で引く）。このため HTTP で作り直した account は古い上限を引き継ぐ疑いがある。今の挙動を試験に固定すると
  不具合を固定することになるので、HTTP 側は固定していない。CoS 側（adapter 名で解除する）は
  `cos_ops_admin_config_account_success_results_keep_their_shape` で固定した。

## 5. 提案

- accounts unit で create の plan と作用を 1 つにまとめるとき、古い上限の解除を adapter 名の key に揃える（HTTP の不具合の修正）。
  HTTP の作成で古い上限が消えることを確かめる試験を同じ変更で足す。応答・拒否理由・認可は変わらない。

## 6. 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-api --test llm_accounts_admin --test cos_delivery_suite` → exit 0（55 passed）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6132 passed・21 skipped、web の tsc -b と unit 705 passed、repo_health_gate all checks passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo clippy -p task-api --test llm_accounts_admin --test cos_delivery_suite -- -D warnings` → exit 0（足した試験の lint。`-p task-api --tests` 全体では、この WorkUnit が触れていない `llm_routes_admin` が base から clippy::type_complexity で落ちる）
- `cargo fmt --all -- --check` → exit 0
- `git diff --name-only $CELERIS_WU_BASE -- crates/task-api/src` → 出力なし（`crates/task-api/src` は変えていない）

## 変更

- `arch-v4 H R2 contract: account の HTTP と CoS の契約を試験に固定する` — 上の 6 件の試験と、この file
