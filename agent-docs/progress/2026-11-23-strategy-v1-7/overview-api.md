---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: overview-api
status: done
completed: 2026-11-23
---
# 戦略層 v1 子 7 overview-api: 案件の概要の読み取り専用 GET

## 結論
- `GET /api/v1/projects/{id}/overview`（`celeris.project-overview/1`）を足した。導出は `task_ops::project_overview`、route は
  `task_api::handlers::project_overview`。どの表にも書かない（新しい表・列・migration・event・cache は 0）。mutating route は足していない。
- 14 項目（ADR D1.3）を決めた順で返し、各 entry に印（confirmed・estimated・needs_decision・contradicted）と drill-down の link を付ける。
  強い警告は規則 1（needs_decision・contradicted）と D1.3 の code。

## 決めたこと
- 戦略の版の一覧は `decisions`（key `strategy-approve-v*`）から引き、成果物の全走査をしない（D1.5）。manifest の一覧も `publish-signoff-v*` の決定から引く。
- O2 の stale は子 2 の `strategy::validate_document`（Intent::Record）の Stale・ContentHash の違反だけを使う（照合を書き直さない）。
- O14 の検査と署名の状態は `task_api::publish::manifest_status` を handler が `ManifestFacts` に写して渡す（同じ関数）。
- O7 の 3 軸は記録の `effective`（子 6 の検査が導いた値）を axis ごとの entry に分け、`negative_result` も別の entry にする。
- 承認は「最後の回答が approve で `is_human_principal`」だけ。operator の回答は承認にしない。
- schema の根の欄数（repo health の size）を増やさないため `script_generation` を根から外した（`ScriptGenerationView` は `ScriptGenerationList.generations` で `$defs` と web の型に残る）。

## 試験（`crates/task-api/tests/project_overview.rs`、19 件、全部 base に route が無いので変更前は 404 で落ちる）
`project_overview_lists_fourteen_items_in_order`・`project_overview_does_not_copy_existing_get_fields`・`project_overview_request_sha_matches_project_request`・
`project_overview_is_read_only`（全表の行数と event の数が 2 回の GET の前後で同じ）・`project_overview_strategy_versions_from_decisions_not_artifact_scan`・
`project_overview_unapproved_strategy_is_not_marked_approved`・`project_overview_operator_answered_strategy_is_not_approved`・
`project_overview_stale_strategy_is_contradicted_with_strong_warning`・`project_overview_contradicted_hypothesis_is_strong_warning`・
`project_overview_unapplied_refutation_is_strong_warning`・`project_overview_url_link_is_https_only`・`project_overview_external_source_is_estimated`・
`project_overview_open_decisions_are_needs_decision`・`project_overview_publish_readiness_from_signoff_decisions`・
`project_overview_unsigned_or_stale_manifest_is_strong_warning`・`project_overview_evaluation_axes_shown_separately`・
`project_overview_unsupported_evaluation_claim_is_estimated_with_warning`・`project_overview_other_project_data_excluded`・`project_overview_empty_project_items_absent`。

## 測定
- fixture（戦略 1 件・決定 1 件の案件）での応答時間: 2.65 ms と 2.60 ms（`cargo nextest run -p task-api --test project_overview --no-capture -E 'test(=project_overview_is_read_only)'` の 2 回）。
  本番の大きさ（task 1,715・decisions 473）では測っていない（未確認）。
- LoC（基準 `$CELERIS_WU_BASE`）: production Rust +1,457（task-ops `project_overview.rs` +1,390、handler +62、schema・handlers・lib・snapshot +5）、test +760、
  生成物（`docs/api/v1`・`web/api/generated`）+735 / −8。純増の理由: 新しい読み取りの口で、置き換える旧い仕組みが無い。

## 検査（提出時の HEAD で実行）
- `cargo nextest run -p task-api --test project_overview` → 19 passed。
- `UPDATE_SCHEMA=1 cargo nextest run -p task-api -E 'test(/committed_schema_matches_generated$/)'` → 1 passed、`corepack pnpm@12.6.0 -C web gen:types` → exit 0。
- `git diff --stat $CELERIS_WU_BASE -- crates/task-core/migrations` → 差分 0。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 7,132 passed・32 skipped、web 745 passed、repo-health verdict=pass）。

## commit（件名）
- strategy-v1 7 overview-api: 案件の概要の読み取り専用 GET /projects/{id}/overview（14 項目・印・強い警告・drill-down）と project_overview_ の試験
- strategy-v1 7 overview-api: O12 を関数に分け router の行を増やさない（size.responsibility-growth）、ADR の保証の表を実際の試験名に直す
- strategy-v1 7 overview-api: 進捗を書く

## 未確認・引き継ぎ
- 失効した引用（`citation_stale`）・予算の超過（`budget_exceeded`）・単価の不明（`cost_unknown`）は導出を実装したが試験が無い（ADR の未確認にも書いた）。
- `log` の link は出さない。O10 の `wall_secs` は返さない。戦略の文書の artifact link の `sha256` は空文字（承認済みの版は O2 の link に sha256 がある）。
- O7 の goal の軸は子 6 の評価器の入力の欠けのため Outcome の到達を表さない（ADR の未確認のとおり）。
- web-overview の WU は `ProjectOverview` の型（`web/api/generated/types.ts`）を使う。

## 親の目的への寄与
- observability（確認済み）: 人が 1 回の GET で 14 項目と印・強い警告を読める backend の口ができた。画面は web-overview の WU（未実装）。
- 改善 A〜D への直接の寄与は無い（読むだけの口）。
