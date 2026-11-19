---
title: アーキ v4 H R2 http-tools — HTTP の道具を handlers.rs から SCC の外の http_io.rs へ移す
tasks: [01M7T5KM3FW3430R5DT4QG5QKV]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# http-tools（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- [ADR 2026-11-19 cos-handlers-remaining-edges](../../adr/2026-11-19-cos-handlers-remaining-edges.md) の D1 を実装した。
  `ApiResult`・`Params`（`FromRequestParts` の実装を含む）・`json_response`（定数 `JSON_CONTENT_TYPE` を含む）・`read_body`・
  `read_json`・`no_query` を `crates/task-api/src/handlers.rs` から新しい `crates/task-api/src/http_io.rs` へ移した。本体は変えていない。
- `handlers::cluster_ids` を消し、`ApiState::cluster_ids(&self)`（`crates/task-api/src/state.rs`）にした。
- task-api の全呼び出し元の import を `crate::http_io` に書き換えた（script で機械的に。`handlers.rs` に re-export は無い）。
- `http_io` は `crate::problem`・`crate::query`・crate root の `MAX_BODY_BYTES` と外部 crate だけを使い、どの SCC にも入らない。
  task-api の SCC の組は 261 → 232（消えた 29・増えた 0）、cos → handlers の行は 50 → 38。
- response・拒否理由・認可は変えていない。

## 1. 変更

commit（件名）:

- `arch-v4 H R2 http-tools: HTTP の道具を handlers.rs から http_io.rs へ移し、cluster_ids を ApiState の method にする`
- `arch-v4 H R2 http-tools: 進捗 http-tools.md を書く`

### 1.1 変えた file の数（基準は `$CELERIS_WU_BASE`、`git diff --numstat`）

| 区分 | file 数 | 中身 |
|---|---|---|
| 定義の移動元・移動先 | 4 | `handlers.rs`（+2 −97）、`http_io.rs`（新規 +90）、`state.rs`（+10。`cluster_ids`）、`lib.rs`（+2。`mod http_io` と doc） |
| cos の呼び出し元 | 6 | `cos/{inbox,knowledge_use,mod,operations,override_op,review}.rs` |
| handlers の子 module | 16 | `handlers/{accounts,authority,clusters,config_fields,org,project_controller,projects,reload,roadmap,secrets,system,task_actions,task_io,tasks,terminals,tests}.rs` |
| SCC の内外の他の module | 39 | `approvals.rs`・`browser.rs`・`chat/*`・`releases.rs`・`script_*.rs` ほか |
| 文書 | 1 | `docs/architecture-map.md`（`http_io` の行を router の行の次に足した） |
| 計 | 66 | 211 insertions・186 deletions（この md を除く） |

`handlers/{authority,project_controller,roadmap,tests}.rs` は `use super::*` で `handlers.rs` の import を受けていたので、
`crate::http_io` の import と、`handlers.rs` から消えた `axum::body::Body`・`store_problem`・`StatusCode` の import を明示で足した。

### 1.2 移した定義と消した定義の行数（base の `handlers.rs` の行）

| 定義 | base の行 | 行数 | 行き先 |
|---|---|---|---|
| `type ApiResult` | 70 | 1 | `http_io.rs`（移動） |
| `const JSON_CONTENT_TYPE` | 72 | 1 | `http_io.rs`（移動） |
| `fn json_response` | 285–300 | 16 | `http_io.rs`（移動） |
| `struct Params`（doc を含む） | 302–303 | 2 | `http_io.rs`（移動） |
| `impl FromRequestParts for Params` | 305–318 | 14 | `http_io.rs`（移動） |
| `fn read_body`（doc を含む） | 320–334 | 15 | `http_io.rs`（移動） |
| `fn read_json`（doc を含む） | 336–349 | 14 | `http_io.rs`（移動） |
| `fn no_query` | 351–353 | 3 | `http_io.rs`（移動） |
| 移動の計 | | 66 | |
| `fn cluster_ids`（doc を含む） | 383–392 | 10 | 消した。`ApiState::cluster_ids`（doc を含む 9 行）を `state.rs` に足した |

- `handlers.rs` は 412 → 317 行（−97 +2）。−97 の内訳: 移動 66、`cluster_ids` 10、移動した定義だけが使っていた import 11、空行 9、
  `validated_workspace` の呼び方を変えた 1。
- `http_io.rs` の 90 行 = 移動した 66 行 + module の doc 3 行 + import 12 行 + 空行 9 行。
- 呼び出し元の import の書き換え: 211 − 90（http_io.rs）− 10（state.rs）− 2（lib.rs）− 1（architecture-map）− 2（handlers.rs）= 106 行を足し、
  186 − 97（handlers.rs）= 89 行を消した（対象は `handlers/tests.rs` を含む 61 file。`use` の分割で 1 行が 2 行になる file と、
  `use super::*` の 4 file に足した明示の import がある）。
