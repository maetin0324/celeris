---
title: 取り残し検出の不備を直す（default branch の一本化・遠隔 repo・受信箱 item の同一性）
updated: 2026-10-11
status: running
tasks: [01M4NM5720BYCYHXT02297GFKK]
---

# 取り残し検出の不備を直す

設計決定は [ADR: done task の変更を取り残さない](../adr/2026-10-11-stranded-done-branches.md) の「付記: 取り残し検出の不備（2026-10-11 本番 deploy）」（F1〜F3）を参照。

完了日: 未完了。

## 目的

2026-10-11 の本番 deploy（50f349d84fea）で取り残し sweep が done root task 218 件を誤検出した。

- default branch NULL の 180 件（うち 175 件は取り込み済み）: 解決経路を `task_ops::changes::resolve_default_branch` の 1 関数にまとめ、sweep・delivery ほか全呼び出し元が使う（F1）。
- 遠隔 repo の 38 件: sweep は ssh せず `not_applicable` にし、受信箱に出さない（F2）。
- 5 分ごとに item が新着に見える: 状態が同じなら行を書かず、item の `at`・id を変えない（F3）。

## 段の一覧

| 段 | unit | 内容 |
|---|---|---|
| design | adr | ADR 付記と本進捗ファイル（コード変更なし） |
| build | resolve | `resolve_default_branch` を作り、旧 `default_branch`・sweep の個別判定・`"main"` 埋めを消し、全呼び出し元を移す |
| build | remote | `not_applicable` 状態（migration の CHECK・sweep・API・celerisctl） |
| build | identity | 状態が変わらない再 sweep で行を書かない |
| build | web-state | web の統合状態表示に `not_applicable` を足す |
| close | verify | 全体検証（test-parallel・clippy）・ADR 付記と実装の突き合わせ・本番への移行節の仕上げ |

葉の記録は `agent-docs/progress/2026-10-11-stranded-sweep-fixes/<unit key>.md` に書く。

## 証拠

- adr: [adr.md](2026-10-11-stranded-sweep-fixes/adr.md)

## 未解決事項

- （build・close 段で埋める）

## 本番への移行（骨子。verify で仕上げる）

- 運用セッションが手で入れた agent-platform の `default_branch=main`（`PATCH /api/v1/repos/01M2XPE3NZ1FZE061FKDY4A9PK`）は残してよい。F1 の解決順で設定が最優先になり、結果は同じ `main`。戻す必要はない。
- 新しい migration（`task_repo_integration_states` の CHECK に `not_applicable`）は daemon 起動時に適用される。適用前に DB の backup を取る（通常の release 手順）。
- deploy 後の最初の sweep で、benchfs の `unintegrated` 38 行が `not_applicable` に移り、受信箱から消えることを確認する（`celerisctl integration list --state unintegrated` と `--state not_applicable`）。
- deploy 後 2 回目以降の sweep で、受信箱の `unintegrated` item の `at` が変わらないことを確認する。
- 本番 host の操作（release・promote・daemon の差し替え）は人が行う。

## 提案

- （なし）
