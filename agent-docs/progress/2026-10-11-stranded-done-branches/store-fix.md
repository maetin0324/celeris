---
tasks: [01M4MAZNNYT4JVRYKCVZ1TF1VF]
---
# store-fix

## 原因

state-store の葉が `task_core::Event` に `IntegrationDetected` / `IntegrationMerged` / `IntegrationDiscarded` / `IntegrationHeld` / `IntegrationReopened` を足したが、`crates/task-api/src/query.rs` の event kind 名の網羅 match（`event_type_name`）に腕が無く、E0004 で task-api とその依存 crate が compile できなかった。

## 直した箇所

- `crates/task-api/src/query.rs`
  - `event_type_name` に 5 variant の腕を足した（serde の tag 名と一致: `integration_detected` / `integration_merged` / `integration_discarded` / `integration_held` / `integration_reopened`）。
  - `EVENT_TYPES`（`types` クエリの語彙）に同じ 5 語を足し、配列長を 76 → 81 にした。
- `crates/task-api/src/query/tests.rs`
  - `EVENT_TYPES.len()` の期待値を 76 → 81 に更新（2 箇所）。
  - `integration_state_event_types_match_their_serde_names` を追加（5 variant の serde 名・`event_type_name`・`EVENT_TYPES` の一致）。
- `docs/api/v1/api-v1.schema.json`・`web/api/generated/schema.json`・`web/api/generated/types.ts`・`gui/app/celeris/types.ts`: `UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated` と gui/web の `gen:types` で再生成（差分は 5 variant の追加だけ）。
- `web/api/realtime/event-kinds.ts`: `EVENT_KINDS` に 5 語を追加（`satisfies EventKind` と網羅型が通すため）。
- `web/api/realtime/invalidation-map.ts`: 5 種の invalidation（`{ sets: ["T", "L"] }`）を追加（`Record<EventKind, KindSpec>` が網羅を要求するため）。

網羅 match は `cargo check --workspace --all-targets` の再実行で他に E0004 が出ないことを確認した（ワイルドカード腕は足していない）。挙動の追加はしていない（kind 名と表示・invalidation の対応だけ）。

## 実行したコマンドと結果

- `cargo check --workspace --all-targets` → exit 0
- `cargo test -p task-api` → 修正前は `schema::tests::committed_schema_matches_generated` だけが drift で失敗（88 passed / 1 failed）。再生成後 `UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated` → exit 0
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、`corepack pnpm@12.6.0 -C web gen:types` → exit 0（先に `pnpm install --offline` 両方 exit 0）
- `corepack pnpm@12.6.0 exec tsc -b`（web）→ exit 0、`corepack pnpm@11.27.0 typecheck`（gui）→ exit 0
- `corepack pnpm@12.6.0 exec vitest run api/realtime` → 3 files / 61 tests passed
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（`Finished dev profile`）

## test-parallel

- `bash scripts/dev/test-parallel.sh`（既定の TMPDIR は 93 文字で、nextest の unix socket path が SUN_LEN を超え 108 件失敗）: `5130 tests run: 5022 passed, 108 failed, 13 skipped`。失敗は全て `path must be shorter than SUN_LEN`（browser_doctor・broker・injection）と broker 待ちのタイムアウトで、store 変更とは無関係の環境要因。
- `TMPDIR=/local/celeris/data/scratch/sf bash scripts/dev/test-parallel.sh`（短い TMPDIR）: `5130 tests run: 5098 passed, 32 failed, 13 skipped`（`CELERIS_TEST_SUMMARY` の passed 5101・failed 32、nextest_exit 100、doctest_exit 0）。
  - 失敗 32 件の内訳はすべて既知の sandbox 失敗の class: `celeris-credentiald::injection_ipc`（20 件、60 秒の readiness timeout）・`celeris-credentiald::broker`（2）・`celeris-credentiald::browser_doctor`（1）・`task-api::browser_e2e`（4）・`task-api::browser_waits`（1）・`task-worker::browser::launcher_run::*`（4、うち launcher_login の 4 件は timeout）。
  - task-api の失敗は browser_e2e と browser_waits の broker 登録系だけで、events・query（store 変更の範囲）に関わる試験は全て通っている。
  - 失敗が base（HEAD 22b47c46）でも起きるかの個別確認は行っていない。2026-10-08 の記録でも同じ class（browser・launcher・credentiald・CDP・scratch の sandbox 既知失敗）が出ている。

## 未解決事項

- 上の 32 件の base 比較は未実施（sandbox で broker・launcher を動かせないため）。人か別の環境での再確認が要る。
- 次の unit（sweep-core）で event の発行元を増やすとき、`EVENT_TYPES` の長さ試験（81）を更新する。

## 再検証（前回の check 不合格の後）

- 前回の check の不合格: `out of scope: web/api/realtime/event-kinds.ts・invalidation-map.ts`。この 2 file は `Record<EventKind, KindSpec>` の網羅と `satisfies EventKind` のため web の型検査に必要な変更で、取り除くと web が compile できない。scope の許可（`crates/`・`docs/api/v1/`・`docs/protocol/`・`gui/app/celeris/types.ts`・`web/api/generated/`・本進捗 dir）に `web/api/realtime/` が無いことが原因。作業は直さず、計画の check を直す必要がある（plan_issue）。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（Finished）
- `cargo test -p task-api` → exit 101。lib 89 件・他の unit/integration は pass。失敗は `tests/browser_e2e.rs` の 4 件（`timed out waiting for fixture readiness`、broker の socket 待ち、既知の sandbox 失敗）。`query::tests::integration_state_event_types_match_their_serde_names` と `schema::tests::committed_schema_matches_generated` は pass。
- test-parallel は前回の run の結果を引き継ぐ（コードは変えていない）。

## 最終検証（範囲 check を web/api/realtime/ まで許可した後の run）

- 範囲 check（`crates/`・`docs/api/v1/`・`docs/protocol/`・`gui/app/celeris/types.ts`・`web/api/generated/`・`web/api/realtime/`・本進捗 dir の外に path が無い）→ `scope ok`
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（`Finished dev profile`）
- `cargo test -p task-api` → exit 101。lib 89・他の suite は pass。`tests/browser_e2e.rs` の 4 件と `browser_waits` が broker の fixture readiness 待ち timeout（既知の sandbox 失敗）。`query::tests::integration_state_event_types_match_their_serde_names` は pass。
- `TMPDIR=/local/celeris/data/scratch/sf bash scripts/dev/test-parallel.sh` → exit 100（nextest 100、doctest 0）。`5130 tests run: 5098 passed, 32 failed, 13 skipped`（summary の passed 5101、failed 32）。
  - 失敗 32 件の内訳: `celeris-credentiald::injection_ipc` 17、`task-worker::browser::launcher_run::*`（launcher_login 含む）7、`celeris-credentiald::broker` 2、`celeris-credentiald::browser_doctor` 1、`task-api::browser_e2e` 4、`task-api::browser_waits` 1。すべて browser・launcher・credentiald・broker の sandbox 失敗（socket 待ち timeout）。store・events・query の試験は失敗していない。
  - store 変更が原因でないことの裏取り（base 22b47c46 での同じ試験の比較）は未実施。sandbox で broker・launcher を動かせないため、人か別環境での再確認が要る。
- 取り残しの検出・受信箱・merge/discard/hold の本体（sweep・API・celerisctl）は後続 unit の担当で、本 unit では kind 名と生成型だけを扱った。