- 移動した 66 行には `impl FromRequestParts` の中の空行 1 行を含む。
- 行の純減はこの WorkUnit の目的ではない（task の成果は SCC の組と cos → handlers の行で数える）。task-api の `src` は
  `handlers/tests.rs` を除いて 23 行増えた（+208 −185）。

### 1.3 handlers.rs に残したもの

route の表（`router`）・`rfc3339`・`now_rfc3339`・`fallback`・`method_not_allowed`・`parse_project_id`・`validated_workspace`・
`validated_workspace_in`・子 module の宣言。

### 1.4 高 risk の path（review profile の class）に当たる変更

| path | class | 変えたもの |
|---|---|---|
| `crates/task-api/src/cos/operations.rs` | authz | import 1 行（`crate::handlers` → `crate::http_io`）、`crate::handlers::read_json` → `crate::http_io::read_json` の 1 行、`crate::handlers::cluster_ids(state)` → `state.cluster_ids()` の 1 行だけ。`OperationAudit` の transaction と principal の判定は変えていない |
| `crates/task-api/src/handlers/secrets.rs` | credential | import だけ（`use super::{…}` を `use super::now_rfc3339;` と `use crate::http_io::{…}` に分けた） |
| `crates/task-api/src/knowledge.rs` | transaction | import 1 行だけ |
| `crates/task-api/src/releases.rs` | selfdeploy | import 1 行と、`crate::handlers::{no_query,Params}` の path の 6 行だけ |

## 2. 測定（measure.snippet。head の検査器、`super::` を辺にする）

before は `$CELERIS_WU_BASE`（task の before と同じ値）、after はこの WorkUnit の HEAD。

```sh
python3 -I agent-docs/progress/2026-11-19-arch-v4-h-r2/measure.snippet measure "$CELERIS_WU_BASE" "$TMPDIR/m/before.json"
python3 -I agent-docs/progress/2026-11-19-arch-v4-h-r2/measure.snippet measure HEAD "$TMPDIR/m/after.json"
python3 -I agent-docs/progress/2026-11-19-arch-v4-h-r2/measure.snippet compare "$TMPDIR/m/before.json" "$TMPDIR/m/after.json" http_io
```

| metric | before | after（この WU） |
|---|---|---|
| ta_scc_units | 49 | 49 |
| ta_scc_pairs | 261 | 232 |
| ta_scc_lines | 622 | 560 |
| handlers_to_cos_lines（head） | 19 | 19 |
| cos_to_handlers_lines（head） | 50（crate 46・super 4） | 38（crate 34・super 4） |
| cos_to_handlers_super_lines（grep） | 4 | 4 |

- compare は exit 0、出力の末尾は `OK`。task-api の SCC の組は「消えた 29・増えた 0」。`http_io` はどの SCC にも無い。
- 消えた 29 組はすべて `<module> -> handlers`（`approvals`・`browser`・`chat`・`knowledge`・`releases`・`script_*`・`tree` ほか）。
- cos → handlers に残った 38 行は `cos/ops/admin.rs` 13・`cos/ops/projects.rs` 17・`cos/ops/tasks.rs` 8。名前の内訳は
  `accounts` 9（WorkUnit `accounts` の対象）・`task_actions` 7・`parse_project_id` 6・`authority` 5・`projects` 3・`clusters` 2・
  `project_controller` 2・`reload`・`system`・`roadmap`・`tasks` 各 1。
- metrics.tsv の after は WorkUnit `after` が書く（この WU は書いていない）。

旧い経路の検査（ADR §7 の 1 本目と、`cos/operations.rs`）。どちらも出力 0 行（`git grep` の exit 1）:

```sh
git grep -nE 'handlers::(ApiResult|Params|json_response|read_body|read_json|no_query|cluster_ids)\b|handlers::\{[^}]*\b(ApiResult|Params|json_response|read_body|read_json|no_query)\b' crates/task-api/src
git grep -n 'crate::handlers' crates/task-api/src/cos/operations.rs
```

## 3. 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh` → exit 0。nextest 6132 tests run: 6132 passed、21 skipped（celeris・celeris-campaign・
  celerisctl・llm-proxy・task-api・task-dispatch・task-worker）。web の tsc -b が通り、vitest は 101 file・705 tests passed。
  repo_health_gate: all checks passed。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `python3 -I scripts/dev/check-architecture-map.py` → exit 0（`OK: 1099 件のパスを確認した`）
- measure.snippet compare（§2）→ exit 0

## 未解決

- 無し。`handlers/accounts.rs`・`cos/ops/admin.rs` の account の処理（D2）と `Effect`・`effect_result` の移動（D3）は WorkUnit
  `accounts` が行う。

## 提案

- 無し。
