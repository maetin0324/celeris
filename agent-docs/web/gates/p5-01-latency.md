# P5-01 遅延 gate（2026-10-01）

---
tasks: [01M3TG9K4VV5Z4GZ0WY4DCVBZS, 01M41RYPGEQQKT2KBPH1WYH0A6]
---

## 測定条件と判定

Chromium、loopback の偽 daemon と gateway（空き port）、`v3Screens()` の全 30 行を測定した。JSON 応答だけを 0 / 5 / 10 秒遅らせ、SSE は遅らせていない。S1 は URL・見出し・枠を別に測った。各値は 1 回の実測値で、中央値や分布を推定していない。

S1 の閾値は URL と見出しが各 **300 ms 以下**、10 秒条件と 0 秒条件の差が各 **100 ms 以下**。S2 は各画面で無関係な `worker_progress` 20 件と 2 秒周期の `daemon` 10 回を送り、画面固有の再取得 **0 本**を確認した。shell の `/health` と `/daemon` の 5 秒補完取得、`/inbox` の 15 秒補完取得は画面固有の本数から除いた。

## 全画面の S1 / S2

| path（fixture） | URL/見出し 0 s (ms) | URL/見出し 5 s (ms) | URL/見出し 10 s (ms) | 差 10−0 の絶対値 (ms) | S2 画面固有の再取得 |
|---|---:|---:|---:|---:|---:|
| `/` (`/`) | 33.1/36.8 | 36.1/39.8 | 43.9/46.9 | 10.8/10.0 | 0 |
| `/inbox` (`/inbox`) | 42.2/80.8 | 48.2/79.6 | 34.9/68.2 | 7.3/12.6 | 0 |
| `/org` (`/org`) | 29.2/61.1 | 49.1/81.1 | 28.6/61.4 | 0.5/0.2 | 0 |
| `/org/secretary` (`/org/cos`) | 56.9/66.6 | 60.7/65.5 | 53.8/58.9 | 3.1/7.7 | 0 |
| `/org/$id` (`/org/cos`) | 57.4/64.7 | 60.1/64.5 | 67.3/71.9 | 9.9/7.2 | 0 |
| `/projects` (`/projects`) | 44.5/76.7 | 32.4/64.8 | 31.6/64.4 | 12.8/12.3 | 0 |
| `/projects/$id` (`/projects/P1`) | 57.8/66.2 | 61.8/71.5 | 60.8/65.4 | 3.1/0.8 | 0 |
| `/projects/$id/docs` (`/projects/P1/docs`) | 60.1/68.0 | 67.9/73.5 | 52.6/57.0 | 7.5/11.0 | 0 |
| `/projects/$id/docs/maintenance` (`/projects/P1/docs/maintenance`) | 62.1/71.9 | 58.4/63.1 | 56.1/63.0 | 6.0/8.8 | 0 |
| `/board` (`/board`) | 39.6/70.6 | 32.7/63.5 | 39.5/71.2 | 0.2/0.7 | 0 |
| `/knowledge` (`/knowledge`) | 41.4/72.5 | 32.3/62.0 | 39.3/69.5 | 2.1/3.0 | 0 |
| `/knowledge/inbox` (`/knowledge/inbox`) | 69.4/81.2 | 64.9/69.8 | 60.8/65.5 | 8.6/15.7 | 0 |
| `/knowledge/skills` (`/knowledge/skills`) | 59.4/69.0 | 58.4/62.7 | 60.5/64.8 | 1.1/4.2 | 0 |
| `/reports` (`/reports`) | 43.7/74.9 | 34.6/65.4 | 37.6/67.8 | 6.1/7.1 | 0 |
| `/approvals` (`/approvals`) | 40.3/71.2 | 37.7/67.6 | 34.2/64.7 | 6.1/6.5 | 0 |
| `/artifacts` (`/artifacts`) | 43.9/74.9 | 44.4/76.2 | 29.6/60.8 | 14.3/14.1 | 0 |
| `/tasks` (`/tasks`) | 41.5/72.6 | 34.0/64.2 | 36.9/67.9 | 4.5/4.6 | 0 |
| `/tasks/new` (`/tasks/new`) | 60.9/76.1 | 59.7/64.4 | 58.0/63.6 | 2.8/12.5 | 0 |
| `/tasks/$id` (`/tasks/T1`) | 64.2/72.7 | 49.5/54.1 | 60.5/65.7 | 3.7/6.9 | 0 |
| `/tasks/$id/files` (`/tasks/T1/files`) | 58.9/68.3 | 51.2/55.6 | 65.3/69.9 | 6.5/1.7 | 0 |
| `/tasks/$id/changes` (`/tasks/T1/changes`) | 59.7/68.8 | 58.4/63.5 | 58.3/63.2 | 1.4/5.7 | 0 |
| `/tasks/$id/runs/$runId` (`/tasks/T1/runs/R1`) | 57.9/67.4 | 58.4/62.9 | 59.1/63.4 | 1.1/3.9 | 0 |
| `/plans/new` (`/plans/new`) | 58.0/64.0 | 61.8/66.3 | 59.1/63.6 | 1.1/0.4 | 0 |
| `/daemon` (`/daemon`) | 57.0/90.1 | 37.2/70.5 | 45.7/76.2 | 11.3/13.9 | 0 |
| `/providers` (`/providers`) | 41.6/74.1 | 29.6/61.1 | 45.5/77.1 | 3.9/3.0 | 0 |
| `/accounts` (`/accounts`) | 43.4/76.1 | 43.9/75.4 | 34.5/67.2 | 8.9/8.8 | 0 |
| `/clusters` (`/clusters`) | 49.6/80.5 | 33.4/63.4 | 34.2/64.0 | 15.5/16.5 | 0 |
| `/releases` (`/releases`) | 48.5/79.3 | 37.2/67.3 | 38.0/68.3 | 10.6/10.9 | 0 |
| `/graph` (`/graph`) | 42.3/73.3 | 39.5/70.0 | 36.5/65.9 | 5.8/7.5 | 0 |
| `/help` (`/help`) | 45.2/78.7 | 35.1/64.9 | 37.6/67.3 | 7.6/11.4 | 0 |

