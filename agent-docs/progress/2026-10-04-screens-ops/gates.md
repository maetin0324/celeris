---
title: 主画面 1 統合後の web 全検査・full e2e
tasks: [01M43DVA9VA6YFH9X2G34NZ4SB]
status: done
updated: 2026-10-04
---
# 主画面 1 統合後の web 全検査・full e2e

WorkUnit gates の記録。対象は build 段 4 葉（run-console・changes-files・task-detail・task-list-graph）の統合結果 HEAD 01cdbf90。

## 検査の結果（修正後）

すべて `corepack pnpm@12.6.0 -C web <script>` で実行した。

| 検査 | exit | 要点 |
|---|---|---|
| install --frozen-lockfile | 0 | |
| typecheck | 0 | `tsc -b` |
| lint | 0 | 既存の warning 4・info 1（今回の差分とは別） |
| test | 0 | vitest 45 files / 284 tests、server node --test 42 tests |
| build | 0 | |
| check:secrets（build の後） | 0 | |
| check:boundaries | 0 | |
| check:parity | 0 | |
| mobile-audit | 0 | 30 path × 4 幅（360/390/412/1440）ok |
| e2e（full、`--retries=0`） | 0 | 213 passed、8 skipped（17.5 分）。開始前の load average 1 分値 1.91 |

e2e の前に `/proc/loadavg` の 1 分値が 16 未満になるまで sleep で待つ形にした（CPU を焼く負荷はかけていない）。

## 落ちたものと修正

1 回目の full e2e（retries 0、load 3.98）は 1 件だけ落ちた（212 passed）:

- `e2e/latency/transition.spec.ts` の `S1 /tasks/$id/runs/$runId`。見出しは出たが、h1 の親の中に h1 以外の要素が無く「画面データ」の待ちが not found で失敗した。
- 原因: run-console 葉で run ログ画面が `ScreenFrame` の page header slot（breadcrumb・actions）を使うようになった。slot を使うとき h1 は `div.min-w-0` に包まれ、description が無いと親の中身が h1 だけになる。page header slot を使う画面は run ログ画面だけだった。
- 修正（`web/features/runs/run-log-view.tsx`）: page header に description「run の状態と出力。末尾にいる間は追記に合わせて送ります。」を足した。試験・予算・retries と共有 component（`web/components/`）は変えていない。
- 確認: latency/transition・parity/latency-gate・parity/runs-files・parity/console を通しで実行し 45 passed。そのあと上の表の全検査と full e2e を流し直して全部 exit 0。

## 未解決事項

- `ScreenFrame` で header slot を使い description を省くと、S1 の「h1 の兄弟」の待ちが成り立たない。今後 slot を使う画面は description を付けるか、S1 試験の側でデータの待ち方を page header に合わせる必要がある（`web/components/`・`web/e2e/latency/` はこの task の範囲外なので変えていない）。

## 提案

- S1 試験のデータ待ちを `[data-screen] > :not([data-slot="page-header"]):not(h1)` のように ScreenFrame の構造に合わせ、description の有無に依存しないようにする（別 task）。
