---
title: 戦略層 v1 子 4 rest-import — MCP 非対応の経路（人の共有・書き出し・取り込みの REST）
tasks: [01M7XQ9QK1KC34Q2A6PJ960TJB]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# rest-import（WorkUnit。子 4 の段 wire）

完了日: 2026-11-22。設計は [ADR 2026-11-22-external-llm-discussion-exchange](../../adr/2026-11-22-external-llm-discussion-exchange.md) の
D2.1・D4.1〜D4.3。基準は `$CELERIS_WU_BASE`（この WU の base）。

## 結論

- `crates/task-api/src/chat/exchange.rs`（新規）に D4.1 の 4 route を置いた。規則は書き直さず、exchange-core の
  `SqliteStore::{chat_exchange_share, chat_exchange_share_revoke, chat_exchange_snapshot, chat_exchange_import}` を呼ぶだけ（規約 2）。
  - `POST /chat/threads/{t}/exchange/shares`: 人が発言を共有する（201 `{share_id, revoked:false}`）。秘密の形を含む発言は 422 `exchange_secret`。
  - `POST /chat/threads/{t}/exchange/shares/{share_id}/revoke`: 共有を取り消す（200 `{share_id, revoked:true}`、取り消し済みは 409）。
  - `GET /chat/threads/{t}/exchange/export?format=json|markdown&share_id=&after_seq=&limit=`: 共有の範囲を `celeris.discussion-snapshot/1` の
    JSON（`application/json`）か Markdown（`text/markdown`）で返す。読むだけで記録を作らない。
  - `POST /chat/threads/{t}/exchange/imports`: 人が外部 LLM との議論から選んだ要約・引用 1 件を `discuss` の発言として取り込む
    （新規 201、同じ key と本文は 200 `duplicate:true`、違う本文は 409 `R7:idempotency_conflict`）。
- 主体: 4 route とも handler の最初で `Principal::Human` だけを通す（`person()`、他は 403 `exchange_forbidden`）。`by`・consent・author は
  `caller.actor()` で、要求の欄からは取らない。MCP の書き込みは guard（403）、CoS run の直接の書き込みは middleware（422
  `cos_audit_context_required`）が handler より先に拒む。token を設定した構成で token の無い要求は middleware の 401。token の無い loopback の
  構成（`Principal::Unauthenticated`）は handler の 403。
- 登録: 書き込みの 3 route を `crates/task-api/src/cos/ops/surface.rs` の `EXCLUDED` と ADR 2026-10-09 の D2・D3 の表に理由
  `human_exchange` で足した（CoS の operation から呼べない）。`docs/api/v1/api.md` の endpoint の表（242〜245）と chat の詳細の表に 4 行ずつ足した。
- wire の型: `ChatExchangeShareRequest`・`ChatExchangeShareResponse`・`ChatExchangeImportRequest`・`ChatExchangeImportResponse` を
  `ApiV1Schema` に登録し、`docs/api/v1/api-v1.schema.json`（`UPDATE_SCHEMA=1`）と `web/api/generated`（`node web/scripts/gen-types.mjs`）を
  再生成した。task-core の `ExchangeShareRequest`・`ImportRequest` は `JsonSchema` を持たず、task-core はこの WU の書き込み範囲の外なので、
  task-api の wire の型から `From` の網羅の struct literal で作る（task-core 側に欄が増えれば compile が落ちる。形の二重の定義は wire と
  domain の境界の 1 か所だけ）。
- ChatGPT の会話全文・Memory を読みに行く経路、browser の login・Cookie、外部の URL を取りに行く処理は無い。取り込みは人が貼った 1 件だけ。
- 新しい表・列・migration・Manager・trait・状態機械は無い。

## 試験（`crates/task-api/tests/chat_exchange.rs`、12 件、接頭辞 `chat_exchange_`）

| 試験 | 守る不変条件 |
|---|---|
| `chat_exchange_share_export_import_roundtrip` | 共有した発言だけが JSON・Markdown で出る（consent は共有した人）。共有していない発言は出ない。取り込みは author が呼んだ人の `discuss` の発言で `metadata_json.exchange` を持つ。Markdown の取り込みも通る。取り消しの後の書き出しは空、二度目の取り消しは 409 |
| `chat_exchange_import_non_human_forbidden` | operator（handler の 403）・MCP（guard の 403）・CoS run（422 と `/cos/operations` の拒否）・token 無し（401、loopback の無 token 構成は 403）は取り込みも書き出しもできず、何も書かれない |
| `chat_exchange_share_non_human_forbidden` | 同じ主体は共有・取り消しができず、何も書かれない |
| `chat_exchange_share_secret_rejected` | 秘密の形を含む発言の共有は 422 `exchange_secret` |
| `chat_exchange_import_secret_rejected` | token・webhook URL の形を含む取り込みは 422 `exchange_secret`、応答に秘密の値が出ない |
| `chat_exchange_import_duplicate` | 同じ key と本文は 200 `duplicate:true` で同じ message、2 件目を作らない |
| `chat_exchange_import_conflict` | 同じ key と違う本文は 409 `R7:idempotency_conflict`、最初の本文は変わらない |
| `chat_exchange_import_other_project` | 別案件の envelope は 422（`R6:project_mismatch`） |
| `chat_exchange_consent_granted_by_is_caller` | 文書が `actor`・`consent`・`approval` を書けば（呼んだ人の名前でも）422 `exchange_self_reported`。受けた consent は呼んだ人 |
| `chat_exchange_import_injection_not_approval` | 注入文の `approved` は R10、`rejected` は R11 で 422。受けた注入文は `proposed` のまま `discuss` で、それを入力にした CoS run の `task.create` は 403 `discussion_mode_forbidden`、task と答えた決定は 0 件 |
| `chat_exchange_export_confidential_default_excluded` | 共有が無ければ書き出しは空。共有した CoS の返事に戦略の文書があっても、この route は文書を出さない（下の未解決）。共有の後に編集した発言は `edited_since_share` で出ない |
| `chat_exchange_export_query_and_thread_kind` | `format` は json・markdown だけ、未知の query は 400、統括の thread は 422 `thread_kind_not_project` |

