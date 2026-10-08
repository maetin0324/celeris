---
title: CoS の作成文書をチャットで開く — 実装・検証記録
tasks: [01M4CDNAZF1CDBZVHFDZ1JZJPX]
status: done
updated: 2026-10-08
---

# CoS の作成文書をチャットで開く — 実装・検証記録

[ADR](../adr/2026-10-08-cos-workspace-files-in-chat.md) に従い実装。run 01M4CEWHJ5BKRMXD3TAZZ75GR6（attempt 2）で前回の作業を引き継いだ。本番サービス・DB・KB 正本は変更していない。外部 LLM・外部ネットワークを使わず、fake adapter と一時 DB、loopback の fake daemon/gateway で検証する。

## 実装

- CoS run 前後の thread workspace の差分と返事中の path 言及から、既存の添付 store にファイルを取り込む。内部ディレクトリ・symlink を除外し、添付件数・容量制限を適用する。
- assistant の message に添付参照と `workspace_files` を保存。終端 message event で web に届ける。既存の message は空配列として読め、migration は不要。
- 絶対/相対 path・inline code・Markdown link から添付を開く。Markdown/TextViewer を再利用して md を描画、csv などを行番号付き表示。画面内表示しない形式はダウンロードを維持する。
- cos-operator §10 / cos-inbox-triage §6 に「結論を最初の 1〜3 行」「画面名とボタン名」「運用者作業を分離」を追加。旧運用節との矛盾を解消し、両 skill の version を 2 にした。人だけの実行認可は変えていない。

## KB の変更提出

repo の `config/skills/{cos-operator,cos-inbox-triage}/SKILL.md` は配置用の原稿。実際に run が読むのは KB の `skills/` であり、repo の変更だけでは本番に反映されない。

`celerisctl knowledge get skills/<name>/SKILL.md` で読んだ正本を基点に、以下を run 成果物として提出する。KB 正本への適用は未実施。

- `kb-skills.patch`: `skills/cos-operator/SKILL.md` と `skills/cos-inbox-triage/SKILL.md` を対象とする unified diff。
- `kb-skills-changes.json`: 同じ2ファイルの置換本文・基点 SHA-256・出典 task。差分の適用前に正本の変更を検知できる。

保存先は `/local/celeris/data/workspaces/01M4CDNAZF1CDBZVHFDZ1JZJPX/artifacts/`。一時写し上で patch を適用し、repo 原稿との一致も確認する。

## 検証

| コマンド | 結果 |
|---|---|
| `bash scripts/dev/test-parallel.sh` | exit 0、4730 passed / 0 failed / 14 ignored、tmp leftovers 0（整形後の tree） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web test` | exit 0、Vitest 83 files / 601 tests、gateway 77 tests |
| `pnpm -C web lint` | exit 0、既存 styles.css の `!important` 警告4件 |
| `pnpm -C web build` | exit 0 |
| `cargo fmt --all -- --check` | exit 0（整形後の tree） |
| `git diff --check` | exit 0 |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| `pnpm -C web e2e chat/workspace-files.spec.ts` | exit 0、1 passed（md/csv 表示・path/Markdown link・PDF download） |

前回の全体試験は 4716 passed / 3 failed。失敗は、新欄 `workspace_files` を含まない roundtrip fixture 2件と、同一 millisecond の ULID 順序を upload 順と仮定した試験1件。fixture を現行 API に揃え、添付IDは集合として比較し、workspace path の優先順は別途検証するよう直した。旧 message のデコード互換・添付上限で run が失敗しない試験も追加した。

ブラウザ試験は返事の SSE を fake daemon から送り、md の見出し・内容の表示、csv の表示、path のリンク化、Markdown link の変換、PDF のダウンロードを確認する。Rust の fake adapter 試験は実際に workspace の md/csv を書き、返事への pin・保存本文・終端 event に載る対応表を確認する。待ちは worker の join と画面要素/出来事で行い、sleep に依存しない。

実機 LLM・本番反映は試験していない。

ログ: run 成果物の `test-parallel-attempt2.log`、`clippy.log`、`web-{typecheck,test,lint,build,e2e}.log`。前回の失敗ログは `test-parallel.log` に保持。

## 差し戻し対応（attempt 3）

- 検査対象 SHA: `4d4a30f815d741309b657da3e6471cba02253a8b`。整形コミット: `293161a9`。進捗コミット: `9f1ff787`。最終 SHA: `9f1ff78749bc4abbe53f0361092cb56a0893d202`。
- 旧記録の `cargo fmt --all --check` exit 0 は誤り。指定 SHA では fmt が exit 1 だったため、`cargo fmt --all` の出力だけを適用した。
- 整形したファイル: `crates/task-api/src/cos/operations.rs`, `crates/task-api/tests/cos_triage.rs`, `crates/task-core/src/chat/triage.rs`, `crates/task-core/src/chat/triage_tests.rs`, `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs`, `crates/task-dispatch/src/dispatcher/tests/cos_chat_triage_fallback.rs`, `crates/task-dispatch/src/dispatcher/tests/cos_chat_triage_inbox_thread.rs`, `crates/task-worker/src/cos_chat.rs`, `crates/task-worker/src/cos_chat/tests.rs`, `crates/task-worker/src/protocol.rs`。
- 再検査: `bash scripts/dev/test-parallel.sh` exit 0（4730 passed / 0 failed / 14 ignored）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
- 文書検査: `sh scripts/dev/progress-index.sh --check`, `sh scripts/dev/check-doc-links.sh`, `sh scripts/dev/check-adr-numbers.sh` は全て exit 0。
