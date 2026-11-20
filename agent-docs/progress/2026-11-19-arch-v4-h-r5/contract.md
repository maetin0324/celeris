---
title: アーキ v4 H R5 contract — 6 op の HTTP と CoS の契約を移す前のコードで固定する試験
tasks: [01M7VJY8NZ5G84ACBNQTPVV3R4]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# contract（WorkUnit）

完了日: 2026-11-20

## 0. 結論

- `crates/task-api/tests/cos_delivery_suite/applied_contract.rs`（既存の test binary `cos_delivery_suite` の module。
  `main.rs` に `mod applied_contract;` を足しただけで、tests/ 直下に新しい binary は作っていない）に、
  `applied_contract_` の試験を 12 本足した。6 op（knowledge_accept・knowledge_reject・knowledge_page_put・task_pause・
  skill_put・skill_delete）× 2 つの入口（HTTP・CoS）の 12 組に 1 本ずつある。
- 12 本とも移す前のコード（WorkUnit の base `$CELERIS_WU_BASE`）で pass（提出時の HEAD で実行。下の 3 節）。
- `crates/task-api/src` は変えていない。変えたのは `crates/task-api/tests/cos_delivery_suite/applied_contract.rs`（新規）・
  `crates/task-api/tests/cos_delivery_suite/main.rs`（1 行）とこの file だけ。policy の file も変えていない。
- 設計は ADR 2026-11-20 applied-shape-migration（WorkUnit `adr` が書く）。impl の 3 つの WorkUnit
  （knowledge・pause・skills）は、移した後もこの 12 本が無修正で通ることを確かめる。

## 1. 試験の一覧

拒否の行の形は 3 つあり、試験はそれぞれ今のコードの形を固定する。

- **前段の拒否**（`OperationAudit::reject`）: 行は `state=rejected`・`action=rejected`、理由は
  `rejected: <detail>; requested reason: <依頼の理由>`。監査 event は rejected の 1 件。
- **transaction の中の拒否**（`OperationAudit::apply` の closure の失敗）: 行は `state=rejected` で、`action` は操作の名前
  （例 `knowledge.accept`）、理由は依頼の文のまま。`result.error` に失敗の文言が入る。
- **外部効果の拒否**（`OperationAudit::external` と `fail_external`）: pending の行が `rejected` になる。`result` は
  `{"error": <code>, "detail": …}`、監査 event は pending → rejected。

