---
title: 健全性v2 R5 map-api-a — task-api tests a〜c の試験に契約を付け、組を判断した
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 map-api-a — task-api tests a〜c の試験に契約を付け、組を判断した

完了日: 2026-11-10

WorkUnit `map-api-a`。commit 済みの雛形 [`mapping/api-a.json`](mapping/api-a.json) を埋めた（`--scaffold` は走らせていない）。
手順は [`fill-guide.md`](fill-guide.md)。基準は `$CELERIS_WU_BASE`。`contracts.py`・crates/ の試験・ほかの fragment は変えていない。

## 結果
- scope `^crates/task-api/tests/[a-c]` の 50 file・320 試験の全部に capability と 1 行の契約を付けた。
  - file ごとの `capability`・`contract` に加え、314 試験に試験ごとの契約（`fns`）を書いた。file の契約だけの試験は、試験が 1 本の file（`browser_live.rs`・`browser_restore_deliver.rs`・`browser_restore_live_session.rs`・`campaign_ident.rs`・`concurrency.rs`・`cos_chat_attach_pin.rs`）の 6 本。
  - 不変条件の試験（scan の `invariant=true`）は 122 本。どの組の `members` にも入っていない（`equivalent_scenario` の 1 組の member は invariant=false）。
  - `tests/common/mod.rs`・`tests/common/cos_ops.rs` は試験を持たないので `files` に無い。helper として組の member に出る。
  - `CELERIS_USERNS_TESTS` が無いと skip する試験も載せた: `browser_h3_injection.rs::production_h3_injects_once_without_exposure`（`inner_h3_injection` は `CELERIS_H3_INNER` のときだけ動く子 process の入口）、`browser_restore_deliver.rs`、`browser_restore_live_session.rs`。

### capability ごとの試験の数

| capability | 試験 | うち invariant |
|---|---|---|
| `cos-inbox-triage-operations-audit` | 91 | 25 |
| `cos-chat-threads-runs` | 53 | 32 |
| `http-api-auth-principals` | 27 | 11 |
| `cluster-admin-connect`（extra） | 22 | 17 |
| `cos-authority-ledger`（extra） | 21 | 0 |
| `console-feed`（extra） | 17 | 0 |
| `approvals-standing-rules`（extra） | 14 | 0 |
| `browser-policy-broker-approval` | 14 | 11 |
| `pre-review-sync-auto-resolve-integration-repair` | 12 | 12 |
| `decisions-plan-gate` | 11 | 0 |
| `browser-site-policy-trusted-devices` | 8 | 8 |
| `cron-jobs` | 8 | 0 |
| `org-conversation`（extra） | 7 | 0 |
| `browser-live-view-control` | 6 | 4 |
| `campaign-eval` | 4 | 0 |
| `project-controller` | 3 | 2 |
| `review-runs-final-review` | 2 | 0 |
| 計 | 320 | 122 |

判断に迷ったものの割り当て:
- `changes.rs`（ADR-0043 D5 の merge・pr・discard）は B0 で `crates/task-ops/src/changes.rs` を owner に持つ `pre-review-sync-auto-resolve-integration-repair` に入れた。
- `browser_doctor.rs`（`GET /browser/readiness`）は「本番の有効化」を持つ `browser-site-policy-trusted-devices` に入れた。
- `browser_restore_*.rs`（identity の復元）は identity を持つ `browser-live-view-control` に入れた。
- `cos_plan_conforms*.rs` は b0_hint に従い `decisions-plan-gate`。ほかの CoS の定型の代答（job_wait_extend・review_rework・routine）は `cos-inbox-triage-operations-audit`。
- file の中で契約の持ち主が違う試験だけ capability を上書きした: `browser_waits.rs` と `authority_ledger.rs` の project wake 3 本 → `project-controller`、`cos_ops_mutations.rs` の再 review の再開 2 本 → `review-runs-final-review`、`console*.rs`・`cos_auth.rs`・`cos_operations_review_report.rs` の CoS chat の試験 → `cos-chat-threads-runs`、`cos_live_fix_d2.rs` の監査の 1 本 → `cos-inbox-triage-operations-audit`。

### extra_capabilities（B0 に無いもの、5 件）

| id | owner_paths | B0 に無い理由 |
|---|---|---|
| `approvals-standing-rules` | `crates/task-api/src/approvals.rs`・`crates/task-core/src/approval.rs`・`crates/task-ops/src/approval.rs` | `decisions-plan-gate` は決定の要求と plan gate で、認可要求と standing rule の owner を含まない |
| `cos-authority-ledger` | `crates/task-core/src/authority.rs`・`crates/task-api/src/handlers/authority.rs` | `cos-inbox-triage-operations-audit` は triage と操作の監査で、許可の台帳を owner に含まない |
| `cluster-admin-connect` | `crates/task-api/src/handlers/clusters.rs` | クラスタの接続・設定の API を持つ capability が無い |
| `console-feed` | `crates/task-api/src/console.rs`・`crates/task-ops/src/console.rs` | console の feed と指示の経路を持つ capability が無い |
| `org-conversation` | `crates/task-api/src/conversation.rs`・`crates/task-ops/src/conversation.rs` | node との対話（task 経路の conversation）を持つ capability が無い |

## 組の判断（37 組: duplicate_setup 36・equivalent_scenario 1）
組の id（`dup-…`・`eq-…`）は [`mapping/api-a.json`](mapping/api-a.json) の `groups` にある（id は hex を含み、進捗の commit hash 検査に掛かるのでここには書かない）。
採った組 26（duplicate_setup 25・equivalent_scenario 1）、採らなかった組 11。採った組はすべて `action: merge`（helper を tests/common へ寄せる・表駆動に束ねる）。消す（delete）案は無い。