実測最大値は URL 69.4 ms、見出し 90.1 ms、10−0 秒の差は URL 15.5 ms・見出し 16.5 ms。30 行とも上記閾値内。

`/inbox` と `/daemon` の画面 Query は上記の補完取得として別扱いにした。`/tasks/new`・`/plans/new`・`/help` はこの fixture で画面固有の初期 Query がなく、S2 の再取得本数は 0 本だった。

## baseline と同じ 3 点

### 7 経路の遷移（JSON 10 s）

| path | URL (ms) | 見出し (ms) |
|---|---:|---:|
| `/inbox` | 113.8 | 118.3 |
| `/tasks` | 44.9 | 59.2 |
| `/projects` | 33.2 | 36.5 |
| `/reports` | 46.8 | 50.1 |
| `/org` | 45.5 | 48.7 |
| `/knowledge` | 45.6 | 48.1 |
| `/daemon` | 33.9 | 41.4 |

### SSE イベントごとの再取得本数（`/tasks` を表示）

| event | tasks | inbox | daemon/rest |
|---|---:|---:|---:|
| `unrelated:worker_progress` | 0 | 0 | 0 |
| `T1:worker_progress` | 0 | 0 | 0 |
| `T1:transitioned` | 1 | 1 | 1 |
| `unknown:transitioned` | 1 | 1 | 1 |

H1 project fallback: 同じ fixture の Query cache を作って 4 件の event を評価したところ、project を解決できず project 集計 key を stale にした回数は **1 回**。これはこの fixture の実測であり、本番の頻度ではない。

### `/tasks` → `/tasks/T1`（JSON 5 s、daemon tick 2 s）

見出しまで 37.4 ms。遷移後の daemon 要求は最初の 12 秒で 3 本、次の 6 秒で 0 本。打ち切りは最初の 12 秒で 3 本、次の 6 秒で 0 本。後半で要求も打ち切りも増え続けていない。

## 再現

依存関係を固定版で用意した後、リポジトリ直下から実行する。`ARTIFACTS` は run の成果物ディレクトリの絶対パスを指定する。

