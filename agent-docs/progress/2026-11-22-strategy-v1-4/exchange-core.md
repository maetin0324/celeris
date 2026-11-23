---
title: 戦略層 v1 子 4 exchange-core — discussion snapshot と Markdown/JSON の交換の純関数（task_core::discussion_exchange）
tasks: [01M7XQ9QK1KC34Q2A6PJ960TJB]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# exchange-core（WorkUnit。子 4 の段 core）

完了日: 2026-11-22。設計は [ADR 2026-11-22-external-llm-discussion-exchange](../../adr/2026-11-22-external-llm-discussion-exchange.md) の
D1.2〜D1.4・D1.6・D2・D3・D4.2・D4.3。

## 結論

- task-core に兄弟 module `task_core::discussion_exchange` を置いた（chat の本体に関数を足していない）。
  - (a) snapshot: 案件 thread の message のうち、人が共有し（`exchange_shared`）、取り消されず（`exchange_share_revoked` が無い）、期限内で、
    共有した版のまま（revision と本文の sha256 が一致し、撤回されていない）のものだけを返す。撤回は `retracted`、共有の後の編集は
    `edited_since_share` として `omitted` に id だけを出す。各 item は `content_hash`（マスク後の送る本文）・`source_revision`
    （`Project.request` の sha256 と承認済み Roadmap の版）・`references`・`actor`・`decision_state`・`confidence`・`consent`・`citation`
    （元の版の sha256）を持つ。MCP の client には自分の propose だけを加える。要約・戦略の file（呼び出し側が workspace から読んで渡す）は、
    運ぶ message が共有され、引用がすべて `current` で、confidential なら人が `include_confidential` で共有したか人の承認の決定があるときだけ出す。
    秘密の形は `envelope::mask_secrets` で `[masked]` にしてから出す。
  - (b) 書き出し・読み込み: envelope と snapshot の Markdown（front matter + 本文）と JSON。Markdown の front matter は ADR の例の YAML の部分集合
    （`key: value`、flow の map・list、`references:` の block list）を読む。snapshot の item は `body_bytes` を持ち、本文に `---` の行があっても
    元どおりに読める。読み込みは各 item の `content_hash` を本文と照らす（合わなければ `R4:content_hash`）。
  - (c) 取り込みの検査: `check_import`（人の貼り付け・Markdown）と `check_propose`（MCP の client）は、入力長（本文 16,384 byte・文書全体
    32,768 byte・references 32 件・url 2,048 byte・note 500 文字・key 128 文字）を切り詰めずに拒否し、`actor`・`consent`・`approval`・`direction`
    を文書が書けば `exchange_self_reported` で拒否し、Celeris が `actor`・`consent`・`idempotency_key` を埋めてから envelope-fix の
    `validate_envelope`（R3・R6・R7・R8・R9・R10・R11 を含む）を呼ぶ。規則は書き直していない（規約 2）。
- store の読み書きは既存の chat の関数（`chat_message_post_as_also`・`cited_message_conn`・`thread_require`・`read_tx`・`writer`）と既存の表
  （`chat_events`・`chat_messages.metadata_json`）だけを使う。新しい表・列・migration は無い。
  - 取り込み・propose は `role=user`・`discussion_mode=discuss`・author が呼んだ主体（`human:<名前>` か `mcp:<client_id>`）の message になり、
    envelope（本文を除く）を同じ transaction で `metadata_json.exchange` に書く。同じ key と本文は既存の message を返し（`duplicate`）、
    違う本文は `R7:idempotency_conflict`（409）。propose の key は `mcp:<client_id>:<idempotency_key>`。
  - 共有と取り消しは `chat_events` に追記する（type `exchange_shared`・`exchange_share_revoked`）。有効な共有は保存せず、読むたびに導く。
- 人の判断が要る点は無かった。ADR の契約から外れた点は次の 1 つ（「設計の付記」）。

## 設計の付記（ADR D2.1 の実装）

- 共有の 2 type は wire の `ChatEventType` に足していない。足すと `ChatEvent` の JSON Schema が変わり、`docs/api/v1/api-v1.schema.json`・
  `web/api/generated` の再生成（rest-import の範囲）が要る。代わりに `chat/run_store.rs` に定数 `CHAT_EVENT_EXCHANGE_SHARED`・
  `CHAT_EVENT_EXCHANGE_SHARE_REVOKED` を置き、`chat_events_page`（SSE の replay と run の events）の SQL がこの 2 type を飛ばす。
  共有の記録は監査の行で、SSE の event ではない。飛ばさないと、replay が未知の type で落ちる（下の probe）。
