---
tasks: [01M4R3V8FBDFEY3MCR91S3HR81]
status: running
title: planner・reviewer run の harness を execution class の候補列と admission で選ぶ（adapter 固定を消す）
updated: 2026-10-12
---

# planner-route: planner・reviewer の harness 選択

親: `agent-docs/progress/2026-10-11-provider-layer-redesign.md`。ADR: `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の「2026-10-12 付記（planner・reviewer の選択）」。

WU の内訳:
- `class-select`（0a3bf4c5）: planner・reviewer run に execution class（`planner`・`review`）を付け、`harness_candidates` の候補 harness を順に `route_select` の admission にかける。
- `adapter-purge`（64789ef9）: `[execution.planner] adapter`・`PlannerConfig.adapter`・`default_planner_adapter` を消し、書かれた config は読み込みエラーにする。
- `closeout`（この節）: ADR 付記、config 例、本進捗、全体検査。

## closeout の結果

1. ADR 付記（「2026-10-12 付記（planner・reviewer の選択）」）を追加した。class の付け方、候補列、admission の順、記録先（`harness_selected`・`llm_route_selected`・`runs.selection_metadata_json` の `harness_rejections`・`route refused`）、config、後続 01M4M8D4AQ の `RoutePolicy` 境界を、実装（`harness_candidates`・`route_select`・`dispatch_run.rs`・`review_spawn.rs`）を読んで書いた。
2. config 例 `config/celeris.pi-hashline.example.toml` の `[harness_selection].execution_class_defaults` に `planner = ["claude_code", "pi"]`、`review = ["claude_code"]` を書いた（従来は `{}`）。
3. 本進捗を書いた（この節と下の証拠・未解決・提案・本番への移行）。
4. 全体検査を流した（下の証拠）。コードの不具合は直していない。

## 証拠

作業ディレクトリは `repos/agent-platform`、ブランチ `celeris-wu/01M4R3V8FBDFEY3MCR91S3HR81/closeout`。

- `sh scripts/dev/check-doc-links.sh` — exit 0、`check-doc-links: ok`。
- `sh scripts/dev/check-adr-numbers.sh` — exit 0、`check-adr-numbers: ok (182 files)`。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0、`check-doc-layout: ok`。
- `cargo fmt --all -- --check` — exit 0（差分なし）。
- `bash scripts/dev/test-parallel.sh`（`TMPDIR=/local/x/ar`）— exit 0。`4897 tests run: 4897 passed, 13 skipped`。`CELERIS_TEST_SUMMARY`: `passed: 4900, failed: 0, ignored: 14, nextest_exit: 0, doctest_exit: 0, tmp_leftovers: 0`。
- `cargo clippy --workspace -- -D warnings` — exit 0。
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0（試験コードを含む）。
- 初回の `test-parallel.sh` は TMPDIR を `runs/…/tmp`（長いパス）にしたまま流したため exit 100（104 failed）。失敗は `path must be shorter than SUN_LEN`（unix socket の path 長）と 60 秒の timeout で、コードの不具合ではない。短い TMPDIR に換えて再実行し、上の結果になった。
- 読み込みの確認（一時的な試験。確認後に削除し、commit していない）:
  - `config/celeris.pi-hashline.example.toml` が `Config::load` で読める。`harness_selection.execution_class_defaults` は `planner = [claude_code, pi]`、`review = [claude_code]`。
  - 例の末尾に `[execution.planner] adapter = "claude-code"` を足すと `invalid config: [execution.planner] adapter is no longer supported; planner runs pick a harness from the candidates in [harness_selection.execution_class_defaults] …` で拒否される。

## 未解決

### 解決済み（closeout2）

1. **決定的な admission 試験**（解決）。`wu/class-tests`（db30c4fe）が `crates/task-dispatch/src/dispatcher/tests/class_route.rs` を足し、注入した `RouteState` で `route_select` を呼ぶ。
   - `planner_and_review_class_routes_skip_full_first_harness_and_defer_when_all_full` — planner と review の両方で、claude_oauth が埋まり codex_oauth が空き → `codex` が選ばれ、`harness_rejections[0]` に `claude_code` が残る。両方埋まり → `NoEligible`（defer）。
   - `undeclared_class_uses_only_normal_genre_selection` — class 未宣言のとき genre の規則だけで `claude_code` を選び、埋まっていれば `NoEligible`。
   - `an_ineligible_harness_wins_over_a_later_undeclared_default` — 末尾の候補（codex）が `route_defaults` を持たなくても、先頭の `NoEligible` で defer する（下の 2 も確かめる）。
   - 実行: `cargo test -p task-dispatch class_route` — exit 0、`3 passed; 0 failed`（3 件とも ok）。
2. **末尾候補の未宣言で defer しない不具合**（解決）。`route_select` は `NoEligible` を見つけたら、それを優先して返すように直した（`route_select.rs` の `saw_ineligible`）。`validate_registry` は execution class の候補が `route_defaults` を持つことを読み込み時に確かめる（`crates/celeris/src/config/llm_route.rs`）。
   - 実行: `cargo test -p celeris --lib execution_class` — exit 0、`execution_class_harness_requires_route_defaults ... ok`（1 passed）。
## 未解決

1. 例は `pi-hashline` の 1 つだけに足した。`celeris.example.toml` には planner・review の例が無い。
2. `pi` を planner の 2 番目に置いたのは例示のため。本番で何を planner に使うかは人が決める。

## 提案

- 本番 config の書き換えは下の手順で人が行う。
- `celeris.example.toml` にも planner・review の例を足すかは人が決める。

## closeout2 の証拠（試験と defer 規則の追記後）

作業ディレクトリは `repos/agent-platform`、`TMPDIR=/local/x/ar`。コードは変えていない（変更は ADR 付記と本進捗だけ）。

- `cargo test -p task-dispatch class_route` — exit 0、`3 passed; 0 failed`。
- `cargo test -p celeris --lib execution_class` — exit 0、`1 passed; 0 failed`。
- `bash scripts/dev/test-parallel.sh` — exit 0。`4901 tests run: 4901 passed, 13 skipped`。`CELERIS_TEST_SUMMARY`: `passed: 4904, failed: 0, ignored: 14, nextest_exit: 0, doctest_exit: 0, tmp_leftovers: 0`。
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0（試験コードを含む）。
- `cargo fmt --all -- --check` — exit 0。
- `sh scripts/dev/check-doc-links.sh` — exit 0、`check-doc-links: ok`。
- `sh scripts/dev/check-adr-numbers.sh` — exit 0、`check-adr-numbers: ok (182 files)`。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0、`check-doc-layout: ok`。
- `sh scripts/dev/progress-index.sh --check` — exit 0、`progress-index --check: ok`。

## 本番への移行（人が実行する手順）

この WU は本番 host・本番 config・DB に触れていない。本番 config の内容は読んでいない。

1. 本番 config（`~/.config/celeris` 配下の celeris の設定ファイル。実際の名前は人が確認）をバックアップする。
2. `[execution.planner]` の `adapter = …` の行があれば削る。この欄が残ると新しい loader は読み込みエラーにする（互換の読み替えは無い）。
3. `[harness_selection]` に `execution_class_defaults = { planner = [...], review = [...] }` を書く。並びは優先順。書く harness ID は `[harnesses.<id>]` で宣言済みで、かつ `[route_defaults.<id>]` を持つこと（宣言されていない ID、`route_defaults` を持たない ID、同じ class 内の重複は読み込みエラー）。
4. 新しい config を読み込ませ、`invalid config` が出ないことを確かめる。
5. 通常の手順（selfdeploy の release・reload）で daemon に反映する。反映後、次に planner run が起きたら `llm_route_selected` の source・harness と、run の `selection_metadata_json` の `harness_rejections` を確かめる。
6. 失敗したらバックアップを戻し、旧 daemon を再起動する。
