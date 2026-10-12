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

1. **決定的な admission 試験が無い（最大の残件）。** `execution_class`・`harness_candidates`・`harness_rejections` を使う試験は既存に無い（grep で確認。既存の試験は config の読み込みと、`execution_class_defaults = {}` を書くだけの fixture）。総合受け入れの「claude_oauth が埋まり codex_oauth が空いているとき planner・reviewer が codex で起動する」「両方埋まると defer する」は未検証。注入した `RouteState` で `route_select` を呼ぶ試験を追加する必要がある（LLM・外部 network は使わない）。この WU の objective には試験追加が無いため、ここに残した。
2. **不具合の疑い（直していない）。** `route_select` の失敗理由は「最後に試した harness」の理由になる。候補列の末尾の harness が `route_defaults` を持たない（`NoDeclaredDefault`）と、他の候補が単に埋まっているだけでも `unroutable`（設定変更待ち）として扱われ、defer にならない。`validate_registry`（`crates/celeris/src/config/llm_route.rs`）は execution class の候補が `route_defaults` を持つかを確かめていない。
3. 例は `pi-hashline` の 1 つだけに足した。`celeris.example.toml` には planner・review の例が無い。
4. `pi` を planner の 2 番目に置いたのは例示のため。本番で何を planner に使うかは人が決める。

## 提案

- 試験 WU を 1 つ足す（planner-route 段の後）: `route_select` に注入した `RouteState` で、(a) claude_code が埋まり codex が空き → codex 起動、(b) 両方埋まり → `NoEligible`（defer）、(c) review も同じ、(d) 宣言なし → 従来の genre・role の選択、の 4 件。
- `validate_registry` で execution class の候補 harness が `route_defaults` を持つことを読み込み時に確かめる（または `NoDeclaredDefault` を `NoEligible` と同じく待ち扱いにする）。どちらにするかは人の決定。
- 本番 config の書き換えは下の手順で人が行う。

## 本番への移行（人が実行する手順）

この WU は本番 host・本番 config・DB に触れていない。本番 config の内容は読んでいない。

1. 本番 config（`~/.config/celeris` 配下の celeris の設定ファイル。実際の名前は人が確認）をバックアップする。
2. `[execution.planner]` の `adapter = …` の行があれば削る。この欄が残ると新しい loader は読み込みエラーにする（互換の読み替えは無い）。
3. `[harness_selection]` に `execution_class_defaults = { planner = [...], review = [...] }` を書く。並びは優先順。書く harness ID は `[harnesses.<id>]` で宣言済みで、かつ `[route_defaults.<id>]` を持つこと（宣言されていない ID は読み込みエラー）。
4. 新しい config を読み込ませ、`invalid config` が出ないことを確かめる。
5. 通常の手順（selfdeploy の release・reload）で daemon に反映する。反映後、次に planner run が起きたら `llm_route_selected` の source・harness と、run の `selection_metadata_json` の `harness_rejections` を確かめる。
6. 失敗したらバックアップを戻し、旧 daemon を再起動する。
