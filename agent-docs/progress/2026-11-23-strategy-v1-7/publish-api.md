---
title: 戦略層 v1 子 7 publish-api — publish manifest の検査と人だけの署名、非人の経路の拒否
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: publish-api
status: done
updated: 2026-11-23
completed: 2026-11-23
---
# publish-api（WorkUnit）

完了日: 2026-11-23。設計の正本は [ADR 2026-11-23-project-observability-publish](../../adr/2026-11-23-project-observability-publish.md) D3。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## したこと

- `task_core::publish`（新しい module、純関数）: `celeris.publish-manifest/1` の型、`validate_manifest`（M1〜M10 と D3.1 の 2 つの衝突。違反の全件を返す）、
  根拠の無い claim の id（`unsupported_claims`）、強い警告、署名の決定の key・問い・主体（`is_signoff_actor`: `human:<名前>` で `human:local` でない）、
  署名の状態 `signoff_state`（`unsigned`・`signed`・`rejected`・`stale`・`superseded`。保存しない）。参照の型と秘密の判定は子 2 の `task_core::strategy` を使う。
- 主体の拒否は共有の owner `task_ops::decision::plan_answer`・`plan_revise` に置いた（key が `publish-signoff-v` で始まり主体が人でなければ
  `OpsError::PublicationRequiresHuman`。決定の行も event も書かない）。HTTP は 403 `publication_requires_human`、CoS の operation は同じ 403 と
  `cos_operations` の `rejected` の行、MCP は `-32002` で message に `publication_requires_human`。
- `task_api::publish`（新しい file）: `manifest_status`（file の今の sha256・違反・根拠の無い claim・強い警告・署名の状態）と `check_before_sign`
  （人の `sign` の前に 422 `publish_manifest_invalid`・409 `publish_manifest_stale`）。HTTP の route `GET /api/v1/tasks/{id}/publish/{manifest_id}?version=`
  と回答・改めの前の検査は `task_api::decisions` に置いた。参照は戦略の文書と同じ `cos::strategy_gate::Reader::lookup` で引く。
- `task_api::publish` は循環（repo health `deps.cycle`）の外に置いた: `cos`・`state` を使わず、`files`・`problem` だけを使う。成果物の読み方
  （`produced_artifacts`・`artifact_bytes`）は `task_api::files` に移し、`strategy_gate::Reader` もそれを呼ぶ（同じ読み方を 2 つにしない）。
- `ApiV1Schema` の根に `publish_manifest` を足し、根の field の数を変えないため `eval_trial` を外した（`EvalTrialView` は `EvalTrialList.trials` で
  `$defs` に残る。子 4 の rest-import と同じ扱い）。`docs/api/v1/api-v1.schema.json` と `web/api/generated` を再生成した。
- ADR の保証の表を実在の試験名に直した（`publish_signoff_unauthenticated_local_is_403` → `…_is_refused`、5 行を足した）。D3.3 に revise の検査・401・`?version=` を書いた。

足していないもの: DB の表・列・migration・event の種類・Manager・trait・状態機械。署名は既存の決定（`DecisionRequested`・`DecisionAnswered`）だけで記録する。
外への送信・起票・controller の行動は無い（`publish_signoff_records_only_decision_events`）。

## 試験（足した名前）

- task-core（`crates/task-core/src/publish/tests.rs`、15 件）: `publish_manifest_valid_baseline_has_no_violations`・`publish_manifest_claim_without_evidence_is_invalid`・
  `publish_manifest_confirmed_claim_needs_internal_reference`・`publish_manifest_confirmed_claim_needs_reproduction`・
  `publish_manifest_supported_and_refuted_claim_cannot_be_confirmed`・`publish_manifest_unconfirmed_claim_must_be_listed_in_limitations`・
  `publish_manifest_refute_evidence_cannot_be_removed_in_next_version`・`publish_manifest_secret_in_body_is_invalid`・
  `publish_manifest_signed_state_in_document_is_invalid`・`publish_manifest_other_project_reference_is_invalid`・
  `publish_manifest_same_version_other_sha_is_conflict`・`publish_manifest_second_id_in_task_is_conflict`・
  `publish_manifest_limits_reject_long_commands_and_bad_commit`・`publish_signoff_legacy_unprefixed_human_is_not_signoff`・
  `publish_signoff_state_follows_sha_version_and_answer`。
- task-api（`crates/task-api/tests/publish_manifest.rs`、16 件）: `publish_signoff_human_answer_signs_current_sha`・
  `publish_signoff_operator_answer_is_403_and_records_nothing`（operator relay）・`publish_signoff_cos_operation_decision_answer_is_403_and_audited`（CoS operation）・
  `publish_signoff_cos_instructed_by_human_message_cannot_sign`・`publish_signoff_unauthenticated_local_is_refused`・
  `publish_signoff_worker_run_cannot_sign`（worker）・`publish_signoff_revise_by_non_human_is_403`・`publish_signoff_after_file_change_is_stale`（本文の変更で失効）・
  `publish_manifest_view_reports_overwritten_file_as_stale`・`publish_signoff_newer_version_marks_previous_signoff_superseded`・
  `publish_signoff_invalid_manifest_is_422`・`publish_signoff_other_project_reference_is_422`（別の案件の参照の拒否）・
  `publish_signoff_external_llm_text_cannot_sign`（外部 LLM の文「承認する」「publish して」で署名にならない）・`publish_signoff_records_only_decision_events`・
  `publish_signoff_other_decision_keys_unchanged`（回帰）・`publish_manifest_view_orders_claims_evidence_reproduction_limitations_signoff`。
