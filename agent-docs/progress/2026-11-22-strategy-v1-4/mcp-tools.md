---
title: 戦略層 v1 子 4 mcp-tools — MCP の discussion snapshot・evidence search・propose の道具
tasks: [01M7XQ9QK1KC34Q2A6PJ960TJB]
status: blocked
updated: 2026-11-22
---
# mcp-tools（WorkUnit。子 4 の段 wire）

設計は [ADR 2026-11-22-external-llm-discussion-exchange](../../adr/2026-11-22-external-llm-discussion-exchange.md) の D1・D2.4・D3・D7
（実装の付記は D1.7）。

## 結論

- 3 道具 `discussion_snapshot`・`discussion_evidence_search`（scope `discussion:read`）と `discussion_propose`（scope `discussion:propose`）を
  `crates/celeris-mcp/src/tools/discussion.rs` に置き、既存の registry（`tools::all()`）に並べた。認証・client ごとの scope・`mcp_calls`・
  流量制限は既存の `crate::rpc` のまま。HTTP・`celerisctl mcp stdio`・`celerisctl mcp call` は同じ `rpc` の dispatch を通るので同じ契約になる。
- 範囲と規則は書き直していない。snapshot と propose は exchange-core の `SqliteStore::chat_exchange_snapshot`・`chat_exchange_propose` を呼ぶだけで、
  主体は認証した client から `mcp:<client_id>` として作る（引数からは取らない）。
- `McpScope` に `DiscussionRead`（`discussion:read`）・`DiscussionPropose`（`envelope::MCP_PROPOSE_SCOPE`、綴りは 1 か所）を足した。
- **止まっている点（plan_issue）**: `McpScope` の JSON Schema の enum に 2 つを足すと、公開 API の schema（`ApiV1Schema` の `GET /mcp/clients` の
  `scopes`）が変わる。生成物 `docs/api/v1/api-v1.schema.json`・`web/api/generated/{schema.json,types.ts}` はこの unit の書き込み範囲（ADR D7.3）に
  無く、再生成しないと task-api の `committed_schema_matches_generated` が落ちる（下の証拠）。計画で範囲を足すまで done にしない。

## 実装前 brief

1. 根本原因: 外部 LLM の入口が全権に近い道具（`db_query`・`api_get`・`decision_answer`）と `console_*` だけで、人の共有の範囲・出典・consent・
   idempotency を持つ MCP の契約が無い。
2. 責務 owner: 道具は `crates/celeris-mcp/src/tools/`、scope は `crates/task-core/src/mcp.rs`。範囲と規則は `task_core::discussion_exchange` と
   `task_core::strategy::envelope`（exchange-core・envelope-fix の成果）。
3. 既存の再利用先: `ToolDef` の registry、`rpc` の scope・監査・流量制限、`chat_exchange_snapshot`・`chat_exchange_propose`、`ChatCitation`。
4. 選択肢: (a) 道具は引数と error の写しだけにし、範囲と規則は task-core を呼ぶ（採用）。(b) MCP 側で chat の表を直接読む（不採用。範囲の実装が
   2 つになる）。検索は (i) snapshot の item（マスク後）を照合（採用。ADR D1.7）、(ii) FTS5 を引いて共有で絞る（不採用。範囲の実装が 2 つになり、
   マスク前の本文で当たると秘密の有無が分かる）。
5. 新しい状態・分岐・fallback・port: MCP の道具 3 つと scope 2 つ（ADR D1.1 の port）。状態・fallback・表は無い。
6. 撤去対象: 無し（旧い経路を置き換えていない。`tasks:interact` の承認系の撤去は scope-guard）。
7. 不変条件: 外部 client の propose は `mcp:<client_id>` の `discuss` の draft で、人の発言・承認にならない。scope の無い client は道具を見ることも
   呼ぶこともできない。共有の外・別案件は出ない。秘密は出ない。試験は下の表。

## 変更