```sh
ARTIFACTS=/absolute/path/to/run/artifacts
WEB_LATENCY_RESULTS="$ARTIFACTS/latency-results.jsonl" corepack pnpm@12.6.0 -C web e2e latency/transition.spec.ts realtime/refetch-scope.spec.ts parity/latency-gate.spec.ts
```

この run ではネットワークからの pnpm 依存取得に失敗したため、既存のローカル `node_modules` を複製し、同じ Playwright 1.63.0 の `node_modules/.bin/playwright` で実測した。生値は run の `latency-results.jsonl`、集約値は `latency-results.json` に保存した。

## 付記（2026-10-03）: 起動の完了を待ってから click を測る

task 01M41RYPGEQQKT2KBPH1WYH0A6（WU spec-ready）、人の決定 b。`e2e/latency/transition.spec.ts`（S1 の各 goto の後）と `e2e/parity/latency-gate.spec.ts`（parity-x 7 経路の最初の goto の後）は、click の前に `e2e/latency/boot-idle.mjs` の `waitForBootIdle` で SPA 起動の完了を出来事で待つ。条件は「nav と `main h1` が出た後、2 frame + `requestIdleCallback` の 1 巡の間に long task（`PerformanceObserver('longtask')`）が 1 つも終わらない」。固定 sleep と networkidle（SSE が開いたまま、JSON は 5/10 s 遅らせる）は使わない。予算 300 ms、10 s−0 s 差 100 ms、Playwright の retries 0、click から URL・見出しまでの測り方は変えていない。

理由: goto は `load` で返るが、その後も起動（session 確認 → shell と home の描画 → data の到着と再描画）の long task が main thread を塞ぎ、goto 直後の最初の click の locator 解決がそれを待つ。この時間は遷移ではなく起動の費用で、host の負荷で増える（前回 qa の commit-fast の切り分け）。

### 変更前・変更後の計測（CPU throttle）

host に負荷をかけず、CDP `Emulation.setCPUThrottlingRate`（1x・4x・6x）で renderer だけを遅くした。手順は 2 spec と同じ（parity 7 経路: goto `/` → 遅延 10 s → 順に click、S1 `/providers`: 遅延 0/5/10 s ごとに goto → nav を click）。変更前は取り込み直後の spec の手順（goto 直後に click）、変更後は `waitForBootIdle` の後に click。同じ build（HEAD `f2859a35`）で、回ごとに変更前・変更後を交互に 3 回。値は click から `main h1` が見えるまでの ms。

| CPU | 経路 | 変更前 click→h1 ms（3 回） | 変更後 click→h1 ms（3 回） |
|---|---|---|---|
| 1x | `/inbox` | 86, 103, 99 | 43, 43, 43 |
| 1x | `/tasks` | 41, 41, 41 | 30, 32, 30 |
| 1x | `/projects` | 37, 37, 37 | 33, 32, 33 |
| 1x | `/reports` | 28, 29, 29 | 29, 29, 30 |
| 1x | `/org` | 33, 32, 32 | 32, 32, 32 |
| 1x | `/knowledge` | 30, 29, 29 | 30, 29, 30 |
| 1x | `/daemon` | 33, 33, 33 | 33, 33, 33 |
| 1x | `/providers@0` | 61, 60, 75 | 71, 71, 71 |
| 1x | `/providers@5000` | 68, 70, 68 | 70, 71, 70 |
| 1x | `/providers@10000` | 73, 74, 73 | 70, 69, 69 |
| 4x | `/inbox` | 234, 235, 229 | 78, 84, 80 |
| 4x | `/tasks` | 36, 35, 34 | 34, 44, 35 |
| 4x | `/projects` | 66, 66, 79 | 65, 81, 66 |
| 4x | `/reports` | 60, 63, 62 | 53, 50, 65 |
| 4x | `/org` | 48, 61, 63 | 49, 61, 49 |
| 4x | `/knowledge` | 44, 44, 60 | 44, 48, 49 |
| 4x | `/daemon` | 48, 45, 47 | 48, 49, 60 |
| 4x | `/providers@0` | 86, 90, 88 | 78, 73, 75 |
| 4x | `/providers@5000` | 71, 85, 81 | 84, 82, 110 |
| 4x | `/providers@10000` | 79, 79, 78 | 76, 74, 73 |
| 6x | `/inbox` | 363, 356, 333 | 106, 103, 100 |
| 6x | `/tasks` | 96, 84, 82 | 109, 99, 104 |
| 6x | `/projects` | 82, 77, 77 | 74, 80, 77 |
| 6x | `/reports` | 84, 81, 56 | 83, 82, 87 |
| 6x | `/org` | 60, 62, 66 | 52, 64, 63 |
| 6x | `/knowledge` | 67, 52, 46 | 50, 47, 42 |
| 6x | `/daemon` | 40, 53, 60 | 51, 60, 59 |
| 6x | `/providers@0` | 104, 106, 121 | 98, 94, 93 |
| 6x | `/providers@5000` | 72, 79, 77 | 82, 80, 80 |
| 6x | `/providers@10000` | 76, 84, 91 | 77, 74, 79 |