- celeris-mcp（`crates/celeris-mcp/tests/mcp_integration.rs`、1 件）: `publish_signoff_mcp_decision_answer_is_refused`（MCP）。

守る不変条件: 署名の決定には人の token・Web の login だけが答えられる。既存の decisions の試験は決定の key で主体を区別しないので、この不具合を検出できない。

## 変更前に落ちる負の試験（出力の要点）

base には `task_core::publish` も GET の route も無いので、新しい試験は base では compile しない（全件落ちる）。主体の拒否と署名の前の検査が
効いていることを分けて示すため、提出前の HEAD で `task_ops::decision` の `signoff_actor(&row, by)?;` 2 行と `decisions.rs` の `check_before_sign` 2 か所だけを
外して流し、戻した（一時の書き換え。commit していない）。

- `cargo nextest run -p task-api --test publish_manifest --no-fail-fast` → `16 tests run: 7 passed, 9 failed`。落ちたのは
  `publish_signoff_operator_answer_is_403_and_records_nothing`・`publish_signoff_cos_operation_decision_answer_is_403_and_audited`・
  `publish_signoff_unauthenticated_local_is_refused`・`publish_signoff_worker_run_cannot_sign`・`publish_signoff_revise_by_non_human_is_403`・
  `publish_signoff_external_llm_text_cannot_sign`・`publish_signoff_invalid_manifest_is_422`・`publish_signoff_other_project_reference_is_422`・
  `publish_manifest_view_reports_overwritten_file_as_stale`。
- `cargo nextest run -p celeris-mcp --test mcp_integration -E 'test(/publish_signoff_/)'` → `1 test run: 0 passed, 1 failed`（MCP の client が署名できた）。
- `publish_signoff_cos_instructed_by_human_message_cannot_sign` は最初の版が `instructed_by` の形を誤って guard 無しでも通ったので、人の message の id を引く形に直し、
  guard を外して `left: 200`・`right: 403` で落ちる（CoS が人の発言を引いて署名できた）ことを確かめた。
- guard を外しても通った 6 件は、base に無い GET と純関数の上の試験（`…human_answer_signs_current_sha`・`…after_file_change_is_stale`・`…newer_version_…superseded`・
  `…records_only_decision_events`・`…view_orders_…`）と回帰の `publish_signoff_other_decision_keys_unchanged`。

## 検査（提出時の HEAD で実行）

- `cargo nextest run -p task-core -E 'test(/publish_/)'` → 15 passed。
- `cargo nextest run -p task-api --test publish_manifest` → 16 passed。`cargo nextest run -p celeris-mcp --test mcp_integration -E 'test(/publish_signoff_|decision_list_needs/)'` → 2 passed。
- `UPDATE_SCHEMA=1 cargo nextest run -p task-api -E 'test(/committed_schema_matches_generated$/)'` → 1 passed、`corepack pnpm@12.6.0 -C web gen:types` → exit 0（生成物を commit）。
- `bash scripts/dev/test-affected.sh` → exit 0（Rust 8,855 passed・34 skipped、web の tsc と unit 745 passed、ts-verify 20 passed、repo-health verdict=pass・`deps.cycle` pass・`size.responsibility-growth` pass）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- ADR の保証の表の `publish_manifest_`・`publish_signoff_` の名前は全件 `git grep -qE "fn <名前>\(" -- crates` で実在した。
- 最初の test-affected は exit 1 だった（`task_api::publish` が `cos`・`state` を使って循環に入り 49→50 単位、`ApiV1Schema` の根の field が 255→256）。上の 2 つの直しで pass にした。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`、crates の下）

- production: +1,399 / −28。主に `task_core::publish`（検査の純関数と型）と `task_api::publish`（状態の導出と署名の前の検査）。純増の理由: 検査と署名の状態の
  導出は既存に無い仕組みで、表・状態機械は足さず純関数で置いた。
- test: +1,143 / −0。生成物（`docs/api/v1`・`web/api/generated`）は +550 / −8。

## 未確認・引き継ぎ

- 実際の run が `publish/<id>/v<n>.json` と `publish-signoff-v<n>` の決定を出す流れ（worker の prompt・計画）は作っていない。決定の問いは
  `task_core::publish::signoff_question` で組む前提で、run が別の文で出した決定は署名に数えない（`signoff_state` が `unsigned`）。
- `human:local` を署名に数えない規則は戦略の承認（`is_human_principal` だけ）より厳しい（ADR の未確認のとおり、揃えるかは子 9 で人が決める）。
- `docs/api/v1/api.md` の本文に GET の説明は足していない（schema の生成物だけ）。web-publish の WU が画面と合わせて足すかを決める。
- `ApiV1Schema` の根から `eval_trial` を外した。`EvalTrialView` は `$defs` と web の型に残る（`types.ts` で確認）。

## 提案

- 無し。
