# CoS triage resolve API

---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
status: done
completed: 2026-10-06
---

## 完了

- `crates/task-api/src/cos/inbox.rs` に `GET /api/v1/cos/inbox`（`state`・`limit`）と `POST /api/v1/cos/inbox/{i}/resolve` を足した。`cos/mod.rs` で router を merge した。
- resolve は CoS run credential だけを受ける。人の bearer は 403 `cos_credential_required`、本文の `actor`・`thread_id` は 422。body は `{idempotency_key, expected_revision, outcome, reason, policy_version, answer?, escalation?}`。未知欄は 400。
- 判定の順: 冪等 key の再送なら既存 operation を返す（hash が違えば 409）→ item が無ければ 404 → 終端状態、`expected_revision` の不一致、同じ source の新しい revision の投入、元の待ちが派生 inbox から消えた（人が先に回答した）場合はどれも 409 `cos_inbox_revision_conflict`。404 より後の拒否は `OperationAudit::reject` で rejected 行と理由つきの監査 event を残す。
- answer: 既存の `InboxAnswerBody` を `inbox_notifications::delegated_request` で元の領域 request に直し、`/cos/operations` から切り出した共通の `operations::dispatch` に渡す。元の領域の検証（decision・approval・phase gate）は迂回しない。登録外の領域（質問への回答など）は 422 `cos_operation_not_allowed`。`OperationAudit.item`（`ItemMark`）があれば、`cos_operation_apply` の同じ transaction の中で item を `answered` にする。ここで item の状態が変わっていれば Conflict になり、全体を rollback する。領域の書込み・`cos_operations`・監査 envelope・chat card・item の状態は 1 transaction にまとまる。
- 明示 human_required の判定は `inbox::human_required_reason` に置き、resolve と `/cos/operations`（decision.answer・approval.decide・execution.phase_gate）の両方から呼ぶ。対象は human check の待ち（`AcceptanceCheck`）と、人が `human_required`/`human-required` のラベルを付けた task のすべての待ち（gate を含む）。結果は 403 `cos_human_required` で、rejected として記録する。
- observe: `answer`・`escalation` がどちらか非 null なら 422。元の待ちが未解決の inbox 項目（判断が要る）なら 422 `cos_observe_needs_judgment`。notice は observe できる。
- escalate: packet を検証して 422 `cos_escalation_invalid` を返す。条件は summary が 1〜600 字、options が空でない・key が重複しない・label が空でない・key が今の問いの選択肢であること（自由記述の問いだけ `reply`）、recommended が options の key か null、recommendation_reason が空でないこと。web_path はサーバが対象から導く path と完全一致させる（task があれば `/tasks/{id}`、無ければ `/inbox`、notice は `/notifications`）。外部 URL・`//`・相対 path は拒否する。answer 付きは 422。検証が通ったら `cos_triage_outbox_claim(item,"escalation",…)` で `notifications`（kind `cos_escalation`）を 1 行作り、続けて監査 operation の transaction で item を `escalated` にする。
- task-core に `chat/triage_view.rs` を足した（`CosInboxItem`、`cos_triage_item_get`・`cos_triage_items`・`cos_triage_superseded`・`cos_triage_mark_tx`）。`triage.rs` は ingest 葉と衝突しないように触っていない。

## 証拠

- `cargo test -p task-api --test cos_triage` → 8 passed（answer の成功と監査 event の actor=cos・理由・card・冪等な再送、人の credential の 403、409 の 3 通り、human_required の resolve と /cos/operations での拒否、observe の 422 と notice の observe、packet の検証、web_path の不一致の拒否、escalate で outbox 1 行・route 1 行・再送で増えないこと）。
- `cargo test -p task-api --no-fail-fast` → 全 binary が ok。既存の cos_operations・cos_operations_domains、lib の `committed_schema_matches_generated` を含む。
- `cargo test -p task-core triage` → 4 passed。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。

## 未解決

- escalate では、outbox の claim と監査 operation が別の transaction になる。store の `cos_triage_outbox_claim` が自前で transaction を開くため。先に claim するので、途中で crash しても outbox は残る。その後の再 resolve は route が取られていれば `already_routed=true` で item の状態だけを記録する。
- 新しい型（`ResolveBody`・`EscalationPacket`・`CosInboxItem`・`ResolveResponse`）を API schema に登録するのは schema 段で行う。
- 統合の前提（ingest 葉）: inbox 由来の item の `source_key` は、派生 inbox の item id（`decision-<id>` など）か `<source_kind>-<source_key>` がその id になる形にすること。`source_kind="notice"` は notice として扱う。
- 「CoS が代わりに答えた」カードは、既存の operation card（actor=cos・reason・operation_id）を同じ transaction で出す形にした。専用の文言は web 側の表示に任せる。

## 提案

- gate ごとの human_required を明示する欄（plan の stage gate に `human_required: true`）が無い。今は task のラベルで代用している。plan の段の定義に欄を足すかは人の判断に回したい。