| CPU | 変更前 最大 / 300ms 超 | 変更後 最大 / 300ms 超 |
|---|---|---|
| 1x | 103 / 0/30 | 71 / 0/30 |
| 4x | 235 / 0/30 | 110 / 0/30 |
| 6x | 363 / 3/30 | 109 / 0/30 |

変更後は 6x でも全経路が 110 ms 以下で、変更前に予算を超えていた goto 直後の `/inbox`（6x で 333〜363 ms）は 100〜106 ms になった。2 回目以降の click（`/tasks` 以降）は変更前後で同程度で、遷移そのものの費用は変わっていない。

### e2e（retries 0、3 回続けて）

`corepack pnpm@12.6.0 -C web e2e e2e/latency/transition.spec.ts e2e/parity/latency-gate.spec.ts --retries=0` を 3 回続けて実行し、3 回とも **33 passed / 0 failed**（各 2.9 m、load 1 分値 0.2〜0.7）。S1 の URL・見出しの最大は 82 / 87 / 87 ms。

## 付記（2026-10-03）: リンクの無い経路も click で測る

S1 は nav にリンクの無い経路（`/org/cos`、`/projects/P1` 以下 3 件、`/knowledge/inbox`・`/knowledge/skills`、`/tasks/new`、`/tasks/T1` 以下 4 件、`/plans/new` の 12 件・台帳 13 行）で、起動完了を待った後に文書の読み込みそのもの（SPA の再起動全体）を計測区間に入れていた。決定 b に合わせ、リンクの無い経路も起動完了の後のアプリ内遷移で測る（commit `a62ab6e5`）。

- 親画面の表は `web/e2e/latency/in-app-routes.ts`: `/tasks` → `/tasks/T1` → `runs/R1`、`/projects` → `/projects/P1` → `docs` → `docs/maintenance`、`/knowledge` → `inbox`・`skills`、`/help` → `/tasks/new`。親画面を遅延 0 で開き、main 内の `a[href=fixture]` が出て起動が完了してから遅延（0/5/10 s）を設定し、click する。親画面にリンクを描かせる偽 daemon の応答（task 一覧・task 詳細の run・案件一覧・案件詳細）も同じ表に持つ。
- どの画面にも SPA のリンクが無い 4 件（`/org/cos` は「話す」が文書の再読み込みになる素の a、`files`・`changes` は `?tab=` のリンクだけ、`/plans/new` は入口なし）は、home で起動完了を待った後に `history.pushState` と `popstate` で遷移させる。
- 変えないもの: 予算 300 ms、10 s−0 s 差 100 ms、retries 0、計測の起点から URL・見出しまでの測り方。

### 変更前・変更後の計測（CPU throttle、リンクの無い経路 12 件 × 遅延 3 × 3 回 = 108 計測）

台本は spec-ready の `throttle-measure.mjs` を流用（WU artifacts の `throttle-measure.mjs`・`throttle.jsonl`・`throttle-table.md`）。値は計測の起点（変更前は goto、変更後は click・popstate）から h1 まで。