| 試験 | op | 入口 | 守る点 | 既存の試験で検出できない理由 |
|---|---|---|---|---|
| `applied_contract_knowledge_accept_http_response_and_refusals` | knowledge_accept | HTTP | 成功の JSON は `etag`・`path`・`sha`・`unchanged` の 4 欄で、`unchanged=false`、`sha` は KB の HEAD。id に `/` を含むと 403 `path_forbidden`。取り込み先 `path` が根の外なら 403、`.md` 以外なら 422 `validation`。どの拒否でも候補が残り、commit は増えない | `knowledge.rs` の `candidates_can_be_accepted_or_rejected` は `path`・`etag` があることと 404 だけを見る。`sha` の値、欄の集合、accept の 403、取り込み先の検査で拒否したとき候補が残ることは見ない |
| `applied_contract_knowledge_accept_cos_result_and_rejections` | knowledge_accept | CoS | applied の `result` は `id`・`path`・`sha` の 3 欄で、`sha` は KB の HEAD。target は `knowledge`/<候補 id>。取り込み先 `path` の誤りは前段の拒否（403）。既にあるページへの取り込み（409 `page_exists`）と知らない候補（404 `candidate_not_found`）は transaction の中の拒否（action `knowledge.accept`）。どの拒否でも候補と既存のページは変わらず、commit は増えない | `cos_ops_mutations.rs` の `cos_ops_mutations_knowledge_accept` は成功の `result.path` だけを見る。拒否の行・理由・rollback は見ない |
| `applied_contract_knowledge_reject_http_response_and_refusals` | knowledge_reject | HTTP | 成功の JSON は `id`・`sha` の 2 欄で、`sha` は KB の HEAD。候補の file は消える。知らない候補は 404 `candidate_not_found` で、commit は増えない | `candidates_can_be_accepted_or_rejected` は reject の 403 と成功の status だけを見る。応答の欄と reject の 404 は見ない |
| `applied_contract_knowledge_reject_cos_rejections_leave_kb_unchanged` | knowledge_reject | CoS | applied の `result` は `id`・`sha` の 2 欄。空でない `request.body` は前段の拒否（422、理由 `rejected: request.body must be empty; requested reason: …`）。知らない候補は transaction の中の拒否（404、action `knowledge.reject`）。どちらでも候補が残り、commit は増えない | `cos_operations_domains.rs` の `cos_chat_ops_domain_knowledge_reject` は `result.sha` と二度目の 404 の status だけを見る。行の state・action・理由と、body の検査は見ない |
| `applied_contract_knowledge_page_put_http_response_and_refusals` | knowledge_page_put | HTTP | 成功の JSON は `etag`・`path`・`sha`・`unchanged` の 4 欄で、`sha` は KB の HEAD、`etag` は `GET /knowledge/page` の etag と同じ。まだ無いページに etag を付けた PUT は 409 `etag_mismatch` で、`etag` 欄を持たない。file も commit も作らない | `writing_a_page_checks_the_etag_and_the_path` は、既にあるページの etag の 409 と 403・422・`unchanged` を見る。成功の欄の集合・`sha`・新規ページの etag の 409 は見ない |
| `applied_contract_knowledge_page_put_cos_rejections_and_human_mark` | knowledge_page_put | CoS | applied の `result` は HTTP と同じ 4 欄で、target は `knowledge_page`/<path>。CoS の編集も GUI と同じ『人が書いた』印（`sources: ["human:authored"]`、`protected_page`）を付けて保存する。`_inbox/` は前段の拒否（403）。古い etag は外部効果の拒否（`{"error":"etag_mismatch","detail"}`）。拒否ではページの中身も commit 数も変わらない | `cos_ops_projects_cron_docs.rs` の `cos_ops_projects_cron_knowledge_page_put_is_external_once` は applied・再送・etag の 409 と commit 数・`_inbox` の status だけを見る。行の形・理由・`result`・人の印は見ない |
| `applied_contract_task_pause_http_response_and_refusals` | task_pause | HTTP | 成功の JSON は `paused_at`・`subtree`・`task` の 3 欄で、`task.id` は対象。`Edited { fields: ["paused_at"], by: "human:test" }`（認証した principal）を 1 件残す。知らない task は 404 `task_not_found`、ULID でない id は 400 `bad_request`、終わった task は 409 `invalid_transition` で、`paused_at` も event も書かない | `project_plan.rs` の pause の試験は 200・`paused_at`・`subtree`・二重 pause の 409・404 の status だけを見る。欄の集合・`Edited.by`・404 の code・400・終わった task の拒否は見ない |
| `applied_contract_task_pause_cos_same_transaction_and_rejections` | task_pause | CoS | task の `paused_at`・`Edited` event と、applied の行・監査 event を同じ transaction で書く。applied の行の INSERT を trigger で落とすと、task は止まらず `Edited` も残らない（片方だけ残ることがない）。成功の `result` は `{"task_id", "paused": true}`、`Edited.by` は `cos:<thread>`。知らない task（404）と既に止めた task（409）は前段の拒否で、task は変わらない | `cos_ops_mutations_task_edit_pause_resume_reopen_retry` は applied と `paused_at` があることだけを見る。`tx_boundary_contract.rs` の同じ trigger の試験は案件の cancel だけを見る |
| `applied_contract_skill_put_http_source_and_refusals` | skill_put | HTTP | GUI の書き込みは frontmatter に `source: gui` を残し、応答は `{"path"}` だけ。KB が無ければ 409 `knowledge_unavailable` で、KB を作らない | `unmapped_suite_b/skills.rs` の `put_skill_creates_and_updates_and_requires_admin` は `path`・422・401 だけを見る。`source` と未初期化の拒否は見ない |
| `applied_contract_skill_put_cos_source_and_rejections` | skill_put | CoS | CoS の書き込みは frontmatter に `source: cos` を残し、`result` は `{"path"}`。名前の不一致は外部効果の拒否（422、`{"error":"validation","detail"}`）で、SKILL.md も commit も作らない。KB が無ければ前段の拒否（409） | `cos_ops_admin_config_org_and_skills_are_audited` は applied の `result.path` と file の有無だけを見る。`source` と拒否の行は見ない |
| `applied_contract_skill_delete_http_status_and_refusals` | skill_delete | HTTP | 成功は 204 で本文が空、SKILL.md を消して KB の commit が 1 つ増える。知らない skill は 404 `skill_not_found`。mount 中は 409 `skill_mounted` で、SKILL.md・mount・commit 数は変わらない | `mounting_a_skill_on_a_node_shows_up_in_mounted_by_including_children` は mount 中の 409 の code だけを見る。204 の本文・commit・404・拒否で file と mount が残ることは見ない |
| `applied_contract_skill_delete_cos_result_and_rejections` | skill_delete | CoS | applied の `result` は `{"name", "deleted": true}`。知らない skill（404、理由 `rejected: skill not found: nope; requested reason: …`）・mount 中（409）・空でない `request.body`（422）は前段の拒否で、SKILL.md・組織の mount・commit 数は変わらない | `cos_ops_admin_config_org_and_skills_are_audited` は、mount 中の拒否の行の数と `result.deleted` だけを見る。理由の文言・404・body の検査・拒否で org と KB が変わらないことは見ない |

各試験の doc comment に、守る点と既存の試験で検出できない理由を書いた（上の表と同じ内容）。

## 2. 反例の確認（試験が不具合を検出すること）

`applied_contract_task_pause_cos_same_transaction_and_rejections` が同じ transaction を守ることを確かめるため、
`crates/task-api/src/lifecycle.rs` の `task_pause_op` を一時的に壊して試験を走らせた。壊し方は、CoS の経路で
`store.update_task` を `audit.apply` の前（transaction の外）で呼ぶもの。試験は
`監査の行が書けなければ task も止めない` で FAIL した。その後 `git checkout -- crates/task-api/src/lifecycle.rs` で戻し、
`git status --short` で src に差分が無いことを確かめた。

## 3. 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-api --test cos_delivery_suite --no-tests=fail -E 'test(applied_contract_)'` → exit 0、
  `12 tests run: 12 passed, 38 skipped`。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo clippy -p task-api --tests -- -D warnings` → exit 0（試験の file も lint する）。
- `bash scripts/dev/test-affected.sh` → 結果は 4 節。
- 範囲: `git diff --name-only $CELERIS_WU_BASE HEAD` と未追跡の file は、上の 3 つの file だけ（`crates/task-api/src` を含まない）。

## 4. test-affected

`bash scripts/dev/test-affected.sh` の結果は result.json の evidence にある（基準は `$CELERIS_WU_BASE`）。

## 5. 未解決事項

- 無し。

## 6. 提案

- 無し。
