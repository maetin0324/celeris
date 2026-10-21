---
title: 全体の試験は release gate だけ — task 全体の最終証跡
tasks: [01M5BZY0AMGFA150CPDPY1YBV1]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# 全体の試験は release 作成時だけ（task 直下の証跡）

完了日: 2026-10-20

ADR は `agent-docs/adr/2026-10-20-full-tests-at-release-only.md`。工程ごとの記録は `agent-docs/progress/2026-10-20-full-tests-release-only/` の `adr-script.md`・`docs-config.md`（prompts の指示文変更は別 WU）。この WU（record-evidence）はコードを変えず、最終検査の証跡だけを記録する。

## 証拠コマンドと結果

| コマンド | exit | 出力の要点 |
|---|---|---|
| `bash scripts/dev/test-affected.sh` | 0 | 「試験対象の変更なし（docs のみ等）」。基準が `$CELERIS_WU_BASE` で、この WU は進捗ファイルだけを足すため対象 crate なし |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告なし。`Finished dev profile` |
| `cargo fmt --all -- --check` | 0 | 差分なし |
| `bash scripts/dev/test-parallel.sh`（foreground、1 回だけ） | 0 | nextest 276 binaries + doctest 13。passed 6212 / failed 0 / ignored 17。nextest 201.5s、doctest 11.8s。`test-parallel: ok` |

test-parallel の CELERIS_TEST_SUMMARY: `"jobs": 8, "passed": 6212, "failed": 0, "ignored": 17, "nextest_exit": 0, "doctest_exit": 0, "userns": null`。

- 失敗した試験: なし。flaky の切り分けは不要。
- `userns: null`: userns / 本物の browser の試験は sandbox の制約（userns が作れない）で skip された。これらは release gate で走る。
- 警告: 「tests left 62 entries in TMPDIR (removed on exit)」。run 終了で消える一時 file。本変更由来ではない。

## この 1 回の実行について

人の決定（run-once）: 本 task は検査の仕組みそのものを変えるため、例外として記録 WU で 1 回だけ全体の試験を走らせた。ADR 2026-10-20-full-tests-at-release-only の「全体の試験は release gate だけ」と矛盾しない（今回限りの例外で、以後の WU・段・phase 完了では回さない）。

## 未解決事項

- 本番 `~/.config/celeris/workspace.toml` の check の書き換えは未実施（下記）。
- cos-operator skill など本リポジトリ外の起票定型に `test-parallel.sh` が残っていれば、運用者・CoS が `bash scripts/dev/test-affected.sh` に直す。

## 提案

- 起票定型の受け入れ条件から `bash scripts/dev/test-parallel.sh` を外し、`bash scripts/dev/test-affected.sh` + clippy + fmt に置き換える。

## 本番への移行

運用者が実施する（本 run は本番 config を触らない）。リポジトリ側 `.config/celeris/workspace.toml` は更新済み。

1. 確認: `grep -n -A3 '^\[commands\]' ~/.config/celeris/workspace.toml`（無ければ作業不要）。
2. `[commands] check` を次に書き換える:
   `check = ["bash scripts/dev/test-affected.sh", "cargo clippy --workspace -- -D warnings", "cargo fmt --all -- --check"]`
3. 反映: daemon の config reload（または次回 release の promote）。確認は新しい段の統合の検査が test-affected.sh を呼ぶこと。
