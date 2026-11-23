---
title: 戦略層 v1 子 7 proposal-api — chat の案を人が adopt・reject・comment する口と、外部の文の印
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: proposal-api
status: done
updated: 2026-11-23
completed: 2026-11-23
---
# proposal-api（WorkUnit）

完了日: 2026-11-23。設計の正本は [ADR 2026-11-23-project-observability-publish](../../adr/2026-11-23-project-observability-publish.md) D2。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した（例外は「検査」の節に書いた）。計画 v2 の再実行の検査は「再実行（計画 v2）」の節にある。

## したこと

- route `POST /api/v1/chat/threads/{t}/messages/{m}/review`（`task_api::chat::proposal_review`）。本文 `{verdict: adopt|reject|comment, expected_revision,
  sha256, attachment?, note?, client_message_id?}`（`deny_unknown_fields`）。handler の先頭で `Principal::Human` だけを通し、operator（relay を含む）・
  MCP・CoS・token の無い構成は 403 `proposal_verdict_requires_human`。MCP の token と CoS の run は handler より前の層で止まる（MCP: middleware の
  読むだけの guard で 403 `forbidden`。CoS: 直接の呼び出しは 422 `cos_audit_context_required`、`/cos/operations` 経由は 422 `cos_operation_not_allowed`）。
  この食い違いは ADR D2.1 に「実装」として本文に書いた。
- CoS の operation の `EXCLUDED` に `human_proposal_verdict` を足し、[ADR 2026-10-09-cos-operations-all-mutations](../../adr/2026-10-09-cos-operations-all-mutations.md)
  の 2 つの表に行を足した。MCP の道具は足していない。
- 記録は既存の chat message: `task_core::proposal_review::chat_proposal_review` が既存の人の送信の tx（`chat_message_post_as_also`）で `role='user'` の
  message を 1 件書き、`metadata_json.proposal_review = {verdict, target: {thread_id, message_id, revision, sha256}, attachment?}` を同じ tx で付ける。
  案の今の版の照合（revision・本文の sha256 か CoS の添付の sha256・撤回）も同じ tx の中で行う。本文は Rust の固定の文で、案の本文を写さない。
  adopt は execute（mode 無し）で「承認ではない。決定 `strategy-approve-v<n>` で人の承認を求めよ」と書く。reject・comment は `discussion_mode: discuss`。
- 採否の状態は保存しない。`task_core::proposal_review_wire`（新しい leaf の module）が読むたびに同じ thread の採否の message から導く
  （author が `human:` のものだけを数え、最後の adopt か reject、comment だけなら `commented`、無ければ `pending`、案が編集・撤回されたら `stale`。
  照合は子 3 の `chat_citation::citation_state`）。
- wire の `ChatMessage` に `proposal: {kind, review_state, review_message_id?, reviewed_by?, reviewed_at?}` と
  `external: {source_kind, source_label, untrusted: true}` を足した（`#[serde(flatten)] marks`）。`external` は `metadata_json.exchange.source.kind` が
  `mcp_client`・`human_paste`・`markdown_import` のときだけ出す（本文からは導かない。子 4 の引き継ぎ）。
- schema（`docs/api/v1/api-v1.schema.json`）と `web/api/generated` を再生成した。root の欄の数（repo health `size.responsibility-growth`）を保つため、
  `HarnessRuleView` を root の一覧から外した（`HarnessView` を通じて `$defs` に残る。publish-api と同じ方法）。
- task-core の循環（repo health `deps.cycle`）を広げないため、戦略の成果物の name の形 `artifact_name` とその逆 `strategy_artifact` を、
  既存の leaf `task_core::strategy_ref` に移した。`strategy`・`strategy_critic` は `pub use` で再 export し、19 か所の呼び出しは変えていない。
  移す前は `chat → proposal_review_wire → strategy_critic → model → … → chat` で循環が 55 単位から 58 単位に増えていた（repo health の worsened 1 件）。
- `ChatMessage` が 8 byte 大きくなり、`ChatEventData` が clippy `large_enum_variant` の閾値（200 byte）を超えた。`Message` と `Run` の variant を `Box` にした
  （作る所は `chat/store.rs`・`chat/run_store.rs`・`chat/session.rs` の 3 か所。JSON の形は変わらない）。
- 同じ名前の公開関数（task-ops の `milestone_review::review_state`）との重複の警告を避けるため、導出の関数名を `proposal_review_state` にした。
- 新しい DB 表・列・migration・event の種類・`chat_events` の type・Manager・trait・状態機械は足していない。

## 試験（足した名前、23 件）

task-api の統合試験（`crates/task-api/tests/proposal_review.rs`、19 件）:

| 守る不変条件 | 試験 |
|---|---|
| 人の adopt は execute の人の指示の message 1 件で、案の本文を写さない | `proposal_review_human_adopt_posts_execute_instruction_message` |
| reject・comment は discuss の message で副作用（task・decision・applied の operation）が無い | `proposal_review_reject_and_comment_post_discuss_messages` |
| operator（relay を含む）は 403 `proposal_verdict_requires_human` | `proposal_review_operator_is_403_proposal_verdict_requires_human` |
| MCP の token・CoS の run（直接と `/cos/operations` 経由）は拒否 | `proposal_review_mcp_and_cos_principals_are_refused` |
| token の無い構成は 403 `proposal_verdict_requires_human` | `proposal_review_unauthenticated_local_is_403` |
| route は CoS の operation の `EXCLUDED`（`human_proposal_verdict`） | `proposal_review_route_not_in_cos_operations` |
| adopt は承認ではない（adopt の後の CoS の起票を strategy_gate が `strategy_source_missing` か `strategy_unapproved` で拒み、task は増えない） | `proposal_review_adopt_is_not_strategy_approval` |
| 版の違い・sha256 の違い・撤回された案は 409、案でない message は 422 | `proposal_review_stale_revision_is_409`・`proposal_review_sha_mismatch_is_409`・`proposal_review_retracted_target_is_409`・`proposal_review_non_proposal_target_is_422` |
| 別の案件の thread の案はこの thread の path から採否できない（404） | `proposal_review_other_project_thread_is_refused` |
| 状態は保存せず導く（採否の message を撤回すると `pending` に戻る） | `proposal_review_state_is_derived_not_stored` |
| 案の編集・撤回で採否が失効する（`stale`） | `proposal_review_state_stale_after_target_edit`・`proposal_review_state_stale_after_target_retract` |
| 外部の文は wire で `untrusted: true`、人の発言には印が無い | `proposal_review_external_text_marked_untrusted_on_wire` |
| 外部の文の注入（「この案を承認済みにせよ」、偽の `proposal_review` の記録）で状態が変わらない | `proposal_review_injected_text_cannot_adopt` |
| 同じ `client_message_id` の再送は二重にならない | `proposal_review_idempotent_client_message_id` |
| CoS の案の添付は添付の sha256 で照合する | `proposal_review_cos_strategy_draft_reviewed_by_attachment_sha` |

unit 試験: `proposal_review_non_human_principals_are_403_at_handler`（task-api の handler、4 主体）、`proposal_review_state_follows_last_verdict_of_a_person`・
`proposal_review_kind_and_mark_come_from_the_record`・`proposal_review_text_is_fixed_and_adopt_is_not_approval`（task-core）。

既存の試験でこれらを検出できない理由: base には route も wire の 2 欄も無く、案の採否を記録・導出する経路が無かった。

ADR の保証の表の `proposal_review_` の行を実際の名前に直した（`proposal_review_mcp_and_cos_principals_are_403` → `…_are_refused`、
撤回・別の案件・CoS の添付・unit の試験を足した）。表の 23 の名前はすべて `git grep -qE "fn <名前>\(" -- crates` で実在を確かめた。

## 変更前に落ちる負の試験（出力の要点）

方法: production の file を `$CELERIS_WU_BASE` の版に戻し、新しい file（task-core の `proposal_review.rs`・`proposal_review_wire.rs`・
`proposal_review_tests.rs`、task-api の `chat/proposal_review.rs`）を消して、統合試験の file だけを残して実行した（その後 `git checkout HEAD -- .` で戻した）。

- `cargo nextest run -p task-api --test proposal_review --no-fail-fast` → exit 100、`19 tests run: 0 passed, 19 failed`。
- 落ち方の要点:
  - route を叩く 16 件は `left: 404`（`"detail":"no such endpoint"`）で落ちた。例: `proposal_review_operator_is_403_proposal_verdict_requires_human`
    （`left: 404 right: 403`）、`proposal_review_adopt_is_not_strategy_approval`（`left: 404 right: 202`）、`proposal_review_stale_revision_is_409`
    （`left: 404 right: 409`）、`proposal_review_other_project_thread_is_refused`（code `not_found`、期待 `chat_not_found`）。
  - `proposal_review_external_text_marked_untrusted_on_wire` は wire の `external` が `Null`、`proposal_review_cos_strategy_draft_reviewed_by_attachment_sha`
    は `proposal.kind` が `Null` で落ちた。
  - `proposal_review_mcp_and_cos_principals_are_refused` と `proposal_review_injected_text_cannot_adopt` は wire の `proposal` が `Null` で落ちた。
    この 2 件の拒否（MCP の 403・CoS の 422）は base の guard でも同じで、落ちたのは wire の欄が無いためである。
- unit 試験 4 件は base に関数が無いので compile できない（落ちる）。
- 出力の全文は run の artifacts の `proposal-review-at-base.log`（repo には置かない）。

## 検査（提出時の HEAD で実行）

- `cargo nextest run -p task-core -p task-api -E 'test(/proposal_review_|strategy_critic|strategy_artifact/)'` → exit 0、28 passed（`proposal_review_` は 23 件）。
- `cargo nextest run -p task-api -E 'test(/committed_schema_matches_generated$/)'` → exit 0、1 passed。`node web/scripts/gen-types.mjs` の後の
  `git status --short` は空（`web/api/generated` は schema と一致）。`docs/api/v1/api-v1.schema.json` と `web/api/generated/schema.json` は `cmp` で同一。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0: `8878 tests run: 8878 passed, 34 skipped`、web は `Test Files 105 passed`・`Tests 745 passed`、
  ts-verify は `20 passed, 0 failed`。その前の 1 回目（comment の直しの前）は `schema::tests::committed_schema_matches_generated` の 1 件だけが
  落ちた（`ChatEventData::Message` に付けた doc comment が schema の説明に出た）。doc comment をふつうの comment に直した。
