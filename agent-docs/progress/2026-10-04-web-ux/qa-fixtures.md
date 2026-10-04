---
title: Web UI 状態確認用 fixture
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: running
updated: 2026-10-04
---

# Web UI 状態確認用 fixture

## 画面群の rich データ

`createFakeDaemon({ profile: "rich" })` を screenshot・mobile-audit の共通 gateway で使う。既定 profile の件数・文言は変えない。

| 画面 | 追加したデータ |
| --- | --- |
| `/tasks` | 長い題名の T1、親子・依存を持つ T2/T3、追加 21 件（計 24 件） |
| `/graph` | 親子を含む 8 節点、依存の 2 辺 |
| `/tasks/T1` | 複数行の目的、子 2 件、依存先、R1 run、長い作業場所 |
| `/tasks/T1/changes` | 長い path の変更 15 件、複数行の diff |
| `/tasks/T1/files` | 長い作業ツリーの path、複数の file、本文 |
| `/tasks/T1/runs/R1` | 会話・tool 使用・tool 結果を含む 27 行の JSONL |
| `/artifacts?project=P1` | 長い path の成果物 12 件と Markdown 本文。`/artifacts` は画面仕様どおり案件の選択を先に示す |
| `/`（Console） | 人の発話、返事の tool step、tool の進行行を含む block 3 件 |

API の schema 適合と実画面のデータ表示は `web/e2e/states/rich-data.spec.ts` で確認する。

## mobile-audit で見つかった画面側の制約

rich データを使う 360/390/412/1440px の監査で、`/tasks`・`/graph`・files・run ログ・`/artifacts` は通った。`/tasks/T1` の R1 へのリンクは 17×44px が 2 箇所あり、同画面と changes の textarea は accessible name がないため落ちた。いずれも画面コードの変更が必要だが、この WorkUnit では `web/features`・`web/components` を変更できない。fixture のデータは残し、後続の UI 修正で再監査する。