- 書き込みは `discussion_exchange/store.rs` の `append_record`（1 つの INSERT）。既存の `append_event` は wire の `ChatEventType` と
  `ChatEventData` を取る SSE の event の追記で、意味が違う（規約 8）。

## 変更

- `strategy-v1 4 exchange-core: discussion snapshot・人の共有・Markdown/JSON の交換・取り込みと propose の検査（task_core::discussion_exchange）`
  - `crates/task-core/src/discussion_exchange.rs`（新規）: 型（`ExchangeShareRequest`・`ExchangeShared`・`ExchangeShareRevoked`・`ExchangeItem`・
    `DiscussionSnapshot`・`ExchangeEnvelope`・`ImportRequest`・`ProposeRequest`・`ExchangeRefusal` など）と純関数（`active_shares`・
    `build_snapshot`・`check_import`・`check_propose`・`references_ok`・`source_label_ok`）。
  - `crates/task-core/src/discussion_exchange/format.rs`（新規）: `envelope_{to,from}_{json,markdown}`・`snapshot_{to,from}_{json,markdown}`。
  - `crates/task-core/src/discussion_exchange/store.rs`（新規）: `SqliteStore::{chat_exchange_share, chat_exchange_share_revoke,
    chat_exchange_snapshot, chat_exchange_import, chat_exchange_propose}`。後続の mcp-tools・rest-import はこれを呼ぶだけでよい
    （mcp-tools の書き込み範囲は task-core を含まないので、propose の store 側もここに置いた）。
  - `crates/task-core/src/discussion_exchange_tests.rs`（新規）: `external_exchange_*` の 16 試験。
  - `crates/task-core/src/chat/run_store.rs`・`chat/mod.rs`: 2 type の定数と再公開、`chat_events_page` の type の除外。
  - `crates/task-core/src/lib.rs`: `pub mod discussion_exchange;` の 1 行。

## 負の試験と、変更前に落ちること（受け入れ条件 2）

base（`$CELERIS_WU_BASE`）には `discussion_exchange` が無い（`git grep -c discussion_exchange $CELERIS_WU_BASE -- crates/task-core/src` は 0 件）。
下の試験はすべて base ではコンパイルで落ちる（`crate::discussion_exchange` が解決できない）。挙動として落ちることは次の probe で示した。

- `external_exchange_share_records_not_in_event_page` の probe: `chat_events_page` の type の除外だけを外して走らせた
  （`cargo nextest run -p task-core --lib -E 'test(external_exchange_share_records_not_in_event_page)'`、log は run の成果物
  `probe-events-page-without-filter.log`）→ **FAIL**、exit 100。`stored enum "exchange_shared": unknown variant`（共有の後に SSE の replay が落ちる）。
  除外を戻すと通る。

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `external_exchange_snapshot_default_empty` | 共有が無ければ snapshot は空（人の export も空） | snapshot が base に無い |
| `external_exchange_snapshot_shared_items_carry_envelope` | item の envelope の欄を Celeris が導く（hash・source_revision・actor・discussion・estimated・人の consent）。`project_id` だけで固定の thread に戻る | 同上 |
| `external_exchange_snapshot_omits_retracted` | 撤回した message は出ない（id と理由だけ）。撤回した message は共有できない | 同上 |
| `external_exchange_snapshot_omits_edited_since_share` | 共有の後の編集は出ない。共有し直せば今の版が出る | 同上 |
| `external_exchange_snapshot_revoked_or_expired_share_hidden` | 取り消し・期限切れの共有は出ない。共有の期限は 30 日以内 | 同上 |
| `external_exchange_snapshot_masks_secret` | 秘密を含む message の共有は `exchange_secret`。出力は `[masked]`、hash はマスク後、引用は元の sha256。JSON と Markdown に秘密が残らない | 同上 |
| `external_exchange_snapshot_omits_confidential` | confidential の文書は人の `include_confidential` か人の承認の決定が無ければ出ない。`mcp:` の共有・`mcp:` の承認・古い引用でも出ない。記録の `approved` は人の決定が無ければ `proposed` | 同上 |
| `external_exchange_snapshot_other_project_rejected` | 別案件の thread（`thread_project_mismatch`）・統括の thread（`thread_kind_not_project`）・別案件の envelope（R6）を拒否 | 同上 |
| `external_exchange_import_idempotent_same_hash` | 同じ key と本文は 1 件 | 取り込みが base に無い |
| `external_exchange_import_conflict_different_hash` | 同じ key で違う本文は 409（R7）。普通の message の key とも一致しない | 同上 |
| `external_exchange_import_injection_stays_discussion` | 注入文の取り込みは `proposed` のまま、`discuss`、author は人。`approved`・`rejected`・`withdrawn` は R11。文書の `approval`・`actor`・`consent`・`direction` は `exchange_self_reported`。共有して読んでも `approved` にならない | 同上 |
| `external_exchange_import_too_long` | 本文・文書・references・key の上限を超えると切り詰めずに拒否し、何も書かない | 同上 |
| `external_exchange_share_and_import_need_a_person` | `mcp:`・`cos:`・`operator:`・空の主体は共有も取り込みも 403 | 同上 |
| `external_exchange_propose_is_mcp_draft` | propose は author `mcp:<client_id>`・actor `mcp`・`discuss` の draft。他の client と人の export には共有が無ければ出ない。scope が無ければ R8、注入文の承認は R10・R11、繰り返しは 1 件 | 同上 |
| `external_exchange_markdown_json_roundtrip_hash` | Markdown と JSON の往復で `content_hash` と本文が保たれる（本文に `---` の行があっても）。ADR の例の手書きの front matter も読める。同じ本文の Markdown と JSON の取り込みは同じ hash。snapshot は両方の形で元どおり。書き換えた本文は R4 | 同上 |
| `external_exchange_share_records_not_in_event_page` | 共有の記録は SSE の replay に出ず、replay は落ちない | 上の probe のとおり、除外が無いと落ちる |

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-core --lib -E 'test(/external_exchange_/)'` → 26 tests run: 26 passed（`external_exchange_` 16、envelope-fix の
  `external_exchange_envelope_` 10）。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 8668 tests run: 8668 passed（13 slow）、31 skipped。web は vitest 105 files / 745 passed、
  test-ts-verify 20 passed / 0 failed、script bundle SKIP 0 件。
- 新しい表・migration が無い（受け入れ条件 0）: `git diff --name-only $CELERIS_WU_BASE HEAD -- crates/task-core/migrations` は 0 行、
  `git diff $CELERIS_WU_BASE HEAD | grep -ciE 'CREATE TABLE|ALTER TABLE'` は 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更 path は `crates/task-core/src/{discussion_exchange.rs,discussion_exchange/,discussion_exchange_tests.rs,lib.rs}`・
  `crates/task-core/src/chat/{mod.rs,run_store.rs}`・`agent-docs/progress/` の中。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`）