- `strategy-v1 4 mcp-tools: MCP の discussion_snapshot・discussion_evidence_search・discussion_propose と discussion:* の scope`
  - `crates/celeris-mcp/src/tools/discussion.rs`（新規）: 3 道具。検索は snapshot の全頁（200 件 × 最大 50 頁）のマスク後の本文を、空白で区切った
    語がすべて含まれるかで照合し、snippet は 400 文字まで。propose の `kind: chat` の参照は呼んだ client の snapshot の item の `citation` と一致
    しなければ `-32002 citation_invalid`。exchange の拒否は `exchange_invalid`・`exchange_too_long`・`thread_*` を `-32602`、それ以外
    （`exchange_secret`・`R7:idempotency_conflict`・`exchange_rules` など）を `-32002` に写す。
  - `crates/celeris-mcp/src/tools/mod.rs`: `pub mod discussion;` と registry の 3 行。`audit_request` は変えていない（3 道具の本文は監査に写さない）。
  - `crates/task-core/src/mcp.rs`: scope 2 つ（`as_str`・`parse`・JSON Schema の enum・往復の試験）。
  - `crates/celeris-mcp/tests/mcp_discussion.rs`（新規）: `mcp_discussion_` の 8 試験（本物の HTTP の `initialize` → `tools/list` → `tools/call`）。
  - `docs/guides/mcp.md`: §4 の scope の表と §5 の道具の説明。
  - ADR の D1.7（実装の付記）。

## 負の試験と、変更前に落ちること（受け入れ条件 0・1）

base には 3 道具も `discussion:*` の scope も無い（`McpScope::DiscussionRead` が無いので試験はコンパイルで落ちる）。挙動として落ちることは次の
probe で示した: registry の 3 行だけを外して `cargo nextest run -p celeris-mcp -E 'test(/mcp_discussion_/)' --no-fail-fast` → exit 100、
`mcp_discussion_tools_listed_by_scope`・`mcp_discussion_propose_duplicate_single`・`mcp_discussion_propose_author_is_mcp_not_human`・
`mcp_discussion_output_has_no_secret` などが FAIL（道具が無く `-32601`）。3 行を戻すと 8 passed。

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `mcp_discussion_tools_listed_by_scope` | `discussion:read` は snapshot・検索、`discussion:propose` は propose だけが一覧に出る。既定の scope と `tasks:interact`・`db:read`・`api:read` には出ない | 道具と scope が base に無い |
| `mcp_discussion_scope_missing_rejected` | scope の無い client の 3 道具の呼び出しは `-32601`、読む client は propose できない。拒否は何も書かない | 同上 |
| `mcp_discussion_other_project_thread_rejected` | 別案件の thread・食い違う組・統括の thread は `-32602`、別案件の message を引く propose は `citation_invalid`。何も書かない | 同上 |
| `mcp_discussion_propose_author_is_mcp_not_human` | propose の author・actor・consent は `mcp:muse`、mode は `discuss`、item は `proposed`。`approved`・`rejected`・`withdrawn` は `-32002`、`actor`・`consent`・`approval` の引数は `-32602`。task・decision は増えない。別の client には見えない | 同上 |
| `mcp_discussion_propose_duplicate_single` | 同じ key と本文は 1 件（`duplicate: true`）、別の本文は `R7:idempotency_conflict`、key は client ごと | 同上 |
| `mcp_discussion_injection_quote_no_escalation` | 注入文を含む共有の message を引用した propose は `discuss`・`proposed` のまま、`approved` は拒否、版・sha256 の違う引用は `citation_invalid`。`Project.request`・task・decision・Roadmap の版・task の events は変わらない | 同上 |
| `mcp_discussion_output_has_no_secret` | 秘密を含む propose は `exchange_secret` で本文を返さない。共有の後に秘密の形になった本文（試験の DB を書き換えて再現）は snapshot で `[masked]`、秘密の前置きで探しても当たらない。`mcp_calls.request` に 3 道具の本文が無い | 同上 |
| `mcp_discussion_evidence_search_excludes_unshared_and_confidential` | 検索は共有の外・別の client の案・別案件を返さない。hit は citation・source・actor・confidence・decision_state・content_hash を持つ。語はすべて含むこと、`query` 513 文字は `-32602` | 同上 |