既存の試験がこれを検出できない理由: 4 route は base に無い（404・405）。変更前に落ちる probe: `chat/mod.rs` の `.merge(exchange::routes())` を
外して同じ試験を流すと 12 件すべて失敗した（`12 tests run: 0 passed, 12 failed`）。戻した後は 12 件とも通る。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-api -E 'test(/chat_exchange_/)'` → exit 0、`12 tests run: 12 passed`。
- `cargo nextest run -p task-api -E 'test(/cos_ops_registry|cos_ops_exclusions/)'` → 通過（EXCLUDED・ADR の表・api.md の表が一致）。
- `cargo nextest run -p task-api -E 'test(committed_schema_matches_generated)'` → exit 0（再生成の後）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → 下の「test-affected」に書く。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `crates/task-api/`・`docs/api/v1/`・`web/api/generated/`・ADR 2026-10-09・
  `agent-docs/progress/` だけ（`cos/strategy_gate*`・`strategy/mod.rs`・`control-ts/`・`ChatPostMessageRequest`・`ChatRelayRequest` は変えていない）。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`、追加のみ・削除 0）

- production Rust: +372（`chat/exchange.rs` 348・`chat/mod.rs` 2・`cos/ops/surface.rs` 15・`schema.rs` 7）。
- test Rust: +703（`tests/chat_exchange.rs`）。
- 生成物: `api-v1.schema.json` +132・`web/api/generated/schema.json` +132・`types.ts` +30。文書: `api.md` +8・ADR 2026-10-09 +6。
- 純増の理由: D4.1 の 4 route（MCP 非対応の経路）は base に無い機能。handler は store の関数を呼ぶだけで、規則は task-core の 1 か所にある。

## 親の目的への寄与

- 確認済み（試験）: MCP の無い client（ChatGPT のどのプランでも）のために、人が選んだ範囲だけを書き出し、人が選んだ 1 件だけを取り込む
  REST がある。外部の文は human の承認にならず（R10・R11）、取り込んだ文の run は `discuss` で操作できない。秘密・別案件・二重取り込み・
  自己申告の主体は拒否する。observability（人が議論の出典を追える）には、取り込みの envelope が `metadata_json.exchange` に残ることで寄与する。
- 推定: 子 7 の画面が `metadata_json.exchange` を外部の文として示せば、人が ChatGPT 由来の文を区別できる（この WU では wire の `ChatMessage` を変えていない）。
- 未確認: 本物の ChatGPT・Muse での貼り付けと書き出しの往復、本番の DB での動作（昇格の後に人が行う）。

## 未解決

- 書き出しは要約・戦略の文書（`ExchangeDocument`）を渡していない（`chat_exchange_snapshot(…, &[], …)`）。CoS の返事の添付の blob を読み、
  要約（`celeris.discussion-summary/1`）と戦略の文書（acl・references・confidence・決定 `strategy-approve-v<n>` の承認）から
  `ExchangeDocument` を作る関数は、MCP の `discussion_snapshot`（mcp-tools）も同じものが要る。task-api と celeris-mcp に別々に書くと規約 2 に反し、
  task-core はこの WU と mcp-tools の書き込み範囲の外なので置かなかった。今は機密の文書は既定どおり外に出ない（出ない側に倒れている）。
- ADR D4.1 の最後の項目「`celerisctl` に同じ 4 つを呼ぶ subcommand を足す」はこの WU では足していない（celerisctl に chat の subcommand が
  1 つも無く、WU の受け入れ条件にも無い）。今は Web の人の token か `curl` で REST を呼ぶ。
- CoS のコメントの advisory 2（`StrategyRef` と `ArtifactPointer` の重複）・3（D5.3 の fail-closed）はこの WU の範囲の外。

## 提案

- integrate-wire の後に、`task_core::discussion_exchange` に「thread の CoS の返事の添付から `ExchangeDocument` を集める」関数を 1 つ置き、
  REST の書き出しと MCP の `discussion_snapshot` の両方がそれを渡す形にする（担当は rdc-path か close の前の修理、書き込み範囲に task-core と
  attachment の読み出しを入れる）。その時に `chat_exchange_export_confidential_default_excluded` を「confidential の文書は
  `include_confidential` の共有が無ければ出ず、ある時だけ出る」の正負両方の試験に直す。
- `celerisctl exchange {share,revoke,export,import}` は rdc-path か子 7 で足す（ADR D4.1 の記述をそのとき満たす）。