| CPU | 変更前（goto 計測）click/goto→h1 最大 ms / 中央値 / 300ms 超 | 変更後（click・popstate 計測）最大 ms / 中央値 / 300ms 超 | 変更後の 10s−0s 差の最大 ms（h1） |
|---|---|---|---|
| 1x | 122 / 45 / 0/108 | 80 / 69 / 0/108 | 28 |
| 4x | 450 / 181 / 4/108 | 325 / 76 / 1/108 | 178 |
| 6x | 462 / 292 / 48/108 | 426 / 95 / 2/108 | 152 |

変更前は 6x で 108 件中 48 件が予算を超えていた（中央値 292 ms）。変更後は中央値が 1x 69 / 4x 76 / 6x 95 ms。変更後に 300 ms を超えた 3 件（4x `/tasks/T1` 325、6x `/projects/P1/docs` 409・426）は、page 内の click から h1 までが 120〜176 ms で、残りは Playwright の click の actionability 待ち（throttle 下の 2 frame の安定判定）だった。遷移そのものは予算内。

### e2e（retries 0、3 回続けて）

`corepack pnpm@12.6.0 -C web e2e latency/transition.spec.ts parity/latency-gate.spec.ts --retries=0` を 3 回続けて実行し、3 回とも **33 passed / 0 failed**（exit 0、各 2.4 m、開始時の load 1 分値 24.9 / 7.8 / 18.5）。S1 の URL・見出しの最大は 82 / 92 ms。

## 付記（2026-10-04）: リンクの無い経路の click も actionability を区間の外へ

リンクの無い経路を親画面の click で測るよう直した後も、celeris の check（2 spec を retries 0 で 3 回）で S1 `/knowledge/inbox`・`/knowledge/skills`（親 `/knowledge` の click）と `/reports`（nav の click）が URL @0 で 309〜342 ms になり落ちた。page 内の時刻（click イベント・pushState・h1）を取ると、click イベントから pushState までは 1〜80 ms で、残りは Playwright の `click()` の actionability 待ち（stable の 2 frame・hit test の往復）だった。負荷下ではこの待ちが 150〜320 ms になり、`toHaveURL` の polling 間隔と重なって予算を超える。

- 直し方（`web/e2e/latency/transition.spec.ts` だけ、commit `7fc1d0ef`）: 計測区間の前に `click({ trial: true })` で actionability を確かめ、区間では `click({ force: true })`（mouse の move・down・up）だけを打つ。nav のリンクと親画面のリンクの両方。
- 変えないもの: 予算 300 ms、10 s−0 s 差 100 ms、retries 0、click から URL・見出しまでの測り方（`toHaveURL` → h1 の `toBeVisible`）。

### 変更前・変更後の計測（CPU throttle、click で測るリンクの無い経路 8 件 × 遅延 3 × 3 回 = 72 計測）

台本は spec-ready の `throttle-measure.mjs` を流用し、mode `armed` を足した（WU artifacts の `throttle-measure-armed.mjs`・`throttle-armed.jsonl`、計測時の host の load 1 分値は 28.9）。pushState + popstate の 4 件（36 計測）は変更の対象外で、6x でも最大 115 ms。

| CPU | click 計測（actionability 込み）起点→h1 最大 / 中央値 / 300ms 超 | armed click（actionability は区間の前）最大 / 中央値 / 300ms 超 | click() そのものの最大 ms（前 → 後） |
|---|---|---|---|
| 1x | 80 / 70 / 0/72 | 51 / 37 / 0/72 | 41 → 11 |
| 4x | 325 / 83 / 1/72 | 91 / 46 / 0/72 | 318 → 34 |
| 6x | 426 / 101 / 2/72 | 108 / 58 / 0/72 | 299 → 42 |

### e2e（retries 0、3 回続けて）

celeris の check と同じ `corepack pnpm@12.6.0 -C web build` の後に `corepack pnpm@12.6.0 -C web e2e e2e/latency/transition.spec.ts e2e/parity/latency-gate.spec.ts --retries=0` を 3 回続けて実行し、3 回とも **33 passed / 0 failed**（exit 0、各 2.4〜2.5 m）。S1 の 270 計測で URL 中央値 13 / 最大 177 ms、見出し中央値 25 / 最大 230 ms、10 s−0 s 差（見出し）の最大 41 ms。

