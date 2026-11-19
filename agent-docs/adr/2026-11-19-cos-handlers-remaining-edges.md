---
tasks: [01M7T5KM3FW3430R5DT4QG5QKV]
---
# ADR 2026-11-19: handlers ⇄ cos の残りの辺（account の 9 行と HTTP の道具）を task-api の SCC の外の module へ出す

- 日付: 2026-11-19
- 状態: Accepted（実装前。この ADR を書いた WorkUnit `adr` は `crates/` を変えていない。今のコードに当てる契約試験は WorkUnit `contract`、
  D1 は `http-tools`、D2・D3 は `accounts`、after の測定と進捗の完了は `after` が行う）
- 関係: [ADR 2026-11-18 org の共有 use-case](2026-11-18-org-write-shared-use-case.md)（同じ形を account に当てる）、
  [ADR 2026-10-09 CoS の全変更操作](2026-10-09-cos-operations-all-mutations.md)（不変条件 `cos-audit-same-tx`）、
  [ADR 2026-10-27 API の主体](2026-10-27-api-principals.md)（`api-principal-from-token`）、
  [ADR 2026-10-08 account の同時実行の上限](2026-10-08-account-concurrency-limits.md)（§(c)〜(e)。上限・暗黙アカウント）、
  [2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- 材料: アーキv4 H の [final-report.md](../progress/2026-11-19-arch-v4-h/final-report.md) の提案 1・2 と
  [struct-deps.md](../progress/2026-11-19-arch-v4-h/struct-deps.md)。
- 計測: [measure.snippet](../progress/2026-11-19-arch-v4-h-r2/measure.snippet)（H の struct-deps の `c2.py` を元にした。head の検査器、
  `super::` を辺にする）。before は開始時の `git merge-base HEAD main`（main の commit「integrate wu/close (phase close)」、アーキv4 G の close
  の統合）で、値は [metrics.tsv](../progress/2026-11-19-arch-v4-h-r2/metrics.tsv)。

## 1. 根本原因

症状: task-api の 49 単位の SCC（head の検査器）の中で `handlers` と `cos` が互いを参照する。before の辺は handlers → cos が 19 行、
cos → handlers が 50 行（`crate::` 46・`super::` 4）。このうち、この ADR が対象にする行は次のとおり。

| 向き | 行 | 中身 |
|---|---|---|
| cos → handlers | `cos/ops/admin.rs:424,445,451,469,480,503,512,526,531`（9 行。`crate::` 5・`super::` 4） | CoS の `account.create`・`account.limit_set`・`account.delete`・`account.check` が `handlers::accounts` の `pool_adapter`・`is_implicit_source`・`resolve_pool_account`・`create_account_op`・`clear_stale_limit`・`set_account_limit_effect`・`delete_account_effect`・`check_account_effect` を呼ぶ |
| cos → handlers | `cos/inbox.rs:32,466,472`・`cos/knowledge_use.rs:22,47`・`cos/mod.rs:32,259`・`cos/operations.rs:30,632,845`・`cos/override_op.rs:12`・`cos/review.rs:27`（12 行） | CoS の route が HTTP の道具 `ApiResult`・`Params`・`json_response`・`no_query`・`read_json` と `cluster_ids` を `handlers` から import する |
| handlers → cos | `handlers/accounts.rs:10,354`・`handlers/reload.rs:6`・`handlers/system.rs:129,139`（5 行） | HTTP adapter が作用の型 `Effect` と `effect_result` を `cos::operations` から import する |

cos の外でも、SCC の 29 単位が HTTP の道具だけのために `handlers` を import している（計 57 行。cos の 12 行を含む。measure.snippet の
辺の excerpt を `handlers::` の後の名前で分けた見積り）。

原因は 3 つある。

1. HTTP の道具（本文の読み取り・JSON の応答・path の値）が route の表を持つ `handlers.rs` にある。`handlers.rs` は route を組むために
   ほぼ全部の HTTP の module を import するので、道具を使う module はすべて `handlers` と循環する。
2. account の業務処理（plan: source → adapter の解決、作用: ディレクトリの作成・上限の保存・削除・検査）が HTTP adapter の
   `handlers/accounts.rs` にある。CoS adapter（`cos/ops/admin.rs`）は業務処理を使うために HTTP adapter を import する（D の org と同じ形）。
3. 作用の約束 `Effect<T>`（外側の `Err` は celeris に届いたか分からない失敗、内側の `Err` は検査か celeris が断った失敗）と
   `effect_result` が CoS adapter の `cos/operations.rs` にある。HTTP adapter は作用の戻り値の型のために CoS を import する。

## 2. 責務 owner

| 責務 | owner（after） | 今の場所 |
|---|---|---|
| HTTP の道具（`ApiResult`・`Params`・`json_response`・`read_body`・`read_json`・`no_query`） | 新しい `crates/task-api/src/http_io.rs` | `handlers.rs` |
| 設定された cluster の id の一覧 | `ApiState::cluster_ids`（`state.rs`） | `handlers.rs` の `cluster_ids` |
| account の plan と作用 | 新しい `crates/task-api/src/account_write.rs` | `handlers/accounts.rs` |
| HTTP の account の route（認証・query・本文・応答） | `handlers/accounts.rs`（HTTP adapter） | 同じ |
| CoS の account の操作（path の解析・監査の拒否・`file_op`・`external`） | `cos/ops/admin.rs`（CoS adapter） | 同じ |
| 作用の約束 `Effect`・`effect_result` | `problem.rs`（`ApiProblem` の owner） | `cos/operations.rs` |
| 操作の記録に値を写す `effect_value` | `cos/operations.rs`（CoS だけが使う） | 同じ |

`http_io`・`account_write`・`problem` は SCC の外に置く。`http_io` は `problem`・`query`・crate root の定数 `MAX_BODY_BYTES`（`lib.rs`。
crate root への `use` は deps.cycle の辺にならない）と外部 crate だけを使う。`account_write` は `problem`・`admin`・`accounts`・`types`・
`task_core`・`task_ops` と外部 crate だけを使い、`state`・`middleware`・`handlers`・`cos` を import しない（どれかを import すると
`handlers → account_write → state → … → handlers` の循環に入る）。

## 3. 既存の再利用先

- 形: [ADR 2026-11-18 org の共有 use-case](2026-11-18-org-write-shared-use-case.md)。plan と apply を SCC の外の module に置き、HTTP と
  CoS の adapter が一方向に呼ぶ。
- 作用の部品: `crate::admin::call`（celeris への依頼と timeout）、`crate::accounts`（`valid_account_id`・`scan_accounts` ほか）、
  `ApiProblem` の constructor（`account_not_found`・`accounts_unavailable`・`standby` ほか）。新しく書かない。
- CoS の監査: `cos/operations.rs` の `OperationAudit`・`file_op`・`external`・`audit.reject`（変えない）。
- 試験: 7 節の試験と、WorkUnit `contract` が今のコードに当てて足す account の契約試験。

## 4. 選択肢

| 案 | 内容 | 判定 |
|---|---|---|
| A | `handlers.rs` に `pub(crate) use crate::http_io::*` を残して呼び出し元を変えない | 不採用。呼び出し元の辺が `handlers` に残り SCC が減らない。re-export は旧い経路の残り（破壊的変更の規則） |
| B | account の作用を trait（`AccountPort` など）にし、`ApiState` に実装する | 不採用。抽象を足すだけで辺の向きは変わらない。この task の規則で抽象の追加は成果に数えない |
| C | `account_write` の関数が `&ApiState` を受ける | 不採用。`state` は SCC の中なので `account_write` が SCC に入る |
| D | 関数を移し、`ApiState` の欄の値を引数で受ける（採用） | 辺が SCC の外へ一方向に出る。新しい trait・Port・状態は無い |
| E | `Effect` を新しい module（`effect.rs`）に置く | 不採用。`Effect` は `ApiProblem` の 2 段の `Result` で、`problem.rs` に置けば単位を足さない |

## 5. 新しい状態・分岐・fallback・port

無し。新しい module は `http_io.rs` と `account_write.rs` の 2 つで、どちらも関数・型の移動先である。DB・config・API・CLI の契約は変えない。

## 6. 決定

### D1: HTTP の道具を `http_io.rs` へ移す

- `ApiResult`・`Params`（`FromRequestParts` の実装を含む）・`json_response`（と定数 `JSON_CONTENT_TYPE`）・`read_body`・`read_json`・
  `no_query` を `handlers.rs` から `crates/task-api/src/http_io.rs` へ移す。本体は変えない（400・413 の応答と文言も同じ）。
- 呼び出し元の import をすべて `crate::http_io::…` に直す。`handlers/` の子 module の `use super::{ApiResult, …}` も直す。
  `handlers.rs` に re-export を残さない。
- `cluster_ids` は `ApiState::cluster_ids(&self) -> Vec<String>` にし、`handlers::validated_workspace` と `cos/operations.rs`（今の 845 行）
  はこれを呼ぶ。`handlers::cluster_ids` は消す。
- `handlers.rs` に残すもの: route の表・`now_rfc3339`・`parse_project_id`・`validated_workspace`・`validated_workspace_in`。

### D2: account の plan と作用を `account_write.rs` へ移す

- plan: `pool_adapter`・`is_implicit_source`・`resolve_pool_account`。作用: `create_account_op`・`clear_stale_limit`・
  `set_account_limit_effect`・`delete_account_effect`・`check_account_effect`。これらが使う private の helper（`pool_source_id`・
  `effective_max_runs`・`account_items`・`implicit_view`・`parse_limit_patch`・`account_admin_error`・定数 `IMPLICIT_ADAPTER`・
  `MAX_RUNS_RANGE`）も一緒に移す。HTTP の route の関数（`accounts`・`create_account`・`patch_account_limit`・`patch_account_limit_inner`・`delete_account`・
  `check_account`・login 系の 3 つ）は `handlers/accounts.rs` に残し、`account_write` を呼ぶ。
- `account_write` の関数は `ApiState` でなく、使う欄の値（`accounts_roots`・`admin_tx`・`max_runs_per_account`・`store`・
  daemon の snapshot）を引数で受ける。引数が 7 を超える関数（`clippy::too_many_arguments`）は、これらの値だけを持つ struct
  （field だけで method・trait を持たない）を `account_write.rs` に 1 つ置いて受けてよい。trait・Port・Manager は足さない。
- store に書く作用（`clear_stale_limit` の celeris が無いときの経路）は `tokio::task::spawn_blocking` で store を使う
  （`ApiState::blocking` と同じ処理。失敗は今と同じく warn の log だけ）。
- 稼働の確認 `require_active`（`middleware.rs`。authz の高 risk path なので変えない）は、adapter が作用の直前に呼ぶ。順序は今と同じにする:
  HTTP は `no_query` → `require_admin` → 本文 → plan → `require_active` → 作用、CoS は plan の拒否を `audit.reject` で記録し、
  `require_active` の拒否は `external` の closure の中で内側の `Err`（`ApiProblem::standby()`）として返す（今の記録と同じ）。
- HTTP と CoS の adapter は `account_write` を一方向に呼ぶ。`account_write` から adapter を呼ばない。
- `cos/ops/admin.rs` の `account.limit_set` の adapter の解決（今の 469〜500 行。`pool_adapter`・`accounts_roots`・暗黙の source の判定を
  inline で書き直している）は、拒否の status・code・監査の target id が変わらないなら `account_write` の plan の関数 1 つにまとめてよい。
  まとめたら、行の減りは「削除の分」として after の測定で移動の分と分けて書く。

### D3: `Effect`・`effect_result` を `problem.rs` へ移す

- `pub(crate) type Effect<T>` と `effect_result` を `cos/operations.rs` から `problem.rs` へ移し、呼び出し元（`handlers/accounts.rs`・
  `handlers/reload.rs`・`handlers/system.rs`・`account_write.rs`・`cos/`）の import を直す。`cos/operations.rs` に re-export を残さない。
- `effect_value`・`OperationAudit`・`Applied`・`file_op`・`external`・`request_hash` は `cos/operations.rs` に残し、本体を変えない。

### 変えないもの

response の status・本文・拒否理由（code と文言）・CoS の監査（operation の記録・監査の行・card・同じ transaction）・認可
（`require_admin`・`require_active`・CoS の principal の判定）・DB・config・API・CLI の契約。policy の file は変えない。

## 7. 不変条件と試験

| 不変条件 | 確かめる試験・検査 |
|---|---|
| `cos-audit-same-tx`（CoS の変更と監査が同じ transaction） | `crates/task-api/tests/cos_operations.rs` の `cos_chat_ops_api_task_create_writes_domain_audit_and_card_together`、`cos_delivery_suite/cos_ops_admin_config.rs` の `cos_ops_admin_config_daemon_and_external_effects_run_once`（`account.check`・`account.delete` が 1 回だけ走り監査される）、`cos_delivery_suite/org_write_contract.rs` の `cos_org_writes_commit_row_browser_event_operation_and_audit_together`、`cos_delivery_suite/tx_boundary_contract.rs` の全件 |
| `api-principal-from-token`（主体は token から決まる） | `crates/task-api/tests/auth_and_guards.rs` の `bearer_token_is_required_when_configured`・`no_token_means_no_authentication`、`llm_accounts_admin.rs` の `management_routes_all_require_a_token`、`org_write_contract.rs` の `org_writes_need_an_admin_principal_and_cos_writes_need_the_cos_run`、`cos_operations.rs` の `cos_chat_ops_api_body_shape_and_identity_claims_are_rejected` |
| account の response と拒否理由が変わらない | `llm_accounts_admin.rs` の全件（`unknown_source_is_not_found`・`delete_account_without_admin_tx_is_accounts_unavailable`・`delete_account_in_use_is_conflict`・`implicit_accounts_are_listed_and_patchable_by_source_and_refused_otherwise` ほか）と、WorkUnit `contract` が今のコードで固定する account の HTTP・CoS の契約試験 |
| HTTP の道具の応答（400・413）が変わらない | `crates/task-api/src/handlers/tests.rs` と既存の route の試験（`bash scripts/dev/test-affected.sh` が task-api と逆依存を走らせる） |
| 新しい module が SCC に入らず、task-api の SCC の組が増えない | `python3 -I agent-docs/progress/2026-11-19-arch-v4-h-r2/measure.snippet compare <before.json> <after.json> http_io account_write` が 0 |
| 旧い経路が残らない（`remove-superseded-mechanism`） | 表の下の `git grep` 3 本がどれも 0 行 |
| `no-unwrap-outside-tests` | `cargo clippy --workspace -- -D warnings` と review |

旧い経路の検査（repo の root で。どれも出力 0 行）:

```sh
git grep -nE 'handlers::(ApiResult|Params|json_response|read_body|read_json|no_query|cluster_ids)\b|handlers::\{[^}]*\b(ApiResult|Params|json_response|read_body|read_json|no_query)\b' crates/task-api/src
git grep -n 'handlers::accounts' crates/task-api/src/cos
git grep -nE 'cos::operations::(\{[^}]*)?\b(Effect|effect_result)\b' crates/task-api/src
```

`cos/operations.rs` は authz の高 risk path（`^crates/task-api/src/cos/operations\.rs$`）である。この変更が `cos/operations.rs` で変えるのは
import・`Effect`/`effect_result` の定義の削除・`cluster_ids` の呼び方だけで、`OperationAudit` の transaction と principal の判定は変えない。

## 8. 期待する効果（見積り）

measure.snippet の before の辺から、D1 で HTTP の道具だけの行、D2 で `admin.rs` の `handlers::accounts::` の 9 行、D3 で
`Effect`・`effect_result` だけの 5 行を除いて SCC を取り直した見積り（新しく足す辺は SCC の外へ向くので数えていない）:

| metric（checker=head） | before | 見積り |
|---|---|---|
| ta_scc_units | 49 | 49 |
| ta_scc_pairs | 261 | 237 |
| ta_scc_lines | 622 | 551 |
| handlers_to_cos_lines | 19 | 14 |
| cos_to_handlers_lines | 50（`crate::` 46・`super::` 4） | 29（`crate::` 29・`super::` 0） |

単位の数は減らない見込み（HTTP の道具を外しても、各単位は route の登録で `handlers` から呼ばれ、別の辺で `handlers` へ戻る）。
after の値は WorkUnit `after` が測り、行の減りを移動の分（D1〜D3 の移動）と削除の分（D2 の limit_set の解決のまとめなど）に分けて書く。
