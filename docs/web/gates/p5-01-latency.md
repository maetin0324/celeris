# P5-01 遅延 gate（2026-10-01）

---
tasks: [01M3TG9K4VV5Z4GZ0WY4DCVBZS]
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