### 採った組

| helper（file 数）・試験 | 案 | mutation_target |
|---|---|---|
| `g`（14） | `common::get_admin` を呼ぶ | `crates/task-api/src/middleware.rs` |
| `p`（11） | `common::post_admin` を呼ぶ | 同上 |
| `d`（5） | `common::delete_admin` を足して呼ぶ | 同上 |
| `env_with_token` ほか（8） | `common::admin_env` を呼ぶ | 同上 |
| `admin_env`・`env_with_token`（3） | `common::admin_env` を呼ぶ | 同上 |
| `admin_headers`・`admin`（4） | `common::admin_headers` を呼ぶ | 同上 |
| `admin`・`auth`（3） | `common::admin_headers` を呼ぶ | 同上 |
| `get`・`g`（2） | `common::get_admin` を呼ぶ | `crates/task-api/src/browser_site_policies.rs` |
| `p`（campaign 2） | `common::post_admin` を呼ぶ | `crates/task-api/src/handlers/tasks.rs` |
| `op_body`（common/cos_ops.rs と 1） | `common::cos_ops::op_body` を use | `crates/task-api/src/cos/operations.rs` |
| `db`（common/cos_ops.rs と 8） | `common::cos_ops::db` を use | 同上 |
| `cos_bearer`（2） | `common::cos_ops::cos_bearer` を use | `crates/task-core/src/chat/credential.rs` |
| `post_op`（2） | common/cos_ops.rs へ移す | `crates/task-api/src/cos/operations.rs` |
| `count`（4） | common/cos_ops.rs へ移す | 同上 |
| `decision_answered_by`（3） | common/cos_ops.rs へ移す | `crates/task-ops/src/decision.rs` |
| `raise_decision`（4） | common/cos_ops.rs へ移す | 同上 |
| `env`（名前付き token 2 本、3） | tests/common/mod.rs へ移す | `crates/task-api/src/principal.rs` |
| `env_with_attachments`（3） | tests/common/mod.rs へ移す | `crates/task-api/src/chat/attachments.rs` |
| `job_body`（2） | tests/common/mod.rs へ移す | `crates/task-api/src/cron_jobs.rs` |
| `git`（2） | tests/common/mod.rs へ移す | `crates/task-ops/src/changes.rs` |
| `ingest_plan_gate`・`conforms`・`ago`（plan_conforms の 2 file） | 3 つを tests/common の plan_conforms 用 module へ一緒に移す | `crates/task-api/src/cos/basis/plan_conforms.rs` |
| `browser`（復元の 2 file） | tests/common の browser 用 module へ移す | `crates/task-api/src/browser_identity.rs` |
| `bridge_binary`（browser の 2 file） | tests/common の browser 用 module へ移す | `crates/task-worker/src/browser_credential.rs` |
| `chat_attach_api_requires_client_upload_id` と `..._rejects_duplicate_file_fields` | 2 つの multipart 本文を行に持つ表駆動の 1 本に束ねる（欄の欠けと file 欄の重複は別の分岐なので、どちらの入力も残す） | `crates/task-api/src/chat/attachments.rs` |

`keep_tests` は各組の `users` から選んだ（`users` が空の `browser`・`bridge_binary` の組は member の file の試験）。

### 採らなかった組（11）
すべて本体が 1〜3 行の包みか 1 回の呼び出しで、写しを 1 か所にまとめても build と読みやすさが変わらないもの（fill-guide 4 節）。
- `at`（固定 epoch の時刻）、`thread`（`project_cos_thread(..).id`）、`skip`（userns_gate の呼び出し）
- `auth`・`admin`・`auth_header`（`format!("Bearer {TOKEN}")`、9 file）、`bearer`（5 file）、`human`・`web`（header 配列）
- `path`・`live`、`resolve_path`（URL の format）
- `get`（`send` と `get_admin` の合成）、`keypair`（ring の 2 呼び出し）、`finish_run`（`chat_run_finish` の 1 呼び出し、users 2）

## 気づいたこと（走査の誤りではない）
- `tests/common/cos_ops.rs::cos_bearer` と `cos_operations.rs`・`cos_triage.rs` の `cos_bearer` は、`project_cos_thread` を `super::` で呼ぶか `common::` で呼ぶかだけが違う。本文の token が違うので走査の hash は分かれ、common 側は `cos_bearer` の組の member に入らない。案は common 側へ寄せる形にした。
- 同じく `common::get_admin`・`post_admin`・`admin_env` は写しと本文の形（`admin_headers()` を使うか `format!` で組むか、`to_string` か `into` か）が違うので組に入らない。採った組の案はこれらの既存 helper を呼ぶ形にした。
- fill-guide 7 節に当たること（capability を決められない試験・走査の取りこぼし・scope の外の変更）は無かった。

## 変更
- `R5 map-api-a: task-api tests a〜c の 320 試験に契約を付け、37 組を判断する` — `mapping/api-a.json` を埋め、この進捗を足した。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --check-fragment agent-docs/progress/2026-11-10-health-v2-r5/mapping/api-a.json` → exit 0（`mapping/api-a.json: ok`）
- 雛形の `scan`・`scope`・各 file の `tests`・`b0_hint`・各組の `members`・`users` が commit 済みの雛形と同じことを Python で照合した → 差分なし
- `git diff --name-only "$CELERIS_WU_BASE"` → `agent-docs/progress/2026-11-10-health-v2-r5/` の 2 file だけ（crates/ の変更なし）

## 未解決
- 無し。

## 提案
- 無し。
