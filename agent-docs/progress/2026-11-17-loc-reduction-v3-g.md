---
title: LoC 削減 v3 G — 試験の整理と最終評価（production 累計純減 11,239 行、size check exit 0）
tasks: [01M7J3NKVEDX67N19WBRME0CRN]
status: done
updated: 2026-11-17
completed: 2026-11-17
---
# LoC 削減 v3 G — 試験の整理と最終評価

完了日: 2026-11-17

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 G。依存は D（01M7J3NKV8E6P9WAWGMJR3KB1F）・E（01M7J3NKVAH9NG53C5FEFNQJYK）・F（01M7J3NKVC2KQ4NGJ4GESHJV0B）。
最終評価の本文（固定計測・各 class・V0 との比較・減った実装の一覧・無関係な main の変更の区別・feature coverage）は run の成果物 `artifacts/final-report.md`（run 01M7MH0TQZ7YC62B28PTTNN131）にある。

## 結論
- 開始時 main `415a19a7004cb6a73b872e861fa8d1cf634dddbb` から提出時の HEAD までに、production 全言語は **11,239 行**、production Rust は **9,815 行**、own_total_with_tests は **17,802 行** 減った。受け入れの size check（`0 1 10000 1 1`）は exit 0。最終目標（累計 10,000 行以上・Rust も純減・own_total_with_tests も純減）を満たす。
- 内訳は C −2,701・D −2,558・E −1,753・F −4,224・G −3 で、和は固定計測と一致する（差 0）。同じ期間に main に入った無関係の変更 01M7KATBGC（health-policy-allowlist-shrink）は production 0 行、own_total_with_tests +261 行。
- V0 の計測点（健全性 v2 の完了時、production 386,112）と比べて −11,013 行。健全性 v2 の前（B0 の基準、366,177）と比べるとまだ +8,922 行。20〜30 万行の中長期の目標には届いていない。
- G の変更は重複した試験の統合だけ（production −3・own_total_with_tests −180）。統合した 4 組ごとに、残る試験が同じ不具合を検出することを失敗注入で確かめた。

## 測定（受け入れの size check と同じ command。分類規則は開始時 main の source-size-report.py・toml に固定。提出時の HEAD で実行）
| 指標 | unit（`git merge-base HEAD main` → HEAD） | fixed（`415a19a7004c` → HEAD） |
|---|---:|---:|
| production 全言語の純減 | 3 | 11,239 |
| production Rust の純減 | 3 | 9,815 |
| own_total_with_tests の純減 | 180 | 17,802 |

HEAD の class: production 375,099・external_test 455,363・inline_test 21,408・generated 43,822・vendor 111,936・fixture 36,305・evaluation_infra 145,206・tooling 25,253・docs 590,402・schema 66,352。

## 試験の統合（受け入れ条件 0。規約 10）
file の移動・target の束ねは無い。差分は 14 file、+73 −253。

| 消した・統合した試験 | 守る仕様 | 今も検出する試験 | 失敗注入 | 結果 |
|---|---|---|---|---|
| task-worker `cluster_login/tests.rs` の `path_has_executable_*` 2 件 | `detach::path_has_executable` は実行可能な file だけを見つける | `detach::tests` の同名 2 件（本文が同一） | 判定から `is_executable` を外す | `detach::tests::path_has_executable_ignores_non_executable_files` FAIL |
| task-worker `paperqa/tests.rs` の `single_line_summary_collapses_whitespace_and_truncates` | `research_runner::single_line_summary` は空白をたたみ上限で切る | `local_deep_research::tests` の同名の試験 | 空白たたみを外す | FAIL |
| celerisctl `accept`・`cancel`・`rereview`・`retry` の `run_on_missing_task_returns_message_error`・`run_on_invalid_id_returns_message_error` 8 件 | 4 コマンドは存在しない id と形の悪い id を `CliError::Message` で返す | `commands::tests::task_id_commands_return_message_errors_for_missing_and_invalid_ids`（4 × 2 を全部呼ぶ） | `retry` の誤りを panic に／`accept` が形の悪い id で `Ok` | どちらも FAIL（失敗の文にコマンド名と id） |
| task-core の schema 一致の試験 7 件の本文 | 生成 schema と `docs/protocol/*.schema.json` の一致（ADR-0003 D6） | 同じ 7 件（名前は不変）。本文を `crate::assert_committed_schema` に | 3 つの commit 済み schema に空白を足す | 対応する 3 件だけ FAIL |

- 本体の変更: task-worker `cluster_login.rs` の再 export `pub use crate::detach::path_has_executable`（使っていたのは消した写しの試験だけ）を消した。
- 統合しなかったもの: adapter 別（claude_code・codex・acp、langmem・local-deep-research・paperqa）の似た試験は adapter ごとの配線を見るので残した。store の高 risk path の `store/tests.rs` の `event_row_schema_matches_committed` は設計 review の範囲を増やさないため共通化していない。

## 変更（commit の件名）
- `loc-reduction-v3 G: 重複した試験を統合する（celerisctl の id 誤りの 8 件を 1 件に、task-core の schema 一致の 7 件を共通の検査に、task-worker の写し 3 件と使われない再 export を消す）`
- `loc-reduction-v3 G: 最終評価の進捗ファイルを足す`

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 受け入れの size check（`0 1 10000 1 1`）→ exit 0、`OK []`。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 8,436 passed・22 skipped、web 101 file・705 件 passed、repo-health verdict=pass new 0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/loc-compare.py --build <artifacts>/measure/loc-compare-head.json --head HEAD` → exit 0（product_only 375,099、module の use 1,291、module 循環 3）。
- `node web/scripts/check-parity.mjs` → exit 1。出るのは開始時 main から続く R30 の 1 件だけ。
- 失敗注入 4 組（上の表）→ 注入した試験はすべて FAIL、戻した後に `git status` で差分なし。

## 未解決
- release gate（`scripts/selfdeploy/release.sh`）は worker の run では実行していない。次の release で人か operator が実行し、合否を確かめる。
- `GET /api/v1/inbox` の撤去（承認済み、selfdeploy の `verify.sh` の変更が要る）、未配線の `run_tmpdir::sweep_stale`・`browser_supervisor::reap_on_start`・`source_state_view`、C7・C8 は F の進捗の未解決のまま。

## 提案
- 次に消す候補は `artifacts/final-report.md` の 7 節。試験では `browser_h3_wire.rs` と `browser_injection_wire.rs` の起動の道具の共通化（約 400〜500 行）が大きいが、本物の browser の試験なので release gate で確かめる task にする。
