---
title: API 説明の実装照合 — §3.63 以降
tasks: [01M3YNFVFZG5AFFM0RS05X22ZT]
status: done
updated: 2026-10-02
---
# API 説明の実装照合 — §3.63 以降

## 処理した文書

| 処理(削除/統合/移動/修正) | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|
| 修正 | `docs/api/v1/gui-api.md` | - | §3.63〜§3 末尾の現行 handler・型との不一致を修正し、§2 に載る未説明 route を補った | `d9524c0f` |

## 直した点

- `retry` の本文に `execution` を追加し、既定の `accept: true`、複製先の gate 再判定、管理認証を明記した。実装は `handlers/task_actions.rs`、`types.rs`、`task_ops::retry`。
- `PATCH /tasks/{id}` の `workspace`、`project_id`、`pause_after` と、`failed` の作業場所変更・unroutable な `blocked` task の再開例外を追記した。実装は `task_ops::edit::TaskEdit` と `handlers/task_actions.rs`。
- 途中目標の cancel/pause/resume を現行の 410 `removed_by_adr_0079` に直し、案件操作 5 本は個別の実在 route で示した。実装は `lifecycle.rs`。
- `run: container` が実行に使われないという古い説明を削除した。実装は `task-worker/src/container.rs`。
- `GET /llm/sources` と MCP 観測 2 本の認証を、通常の読み取り認証（token_file 設定時のみ bearer 必須）に直した。実装は各 handler と `middleware.rs`。
- §2 の一覧にあった routing、rereview、cluster settings、docs maintenance、scratch、browser policy / identity / Live View / control の route を §3.126 に追加した。各 route の本文・応答・管理認証・主な状態コードは対応 handler と型で確認した。
- この範囲の見出しから Phase 番号などの経緯ラベルを外し、retry・通知・リリースの経緯を ADR 参照に縮めた。

## 証拠

| コマンド | 結果 |
|---|---|
| `rg -n 'pub(crate) fn routes|\.route\(' crates/task-api/src` と各 handler・`types.rs`・`crates/task-ops/src` の照合 | §3.63 以降の見出し内の `METHOD /path` 87 件はいずれも router のパスに一致。browser control の 6 本は定数 `BASE` と `format!` を展開して確認 |
| §2 の（メソッド、パス）を §3 の本文と比較する Python ワンライナー | §3 に記載のない行は #8/#9/#10/#32/#33 の run ファイル 5 本だけで、§3.8 のファイル系として既にまとめて記載 |
| `sh scripts/dev/check-doc-links.sh docs/api/v1/gui-api.md` | `check-doc-links: ok`、exit 0 |
| `git diff --check` | exit 0 |
| `sh scripts/dev/check-doc-links.sh` | exit 1。43 件は README・agent-docs・docs/guides・docs/ops・gui/docs にあり、この WU の変更ファイルに起因するものは 0 件 |

Rust・生成 schema は変更していないため、`cargo test --workspace` と `cargo clippy --workspace -- -D warnings` は実行していない。

## 未解決

- リポジトリ全体のリンク検査は担当範囲外の 43 件で失敗する。gui-api.md 単体のリンク検査は通る。
- `gui/docs/celeris-api-v1.md` は削除された `docs/api/v1/overview.md` を参照する。gui/ はこの task では変更できない。

## 提案

- §2 と router の一致、および §2 の各 route に §3 の説明があることを文書検査に加える。定数から組み立てる browser control の 6 本も対象にする。
- `gui/docs/celeris-api-v1.md` を編集できる作業で、旧 overview.md への参照を gui-api.md への案内に変える。
