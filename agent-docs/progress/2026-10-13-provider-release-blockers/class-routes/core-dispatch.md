# execution class ごとの route 候補: core-dispatch

run `01M4TFXY754XRP0DFYC7EQAHNG`（unit `class-routes`）/ WorkUnit `core-dispatch`。親は provider 廃止リリース不具合 task（`01M4T94XWN`）。
ADR `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の 2026-10-13 付記 §5 に従う。

## 変更

- `crates/task-core/src/llm_route.rs`
  - `Registry.route_class_defaults: BTreeMap<HarnessId, BTreeMap<ExecutionClass, Vec<CandidateKey>>>`（`#[serde(default)]`）を追加。
  - `route_priority(registry, harness, cos, execution_class)` に execution class を渡す。順序は次のとおり。
    1. `cos` なら `cos_route_candidates`（class に関係なく）。
    2. `route_class_defaults[harness][class]` が宣言されていればその列で置換する（`route_defaults` へ fallback しない。空の列も宣言として扱い、候補が無ければ `NoDeclaredDefault`）。
    3. 無ければ `route_defaults[harness]`。
  - `RoutePolicy` / `DefaultRoutePolicy` / `default_route_policy` の形は変えていない。
- `crates/task-dispatch/src/dispatcher/route_select.rs`
  - harness loop の `one.harness.execution_class = None` を削除し、class を `route_select_one` の `route_priority` まで届ける。
  - harness 選択の rule 照合への影響: `route_select_one` は `one.harness.harness = Some(..)` なので `select_harness` は明示 harness で即返る。rule 照合は class に依存しない。選択と候補用 class を分ける必要は無かった。
  - module doc の順序の説明を更新。
- `crates/task-dispatch/src/dispatcher/tests/class_route.rs`: `override_registry`（claude_code・既定 sonnet・coding 候補 opus）で 4 試験を追加。
- `crates/task-dispatch/src/dispatcher/tests/route_select.rs`: `class_candidates_survive_the_harness_loop` を追加。
- `crates/celeris/src/config/llm_route.rs`: 変更なし。`Registry { harness_selection, ..Default::default() }` のため新欄は自動で空の表になる（TOML 欄は config の leaf）。

## 判断

- class 候補は harness の既定を置き換える。足すのではない。class 候補が全て不適格なら `NoEligible` で fail closed する（既定の sonnet は選ばれない）。
- CoS の経路は class を見ない。
- `route_class_defaults` は `serde(default)` にした。既存の config（欄が無い）はそのまま読める。

## 検証（この WU の範囲）

| 条件 | 実行 | 結果 |
|---|---|---|
| route_priority の class 優先・cos の無視 | `cargo test -p task-core --lib` | 911 passed, 0 failed（新規: `route_priority_prefers_the_execution_class_list`、`route_priority_ignores_the_class_for_cos`） |
| coding は opus、他 class と class なしは sonnet | `cargo test -p task-dispatch --lib -- class_route_override` | `class_route_override_selects_opus_for_coding_and_sonnet_for_other_classes` ok |
| coding の class 候補が先頭 | 同上 | `coding_class_picks_the_class_candidate_first` ok |
| class 候補が全て不適格なら harness 既定へ fallback しない | 同上 | `class_list_does_not_fall_back_to_the_harness_defaults` ok（NoEligible、拒否理由は opus のみ） |
| class が harness loop を通る | 同上 | `class_candidates_survive_the_harness_loop` ok |
| task-dispatch 全 lib | `cargo test -p task-dispatch --lib` | 846 passed, 0 failed |
| celeris config | `cargo test -p celeris --lib config::` | 137 passed, 0 failed |
| 整形 | `rustfmt --edition 2024 --check` (4 files) | exit 0 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 範囲 | `git diff --name-only $CELERIS_WU_BASE` | 4 files（`crates/task-core/src/llm_route.rs`、`crates/task-dispatch/src/dispatcher/route_select.rs`、`crates/task-dispatch/src/dispatcher/tests/class_route.rs`、`crates/task-dispatch/src/dispatcher/tests/route_select.rs`） |

## 未解決・残り（他 unit）

- TOML の `[route_defaults.<h>].classes` 欄・検証・registry 構築（config unit）。現時点では `route_class_defaults` は常に空で、本番の挙動は変わらない。
- `migration_runbook_toml_loads` の coding opus 期待値・runbook（runbook unit）。

## 提案

- `route_select` が失敗時に `RouteTrace.candidates` を空のまま返すため、拒否された class 候補の列を `llm_route_selected`・`worker_progress` の metadata で見にくい。失敗時も `candidates` を埋めるか、`execution_class` を metadata に入れると、どの列で落ちたか後から追える（metadata version は据え置きで加法的に足せる）。