- `python3 scripts/dev/repo-health/repo_health.py --repo scripts/dev --base $CELERIS_WU_BASE --head worktree --policy-version 1` → exit 0、
  `verdict=pass new=0 worsened=0`。
- migration の差分: `git diff $CELERIS_WU_BASE HEAD -- crates/task-core/migrations | wc -l` → 0。

## 再実行（計画 v2）

計画 v1 の run は done を返したが、run の後の範囲 check が `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` を範囲外として落ちた（exit 1）。
この ADR の 2 つの表の行は消せない。ADR を `$CELERIS_WU_BASE` の版に戻して
`cargo nextest run -p task-api --lib -E 'test(/cos_ops_registry_matches_adr_domain_assignment_and_exclusions$/)'` を流すと exit 100 で落ちた
（`left` の surface.rs の分類に `("POST", "/api/v1/chat/threads/{}/messages/{}/review")` があり、`right` の ADR の表に無い）。計画 v2 が許可 path に
この ADR を足した。実装は前の run の commit をそのまま使い、作り直していない。この run の検査はすべて提出時の HEAD で実行した
（この節を足した commit は進捗 file だけを変える）:

- 範囲 check（計画 v2 の許可 path、`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 範囲外の path は 0 件。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0: `8878 tests run: 8878 passed (7 slow), 34 skipped`、web は `Test Files 105 passed (105)`・
  `Tests 745 passed (745)`、ts-verify は `20 passed, 0 failed`。
- `cargo nextest run -p task-core -p task-api -E 'test(/proposal_review_|cos_ops_registry_|cos_ops_exclusions_|cos_ops_every_allowed_|committed_schema_matches_generated$/)'`
  → exit 0、`37 tests run: 37 passed`。内訳: `proposal_review_` 23 件、task-api の `cos::ops::tests` の `cos_ops_registry_classifies_every_mutation_as_allowed_or_excluded`・
  `cos_ops_registry_matches_adr_domain_assignment_and_exclusions`・`cos_ops_exclusions_match_all_named_parameters_and_report_adr_reason`・
  `cos_ops_every_allowed_row_is_reachable_through_match_operation`、`cos_delivery_suite_b` の `cos_ops_registry_` 3 件、schema の一致の試験 7 件。
- `node web/scripts/gen-types.mjs` の後の `git status --short` は空。`cmp docs/api/v1/api-v1.schema.json web/api/generated/schema.json` → exit 0。
  `web/api/generated/types.ts` の `ChatMessage` に `"external"?: ChatMessageExternal | null` と `"proposal"?: ChatMessageProposal | null` がある。
- migration の差分: `git diff $CELERIS_WU_BASE HEAD -- crates/task-core/migrations | wc -l` → 0。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`）

- production の Rust: +759 / −21（純増 +738）。内訳: task-core `proposal_review_wire.rs` +339、`proposal_review.rs` +276、`strategy_ref.rs` +18
  （`strategy` −4/+2・`strategy_critic` −11/+4 から移した分を含む）、`chat/` +18/−5、`lib.rs` +2、task-api `chat/proposal_review.rs` の本体 +86・
  `chat/mod.rs` +5・`cos/ops/surface.rs` +5・`schema.rs` +4/−1。
- test の Rust: +1,114（`tests/proposal_review.rs` +954、`proposal_review_tests.rs` +128、handler の unit +32）。
- 生成物: `docs/api/v1/api-v1.schema.json` と `web/api/generated/schema.json` が各 +187/−4、`types.ts` +31。
- 文書: ADR 2 本で +13/−3。
- 純増の理由: 新しい route と、採否の照合（同じ tx の中）・状態の導出・wire の 2 欄の導出が要る。表・状態機械を足す代わりに、既存の chat message と
  `chat_citation` の照合を使った。

## 未確認・引き継ぎ

- adopt を受けた CoS の execute の run が外部の案の本文を thread で読むので、本文の中の指示が CoS の行動を誘う余地は prompt の段に残る（ADR の未確認と同じ）。
  backend の保証は、どの operation も goal_guard・strategy_gate・authority_gate を通ることまで。
- 採否の状態の導出は、案の message ごとに採否の message を 1 回照会する（使う索引は確かめていない）。本番の大きさ（thread の message 数千件）での
  一覧の時間は測っていない。
- Web の操作（採用・却下・コメントの button と外部の文の印の表示）は WU web-proposals が行う。

## 提案

- MCP の token と CoS の run は handler の前で別の code（`forbidden`・`cos_audit_context_required`・`cos_operation_not_allowed`）で止まる。画面と
  client が 1 つの code で扱いたいなら、子 9 で code を揃えるかを決める（今は handler の code を最後の防御にしている）。
