---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: CampaignIdentity 保存・全祖先整合・冪等性（core-store）
work_unit: core-store
status: done
updated: 2026-10-19
---
# core-store

## 変更
- `CampaignIdentity` に `variant: Option<CampaignVariantRef>`・`variant_org: Option<CampaignOrgPin>`（serde default・skip。`deny_unknown_fields` は維持）。`validate` は片欠けを拒否し、各 wire 型の形検証を呼ぶ。`decision_log` から 2 型を re-export。
- migration **0088** `0088_task_campaign_variant.sql`: `task_campaigns.variant_json`・`variant_org_json`、`decision_log_runs.variant_id`・`variant_content_sha256`。**0087 は別 branch（`celeris/01M59W8DEFMMP2C2SX5VE4KTKG` 系の `0087_decision_log_lane_stages.sql`）が使用中**のため飛ばし、`RESERVED_VERSIONS` に 87 を追加（audit 時点の「0087 空き」は古くなっていた）。`SCHEMA_VERSION = 88`、版数の固定試験を更新。
- store（`crates/task-core/src/store/decision_log.rs`）:
  - `task_campaign_set_tx`: 既に違う識別を持つ task への再設定を拒否（従来は置換。同じ識別は no-op）。祖先鎖（root まで）の識別と違えば拒否、既に識別を持つ子孫と違っても拒否。`task_campaign_set` は自前の transaction で包む。
  - `task_campaign_own(_tx)`: 共通の `campaign_from_row`。壊れた JSON・片欠け・形の不正・repetition の範囲外は `StoreError::Invalid`（黙って variant 無しにしない）。
  - `task_campaign_resolve`: 名前と型はそのまま、中身を checked に置換（旧い最寄り優先だけの実装は消した）。root まで辿り、行を持つ全祖先が一致しなければ `Invalid`、循環・`CAMPAIGN_ANCESTOR_LIMIT`(32) 超えも `Invalid`（従来は上限で `Ok(None)`）。1 つの lock の中で読む。
  - `task_campaign_resend_tx`: 判定は従来どおり全欄一致 → 新欄込み（variant 変更・脱落・追加は `Conflict`）。
  - 決定ログ: `DecisionLogProposal.variant_id`・`variant_content_sha256` を `apply_campaign` で写し、run 行に保存・再読。
- 既存の非 variant 行: 新列 NULL → `variant: None`。意味は不変。

## 証拠
- `cargo nextest run -p task-core` → 1028 passed, 0 failed（新試験 `store::campaign_variant_tests` 11 件: 保存再読・再 open 後の Same・変更/構成変更/脱落の Conflict と同 task 再設定拒否・片欠け拒否・子孫継承と root 不一致拒否・既存子孫に反する root 設定拒否・保存済み矛盾を最寄りで隠さない・循環/上限・不正保存値・create transaction 失敗で task/events/識別/冪等キー/decision_log が残らない・run 行の variant）
- `cargo clippy -p task-core --all-targets -- -D warnings` → 0（core-wire の試験の `field_reassign_with_default` 2 件も直した）
- `cargo clippy --workspace -- -D warnings` → 0
- `cargo fmt --all -- --check` → 0
- `cargo nextest run -p task-api -E 'binary(campaign_ident)'` → 1 passed（既存の POST 再送・不一致）

## 次段の葉が直すもの（本葉は task-core 以外を変えていない）
- `cargo check --workspace --all-targets`: task-dispatch の試験 literal 4 箇所（`decision_log_wire.rs:705`、`tests/trial_context.rs:77`、`tests/lane_assist.rs:378,420`）が `variant`/`variant_org` 欠落で compile error → dispatch 葉で `variant: None, variant_org: None`。
- `task-api schema::tests::committed_schema_matches_generated` が失敗（`CampaignIdentity` の新欄）→ api-schema 葉で `UPDATE_SCHEMA=1` 再生成と web 生成型。
- 呼び出し側（`run_context.rs`・`lane_assist.rs`・`decision_log_wire.rs`・e2e）は `task_campaign_resolve` の名前を保ったので変更不要。ただし矛盾時に `Err` を返すようになったため、dispatcher 側での扱い（blocked＋理由）は dispatch 葉で決める。
- 未実施: `bash scripts/dev/test-parallel.sh`（上の 2 点で赤になる段のため統合段で）。

## 提案
- `task_campaign_set_tx` の「同 task への別識別は置換」を「拒否」に変えた。手で識別を差し替える運用は無い前提（API は resend で先に 409）。