- production: +1,811 / −5（`discussion_exchange.rs` +954、`discussion_exchange/store.rs` +556、`discussion_exchange/format.rs` +282、
  `chat/run_store.rs` +15 / −2、`chat/mod.rs` +3 / −3、`lib.rs` +1）。純増の理由: snapshot・共有・取り込み・propose・Markdown/JSON の交換は
  base に無い機能で、後続の mcp-tools・rest-import はこの関数を呼ぶだけにする（規則を 3 か所に書かない）。新しい Manager・trait・状態機械・表は無い。
- test: `discussion_exchange_tests.rs` +994。

## 親の目的への寄与

- 確認済み（試験）: 外部 LLM に渡る範囲は人の共有だけで、撤回・編集・取り消し・期限・confidential・秘密で外に出ない。外部の文（人の貼り付け・
  MCP の propose）は `discuss` の未承認の draft で、注入文も `approved` にならず、文書の自己申告の主体・承認は拒否される（全体の不変条件
  「外部 LLM の発言を human actor や承認と取り違えない」「機微な情報は既定で外に出ない」、共有フォーマットの欄・idempotency・入力長の上限）。
- 推定: mcp-tools と rest-import がこの関数を呼べば、MCP と REST の両方で同じ範囲と規則が掛かる。実際に呼ぶかは後続の unit の review で確かめる。
- 未確認: 本物の ChatGPT・Muse・RDC の経路での動作（この unit は経路を作っていない）。

## 未解決

- 要約・戦略の file は呼び出し側が workspace から読んで `ExchangeDocument` として渡す。どの file を渡すか（CoS の返事の `workspace_files` から
  読む処理）は mcp-tools・rest-import が決める。
- CoS のコメントの advisory 2（`StrategyRef` と `strategy::ArtifactPointer` の重複）・3（D5.3 の fail-closed）はこの unit の範囲の外で、手を付けていない。
- Markdown の front matter は ADR の例の YAML の部分集合だけを読む（block の map・複数行の文字列は読めず `exchange_invalid`）。

## 提案

- rest-import は `ExchangeShareRequest`・`ImportRequest` を wire の型として `ApiV1Schema` に登録するとき、`strategy` の型（`Source`・`Confidence` など）が
  `JsonSchema` を持たないので、wire 側の型の扱いを決める（子 5 と重なる `strategy/mod.rs` は変えない）。
- 子 7 が共有の状態を Web に出すときは、SSE の event にするか、`chat_exchange_snapshot` を読む GET にするかを決める（今は SSE に出ない）。
