---
title: dispatcher/daemon への coding 既定ハーネス配線と統合試験
tasks: [01M49X6CW5KE5HY0EZRQR0S045]
status: done
updated: 2026-10-07
---
# dispatcher/daemon への coding 既定ハーネス配線（WorkUnit dispatch）

ADR `agent-docs/adr/2026-10-07-coding-harness-default-pi-hashline.md` の「後続葉 dispatch」。完了日 2026-10-07。

## 実装
- **有効化スイッチ**: `HarnessSpec.adapter_policy`（task-core、既定 `provider_order`）と `[[harnesses]] adapter_policy = "model_family"`（celeris config）。固定 `adapter`・`conversation = true` との併用は設定検証で拒否。書かなければ挙動は今と同じ。
- **pi の config/daemon 配線**: pi-adapter 葉が差し戻した `a8b4f1e5` の `crates/celeris`・`crates/celerisctl` 差分（provider 行の `extensions`/`tools`、adapter 許可一覧、`build_adapters` の `pi`）を取り込んだ。
- **dispatcher**（`crates/task-dispatch/src/dispatcher/coding_default.rs`）:
  - `CodingHarnessDefault { harnesses, sources, model_families }` を daemon が `Config::coding_harness_default()` から作り、`set_coding_harness_default` で渡す（bootstrap と設定の再読込の両方）。
  - 行の family は `task_core::derive_family`（LLM source → pool の adapter（pool を使う行だけ）→ catalog の `ModelProfile.family`）。`legacy_provider_profiles` の `ModelProfile.family` に catalog の family を入れた（catalog の穴埋め `unknown` は不明扱い）。provider 名・model id の文字列で判定しない。
  - `select_provider_with_default`: 対象（harness が model_family・adapter 明示なし・CoS でも planner でもない）なら、`prefers(行の adapter, family)` でない行を除外集合に足して `select_provider_excluding` を呼ぶ（sticky・cheap local・pool score・設定順の順序はそのまま）。選べなければ絞る前の候補で同じ関数をやり直す（fallback）。preferred 段の「選べない」記録（`unroutable`）は残さない。
  - `StaticPolicy::matches`・retry/cooldown・account pool の採点・enforce の除外は変えていない。
- **監査**: `LaneResolution.coding_default: Option<CodingDefaultResolution { family, family_basis, adapter_choice }>` を足し、`RoutingAudit.coding_default` に写す。`adapter_choice` は `preferred` / `fallback{reason}` / `explicit`（明示 adapter の run は選んだ行の family 付き）。対象外ハーネスの run には出さない。実際の adapter（`pi` を含む）は従来どおり `WorkerStarted.adapter` → `RoutingAudit.adapter`。
- execution metrics（task 単位）には足していない（ADR「metrics 整合」どおり）。
- schema: `UPDATE_SCHEMA=1`（docs/api/v1 の 2 file）、`node web/scripts/gen-types.mjs`、`gui` で `pnpm gen:types` を再生成。
- sessions.rs の `SUPPORTED_ADAPTERS` は変えていない（pi の session resume は未実装のため。ADR どおり）。sticky session は CoS の run だけで、CoS は既定解決の対象外。

## 試験（名前に `coding_harness_default`）
- task-dispatch（12）: `claude_row_prefers_claude_code`、`gpt_openai_prefers_pi`、`opencode_go_prefers_pi`、`deepseek_prefers_pi`、`unknown_provider_prefers_pi`、`falls_back_when_pi_unavailable`（満杯・cooldown・pi 行なし）、`provider_order_policy_unchanged`、`cheap_local_qwen_pi_first`、`pi_adapter_id_matches_worker`、`records_adapter_in_metrics`（tick → `WorkerStarted.adapter == "pi"` → routing audit）、`explicit_adapter_wins`、`other_harness_unchanged`。
- celeris（2）: `coding_harness_default_config_builds_dispatcher_inputs`、`coding_harness_default_config_rejects_fixed_adapter_and_unknown_policy`。

## 証拠
- `cargo nextest run -p task-dispatch -E 'test(/coding_harness_default/)'` → 12 passed。
- `cargo nextest run -p celeris -E 'test(/coding_harness_default|pi_adapter/)'` → 5 passed。
- `bash scripts/dev/test-parallel.sh` → exit 0、4225 passed / 0 failed / 13 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0（`--all-targets` でも警告なし）。`cargo fmt --all --check` → exit 0。

## 未解決事項
- 本番の有効化（`[[harnesses]] id = "coding"` に `adapter_policy = "model_family"`、pi 行の追加、Hashline の導入）は人の手順（ops-docs 葉）。
- llm-proxy 経由（`celeris` source）の行は family Unknown → pi が既定（ADR の表どおり）。
- gui/web の typecheck・e2e は流していない（生成型は optional 欄の追加だけ）。close-out で確認する。

## 提案
- `ModelFamily::parse` が catalog の穴埋め値 `"unknown"` を `Unknown` と読むようにすれば、config 側の除外（`crates/celeris/src/config/dispatch.rs`）が要らなくなる。

## 範囲 check の不合格（attempt 1, 2026-10-07）
- 範囲 check が `crates/celerisctl/src/commands/worker_tests.rs`（`ProviderConfig` の struct literal 5 か所に `extensions`/`tools` を追加）と `crates/task-ops/src/routing_outcome/tests.rs`（`LaneResolution` の literal に `coding_default: None`）を範囲外として落とした。
- どちらも ADR が定めた欄（provider 行の `extensions`/`tools`、監査の `coding_default`）を struct に足すと、全欄を列挙する既存試験の literal が compile できなくなるための機械的な 1 行修正で、外すと `cargo test --workspace` が build で落ちる。pi-adapter 葉が同じ理由で celeris/celerisctl 配線を差し戻した経緯と同じ。
- plan_issue として申告: dispatch 葉の範囲 check の許可 pattern に `crates/celerisctl/src/commands/worker_tests.rs` と `crates/task-ops/src/routing_outcome/tests.rs` を加える（新 key の葉で本 branch tip を ff 取り込みさせる）。