受け入れ条件 1 の「承認に数えられない」は、`mcp_discussion_propose_author_is_mcp_not_human`（記録の主体が `mcp:muse`、`approved` を拒否、decision が
増えない）と `mcp_discussion_injection_quote_no_escalation` で示した。`by=mcp:` の決定への答えが R10 を満たさないことは envelope-fix の
`external_exchange_envelope_mcp_answer_not_approval` が示す。

## 証拠（開始時の HEAD「integrate wu/exchange-core (phase core)」の上で実行）

- `cargo nextest run -p celeris-mcp -E 'test(/mcp_discussion_/)'` → 8 tests run: 8 passed
- `cargo nextest run -p celeris-mcp --no-fail-fast` → 85 tests run: 85 passed
- `cargo nextest run -p task-core --lib -E 'test(/mcp::/) | test(/external_exchange_/)'` → 32 tests run: 32 passed
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p celeris-mcp --tests -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo nextest run -p task-api --lib -E 'test(committed_schema_matches_generated)'` → **FAIL**（`McpScope` の enum が生成物と違う）。範囲外の生成物の
  再生成が要る（plan_issue）。
- `bash scripts/dev/test-affected.sh` は未実行（上の 1 件が落ちることが分かっているため。範囲を足した後に実行する）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → `agent-docs/adr/2026-11-22-external-llm-discussion-exchange.md`・`crates/celeris-mcp/src/tools/{discussion.rs,mod.rs}`・
  `crates/celeris-mcp/tests/mcp_discussion.rs`・`crates/task-core/src/mcp.rs`・`docs/guides/mcp.md`（と、この進捗）。すべて ADR D7.3 の範囲の中。

## LoC（開始時の HEAD からの差分）

- production: +453 / −1（`tools/discussion.rs` +435、`tools/mod.rs` +4、`task-core/src/mcp.rs` +14 / −1。うち試験の 2 行）。純増の理由: 3 道具と 2 scope は
  base に無い port（ADR D1.1）。範囲と規則は task-core を呼び、書き直していない。
- test: `tests/mcp_discussion.rs` +831。
- docs: `docs/guides/mcp.md` +26、ADR +19。

## 親の目的への寄与

- 確認済み（試験）: MCP 対応の外部 LLM（ChatGPT・Muse など）は、人が共有した範囲だけを出典・hash 付きで読み、未承認の案を `mcp:<client_id>` の
  draft として記録できる。外部の発言は人の発言・承認にならず、注入文で権限・状態が上がらない（全体の不変条件「外部 LLM の発言を human actor や
  承認と取り違えない」「機微な情報は既定で外に出ない」、改善 B の「議論を出典付きで共有する」の外部の入口）。
- 推定: RDC の `celeris-chat` → `celerisctl mcp call` の経路も同じ `rpc` を通るので同じ契約になる。実際の経路の試験は rdc-path。
- 未確認: 本物の ChatGPT・Muse・RDC home-dev での動作。本番の client への scope の付与（人が昇格の後に行う）。

## 未解決

- 公開 API の schema の生成物の再生成（上の plan_issue）。
- 要約・戦略の file（`ExchangeDocument`）は MCP から渡していない（`McpState` は CoS の workspace の場所を持たない。ADR D1.7）。今の MCP の
  snapshot・検索の item は `message` と `proposal` だけ。要約・戦略を外部 LLM に渡すなら、CoS の workspace の file を読む処理を 1 か所
  （task-core か task-dispatch）に置き、MCP と REST の書き出しの両方から呼ぶ形にする。
- `citation_invalid` の検査は MCP 側にある。REST の取り込みでも同じ検査が要るなら task-core に移す（ADR D1.7）。

## 提案

- 計画: mcp-tools の書き込み範囲に `docs/api/v1/api-v1.schema.json`・`web/api/generated/` を足す。再生成は
  `UPDATE_SCHEMA=1 cargo nextest run -p task-api --lib -E 'test(committed_schema_matches_generated)'` と `pnpm -C web gen:types`。rest-import も同じ
  生成物を変えるので、統合の merge で衝突しうる（変わる行は `McpScope` の enum と rest-import の新しい型で離れている）。
