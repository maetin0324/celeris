---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: tier-drop-operator
updated: 2026-10-12
---
# tier-drop-operator

## 状況
ADR 2026-10-11 llm-source-harness-selection の残件だった tier→model の束縛を撤去した。dispatcher の `select_route` が選んだ source×model を `route_adapter`（dispatcher 側・worker 側とも）が `with_model` で harness に載せる経路だけが残る。lane（Tier）は品質／予算の lane としてだけ使い、model を引かない。

調べた結果、束縛を本番で組み立てる箇所は無かった（`TieredAdapter` を作るのは試験だけ、`apply_to_bindings` の呼び出しも試験だけ）。本番で tier→model の対応を要するものは残っていない。

消したもの:
- `crates/task-worker/src/tiered.rs`（`TieredAdapter`）と `pub mod tiered`。
- `WorkerAdapter` の `model_for_tier`・`reasoning_effort_for_tier`・tier 別束縛の差し替え口・`supports_reasoning_effort`・`with_reasoning_effort`、`BrowserSpecialistAdapter` の同じ中継。
- codex の reasoning effort 経路（`CodexConfig.reasoning_effort`、`-c model_reasoning_effort=…` の付与、`with_reasoning_effort` の実装）。値を入れる経路は tier 束縛だけで、daemon は常に `None` を入れていた。
- task-core `model_routing` の `ModelBinding`・束縛 map 型・`resolve`・`reasoning_effort`。
- task-core `model_catalog::assignments` の `apply_to_bindings`・`configured_wire`（`WireRule` は llm-proxy・config の catalog 重ね合わせが使うので残す）。
- 上記の試験: `with_model_pins_the_base_model_and_clears_tier_bindings`、束縛差し替えの試験、`tier_resolution_never_substitutes_a_missing_or_disabled_model`、`reasoning_effort_is_optional_and_old_bindings_still_parse`、assignments の `apply_*` 4 件、codex の effort 試験 3 件、claude-code の effort 不通過試験、dispatcher 試験の `three_lane_adapter`。

書き換えたもの:
- dispatcher の routing 監査（`LaneResolution.reasoning_effort`）は `None` を書く（route は effort を持たない）。欄自体は旧 event を読むため残す。
- codex・claude-code の「tier 束縛が `--model` に届き account env を保つ」試験を、`with_model` → `with_env` の具体 model 経路の試験（`route_model_reaches_cli_model_argument_and_preserves_account_env`）にした。
- langmem の鍵試験は tier の繰り返しと `TieredAdapter` の包みを外し、`LangMemAdapter` を直接使う。
- routing context 試験は `InstantAdapter` を直接渡す。
- 関係する doc comment（`worker_task.rs`・`acp.rs`・`model_policy.rs`・config の `model_catalog.rs`・assignments の module doc）を route の model 前提に直した。
- HEAD が `cargo fmt --check` を通らなかった（worker の置換で `WorkerHint { tier, <空行> }` などが残っていた）ため、`cargo fmt --all` の整形だけの差分が 100 余りの試験ファイル等に入っている。

## 検査
- 受け入れ grep `! grep -rnE 'ProviderConfig|ProviderSpec|provider_select|providers_dir|tier_models|in_use_cos|SourceOrigin|worker_hint\.adapter' crates --include=*.rs`: exit 0（0 件）。`TierModels|ModelBinding|model_for_tier|TieredAdapter|apply_to_bindings|with_reasoning_effort` も crates 内 0 件。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0。4885 tests run: 4885 passed, 13 skipped。
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `cargo test -p task-core -p task-api -p task-worker schema_matches`: 10 passed、0 failed。schema の変更なし（`ModelBinding` は API・worker protocol の schema に出ていなかった）。

## 未解決
- `web/api/generated/schema.json` に旧 provider 表示の `tier_models` / `ModelBinding` が残る。Rust の schema 試験の対象外の生成物で、画面側（ui-v2）の型追随で消す。

## 提案
- reasoning effort を使いたい場合は、harness の model 選択と同じく selected route（`SelectedLlmRoute`）に effort を持たせ、`route_adapter` で harness に渡す形にする。lane から引く形には戻さない。必要になった時点で ADR に付記して実装する。

## 本番への移行
- config の変更は無い（束縛を作る config 欄は前段で既に消えている）。
- 挙動の差: codex の run に `-c model_reasoning_effort` が付かなくなる。本番 daemon は既に effort を入れていなかったので、実際の argv は変わらない。routing 監査の `reasoning_effort` は新しい run では空。
