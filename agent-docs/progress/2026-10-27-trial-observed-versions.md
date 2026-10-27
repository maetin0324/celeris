---
title: 試行ごとに実際に動いた版を記録し、宣言値と違えば拒否する（main-4 契約 U4・U11）
tasks: [01M5XXF7QAXTD8E7MM6DF25JRG]
status: done
updated: 2026-10-27
---
# 試行ごとに実際に動いた版を記録し、宣言値と違えば拒否する（main-4 契約 U4・U11）

完了日: 2026-10-27

判断は [ADR 2026-10-27-trial-observed-versions](../adr/2026-10-27-trial-observed-versions.md)。

## 変更

- `crates/task-core/src/pricing.rs`: `PRICE_TABLE_VERSION = "2026-09-approx-1"`（単価表の版 ID）。
- `crates/task-api`: `GET /health` に `pricing_version`・`cos_policy_version`（`ApiSettings.cos_policy_version`、daemon は
  CoS 有効時に `[cos.triage] policy_version`）。`docs/api/v1/api-v1.schema.json`・`api.md` §3.1・`web/api/generated/*` を再生成。
- `crates/campaign`:
  - `manifest.rs`: 省略可の `contract = {revision: "main-4-ops-r1", pinned_versions: {arm: {claude_code_version, claude_code_sha256,
    pricing_version, cos_policy_version}}}` と検査（未知の revision・有効 arm の欠落・B/C の pricing_version 欠落・A の daemon 欄を拒否）。
  - `versions.rs`（新規）: `ObservedVersions` と宣言との照合（`mismatches`・`verify`）。
  - `executor.rs`・`celeris_exec.rs`・`harness_exec.rs`・`dispatch.rs`: `Executor::observe_versions`。B/C は `GET /api/v1/health` と
    `--worker-program claude=<path>` の実体（sha256・`--version`）、A は起動する Claude Code の実体。
  - `runner.rs`・`ledger.rs`: 契約 revision の試行は routing・投入の前に観測・照合し、不一致は `executor_error`（`version_mismatch: …`）。
    観測は台帳の planned 行（`plan_with`）に載せ、再開時は観測し直さない。`record.rs` の `TrialRecord.observed_versions`。
  - `checker.rs`: 契約 revision の実行済み試行に観測が無い・宣言と違えば違反。
  - `main.rs`: `--worker-program claude=<path>` を `CelerisExecutor::with_claude_code` に渡す。
- 文書: `docs/ops/mlsys-main-4-ab-contract.md` §10 U4・U11 に解消先（`main-4-ops-r1`）を追記。`judged-scope-v2` の条件は変えない。

## 試験（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

`crates/campaign/tests/celeris_exec.rs`（偽 daemon `Fake` が `GET /api/v1/health` に答える）:
- `ops_r1_records_observed_versions_from_the_fake_daemon_and_binary`: 一致なら B/C を投入し、記録に release・単価表・Claude Code の
  path/sha256/版。A は Claude Code だけ。書き換えた記録は checker が `observed_versions release` で落とす。
- `ops_r1_rejects_release_mismatch_before_submitting`: release 不一致で B/C は POST 0 件・`executor_error`・`version_mismatch: release …`。
- `ops_r1_rejects_a_replaced_claude_code_binary_and_pricing_mismatch`: binary の差し替え（sha256）と単価表の版の不一致を 3 腕とも拒否。
- `ops_r1_rejects_when_the_daemon_does_not_report_versions`: `/health` が取れなければ拒否。
- `ops_r1_resume_keeps_the_start_observation`: 投入後に daemon が変わっても開始時の観測で記録する。
- `manifest_without_contract_keeps_pinned_bundle_behaviour`: `contract` の無い manifest は `/health` を読まず、記録・hash が変わらない。
- `contract_declaration_is_validated`。

検査コマンドの結果は task の完了報告に書く（提出時の HEAD で実行）。

## 本番への移行

- 本番 config の変更は無い。daemon を新しい release にすると `GET /health` に 2 欄が増える（追加のみ）。
- `main-4-ops-r1` で測るときは manifest に `contract` を書き、`run --executor celeris` に `--worker-program claude=<測定 daemon の
  [adapters.claude_code] command と同じ実体>` を渡す。

## 未解決事項

- 外部 operator の方針の版は daemon から観測できず、照合していない（human-ops の `policy_ref` に残る）。
- `main-4-ops-r1` の評価条件の文書（A9、task 01M5XXF7QDVASQFQBT1GZBXS6V）はまだ main に無い。revision 名は A9 の objective の例
  `main-4-ops-r1` に合わせた。A9 が別名にするなら `CONTRACT_REVISION_OPS_R1` を同じ変更で直す。
- 単価表の cache write（5 分・1 時間）の区別は未対応（U11 の残り）。

## 提案

- B の `--worker-program claude=` と測定 config の `[adapters.claude_code] command` の一致を preflight で検査する（現在は運用者が揃える）。
