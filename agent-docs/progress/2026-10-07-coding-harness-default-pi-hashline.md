---
title: coding の既定ハーネス（Claude 系は Claude Code、非 Claude 系は Pi + Hashline）
tasks: [01M49X6CW5KE5HY0EZRQR0S045]
status: done
updated: 2026-10-07
---
# coding の既定ハーネス（Claude 系は Claude Code、非 Claude 系は Pi + Hashline）

ADR `agent-docs/adr/2026-10-07-coding-harness-default-pi-hashline.md`。完了日 2026-10-07（close-out 葉）。
leaf 別の進捗は `2026-10-07-coding-harness-default-pi-hashline/`（survey / adr / family / pi-adapter / dispatch / ops-docs）。

## 成果

- Claude 系 model の coding run は既定で `claude-code` 行、非 Claude（GPT/OpenAI・OpenCode Go・DeepSeek・未知の provider）は `pi` 行を優先する。`[[harnesses]] id = "coding"` に `adapter_policy = "model_family"` を書いて有効化（書かなければ挙動は従来どおり）。
- Claude family の判定は `LlmSourceRef` → account pool の `AccountAdapter` → routing catalog の `ModelProfile.family`（`ModelFamily::parse`）。provider 名や model id の文字列直書きは使わない。未知・不明は非 Claude。
- Pi + Hashline は ACP ではなく新 adapter `pi`（`--mode json -p`、1 run = 1 process）。Hashline は extension の path として人が config に指定（`--no-extensions` + `-e` のみ）、tool は allowlist のみで subagent/planner 名は config 検証が拒否。
- 明示 adapter は従来どおり最優先。fallback / retry / cooldown / account pool / cheap lane の Qwen 優先は変更なし（preferred が全滅したら従来の候補へ倒れる）。
- run 単位の routing 監査に `coding_default { family, family_basis, adapter_choice }`（`RoutingAudit.coding_default`）を足し、(model, harness) 別での success / token / cache-hit / cost の比較に使う（見方は `docs/ops/coding-harness-pi-hashline.md` §5）。execution metrics（task 単位）には足していない（ADR「metrics 整合」）。

## 証拠（統合後の branch tip `d0bf8b63` で実行、2026-10-07）

- `corepack pnpm install --offline` → exit 0（Already up to date、pnpm v12.6.0）。
- `bash scripts/dev/test-parallel.sh` → exit 0（nextest 4225 passed / 0 failed / 13 skipped + doctest。summary は ignored 14、140 binaries）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all --check` → 差分なし。
- `git diff --check` → exit 0。`git diff --quiet "$CELERIS_WU_BASE" -- crates/ gui/` → exit 0（対象外差分なし）。
- 試験: `coding_harness_default`（task-core 10 / task-dispatch 12 / celeris 2）、`pi_adapter`（task-worker 14）。必須 6 ケース（Claude→claude-code、GPT/OpenAI→pi、OpenCode Go→pi、DeepSeek→pi、未知→pi、明示指定の尊重）と回帰（provider_order 不変・other harness 不変・fallback）を検証。
- 生成型の typecheck（worktree で `pnpm install --offline` の後）:
  - web: `corepack pnpm@12.6.0 -C web typecheck`（`tsc -b`）→ exit 0。
  - gui: `corepack pnpm@11.27.0 typecheck`（gui を cwd に、`react-router typegen && tsc -b`）→ exit 0。
  - 生成型の変更は `LaneResolution.coding_default`（optional）の追加（`docs/api/v1` 2 file、`gui/app/celeris/types.ts`、`web/api/generated/`）。

## 未解決事項（人の手順）

- 本番 host への pi / Hashline の導入と config の追記（`adapter_policy = "model_family"`、pi 行）は人が `docs/ops/coding-harness-pi-hashline.md` の手順で実行する。agent は本番 host を変えない。
- 実 Pi + 実 LLM での動作確認（routing 監査で `adapter = pi`・`adapter_choice = preferred` になること、Claude 系が `claude-code` のままになること）は導入後の確認として手順書 §6 にある。
- Pi の session resume（`SUPPORTED_ADAPTERS` に `pi`）は未実装（ADR 既定どおり。それまで Pi の run は毎回新しい session）。
- codex の OAuth を Pi に渡す経路は無い（pi 行の codex pool は config で拒否。codex 行は fallback で従来どおり使う）。

## 提案

- catalog の family の穴埋め値 `"unknown"` を `ModelFamily::parse` が `Unknown` と読むようにすると、`Config::coding_harness_default()` 側の除外（`crates/celeris/src/config/dispatch.rs`）が不要になる。
- (model, harness) 別の task 単位集計が必要になったら `GET /metrics/execution` の `group_by=adapter` を別 ADR で決める（最後の実行 run の adapter で数える規約が必要）。
