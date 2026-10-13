---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
status: done
title: close5 scripts — live fixture の [[providers]] を現行の source/harness 形へ
completed: 2026-10-13
---

# close5 / scripts: 非 cargo の live fixture を現行の config 形へ

親: `agent-docs/progress/2026-10-11-provider-layer-redesign.md`（未解決事項「非 cargo fixture を現行 source/harness config に移す」）。形の正本は `docs/ops/provider-llm-source-migration.md`「移行後の TOML」と、`crates/celeris/src/config/tests.rs` の fixture（`[llm_sources.<id>]`・`[harnesses.<id>]`・`[route_defaults.<id>]`・`[harness_selection]`）。

## 書き換えたもの

| script | 変えたこと |
|---|---|
| `scripts/dev/cos-chat-live.sh` | `[[providers]] claude-live` を `[llm_sources.claude_oauth]`・`[harnesses.claude_code]`・`[route_defaults.claude_code]`・`[harness_selection]` に置き換えた。`concurrency = 4` は top-level の `max_concurrency = 4` へ（コメントは残した）。`[cos]` の `llm_source`・`tier` を消し、`route_candidates = [{ source = "claude_oauth", model = "claude-opus-5-5" }]` にした。 |
| `scripts/dev/cos-chat-bench.sh` | 同じ置き換え（`[cos]` の `route_candidates` は claude-opus-5-5）。`POOL_CONFIG`・`COS_ACCOUNT` を `ACCOUNT_POOL` の 1 本にし、account を使うときだけ末尾に `[accounts] claude_dir` を足す（`[cos] account_id` は現行の `CosConfig`（`deny_unknown_fields`）に無い）。**第 5 引数 `codex` を外した**（下の「挙動の変更」）。usage・冒頭コメントも直した。 |
| `scripts/live-check-phase53.sh` | `[[providers]] fake-local` を `[llm_sources.fake]`・`[harnesses.fake]`・`[route_defaults.fake]`・`[harness_selection]` に置き換えた。`concurrency = 2` は top-level の `max_concurrency = 2`（既にあった行）に一本化して落とした。`[adapters.fake]` と `[reviewer] tier` は残した。 |
| `scripts/selfdeploy/tests/migrate_to_local_test.sh` | 移行台本の試験 config の `[[providers]] x` を `[llm_sources.x]`・`[harnesses.x]`（fake）に置き換えた。 |
| `scripts/test_portable_providers.py` | 試験 fixture の `[[providers]] claude`（`env = {SECRET…}`）を `[llm_sources.claude]`（`accounts = ["must-stay-private"]`）に替えた。秘密値が変更の要約に出ないことの確認は同じ。 |

## 残したもの（消していない）

- `scripts/portable-providers.py`（と対の `test_portable_providers.py`）は**消さなかった**。理由: この script が書き換える `[[harnesses]] adapter = …` と `[[roles]] adapter = …` は、現行の config が今も読む欄である。`crates/celeris/src/config/harness.rs` の `HarnessConfig.adapter` は「省略時は tier だけで選ぶ」固定 adapter の欄として生きていて、pin を外すと routing が変わる。旧形を検査するだけの script ではないので、消す条件に当たらない。書き換えたのは試験 fixture の `[[providers]]` だけ。
- `docs/` の旧形の説明・移行 runbook は docs WU の範囲。ここでは触っていない。

## 挙動の変更（人が確かめる点）

- `cos-chat-bench.sh` の harness 引数は `claude-code` だけになった。現行の config は `[cos] harness` を `claude-code` に固定する（`validate_cos_mapping`）ので、`codex` を CoS に流す形は読み込みエラーになる。codex の CoS 計測は元々未計測（ADR 2026-10-08 の付記）。codex の worker 計測が必要なら別の台本として切る。
- `[cos] account_id` の指定は消えた。account は `[accounts] claude_dir` の pool から dispatch で選ばれる（`crates/celeris/src/config/cos.rs` の「source/model/account are selected at dispatch」）。`same-thread`・`answers` は隔離 dir に account を 1 つだけ置くので、pool がそれを選ぶ。

## 検査の結果

- `grep -rn '\[\[providers\]\]\|providers_include' scripts` → 出力なし、exit 1（0 件）。受け入れ 0。
- `bash scripts/selfdeploy/tests/migrate_to_local_test.sh` → exit 0。末尾の行は `migrate_to_local_test: all ok`（`ok: switch-fail-swap …` まで全件 ok）。
- `bash -n` → `scripts/dev/cos-chat-live.sh`・`scripts/dev/cos-chat-bench.sh`・`scripts/live-check-phase53.sh`・`scripts/selfdeploy/tests/migrate_to_local_test.sh` すべて exit 0。
- `python3 -I scripts/test_portable_providers.py` → `Ran 3 tests … OK`（exit 0）。`python3 -I -m py_compile scripts/portable-providers.py` → exit 0（生成された `__pycache__` は消した）。
- 3 本の heredoc config（`cos-chat-live.sh`・`cos-chat-bench.sh`・`live-check-phase53.sh`）を取り出し、token 等の変数を埋めて現行の loader に通した（`cargo run -q -p celerisctl -- routing show --config <file>`）→ 3 本とも exit 0。`claude_code` の route_defaults は `claude_oauth/claude-opus-5-5`、`fake` は `fake/fake`。
- 前回の書き換えは `[harness_selection]` に `rules = [{ harness = … }]` だけを書いていた。`HarnessRule.required_capabilities` と `HarnessSelection.execution_class_defaults` には serde の default が無く、この形は読み込みで落ちる。`crates/celeris/tests/integration_api.rs` と `instance_handoff.rs` の実例に合わせ、`execution_class_defaults = {}` を足し、rule に `required_capabilities = []` を足した（3 本とも）。loader で確かめたのはこの修正後の形。

## 未検証・やらなかったこと

- `[[llm_routes]]`・`[harness_selection]` の rule 数などの細部は、上の loader 通過で見たものだけ。celeris の daemon 起動（live）は見ていない（LLM を呼ぶため）。
- live script（`cos-chat-live.sh`・`cos-chat-bench.sh`・`live-check-phase53.sh`）は実行していない（LLM・外部ネットワークを呼ぶため。`bash -n` のみ）。
- `docs/ops/opencode-go-and-model-catalog.md` §3a の `POST /api/v1/providers` は docs WU の担当。
