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

## 状態の変種

状態は `web/e2e/support/states.ts` の一覧に置く（`screens.ts` の台帳行・`spa-routes` の画面数は変えない）。各状態は偽 daemon の設定（`createFakeDaemon` の option。既定は rich profile）と代表画面の URL を持ち、`web/e2e/states/states.spec.ts`（functional project）が 360px で 1 画面ずつ偽 daemon を起こし直して確かめる。

| 状態 key | 偽 daemon の設定 | 当てた画面 | 確かめること |
| --- | --- | --- | --- |
| `long-text` | T1 の題名・目的と受信箱の先頭項目に、区切りの無い英数字を含む長文 | `/inbox`・`/tasks`・`/tasks/T1` | 長文が描かれ、文書の幅が viewport を超えない（横 scroll なし） |
| `long-id` | 区切りの無い 52 文字の task ID（一覧・詳細・timeline）と長い案件 ID、受信箱の項目 ID | `/inbox`・`/tasks`・`/tasks/<長い ID>` | 長い ID の行・見出しが描かれ、横 scroll なし |
| `empty` | task 一覧・graph が 0 件、受信箱・通知が 0 件（`inboxItems: []`・`notices: []`） | `/inbox`・`/notifications`・`/tasks`・`/graph` | 空表示の文言が出て、エラー表示が無い |
| `many` | task 150 件・受信箱 40 件・通知 60 件 | `/inbox`・`/notifications`・`/tasks` | 件数と最後の項目が描かれ、横 scroll なし |
| `loading` | `hold: {}`（`/api/v1` の JSON を `releaseHeld()` まで保留。時計・busy loop を使わない） | `/inbox`・`/tasks`・`/tasks/T1`・`/providers` | `data-fetch-state="loading"`・`aria-busy`・「読み込み中…」、解放後に消える |
| `error` | `fault: { status: 503 }`（受信箱・task・providers の取得） | `/inbox`・`/tasks`・`/tasks/T1`・`/providers` | 再試行の後に `role="alert"` のエラー表示と「再試行」ボタン |
| `stale` | `streamStatus: 503`（`/api/v1/stream` が最初から 503 を返し続け、再接続も失敗する） | `/`・`/tasks`・`/tasks/T1/runs/R1`・`/providers` | 接続状態が「再接続中」（`data-connection="reconnecting"`）になり、取得済みのデータは残る |
| `forbidden` | permission denied。daemon は rich のまま、`route` で gateway の `/api/inbox/items`・`/api/tasks`・`/api/providers` を 403 にする | `/inbox`・`/tasks`・`/tasks/T1`・`/providers` | `data-fetch-state="permission-denied"` と「権限がありません」 |

偽 daemon に足した制御: `fault`・`hold`・`streamStatus`・`inboxItems`・`notices` の option と、`setFault()`・`releaseHeld()`・`heldCount`・`dropStreamClients()`。既定の option では挙動は変わらない。試験用 gateway（`web/e2e/support/fixture-gateway.ts`）は option を受けて偽 daemon を返す。

### 画面が状態を描けない所見

- `long-text /inbox`: 受信箱の項目の題名 link が `inline-flex` の中で `break-words` のため、区切りの無い長い語が折り返さず 360px で文書幅が 417px になる（横 scroll）。画面コード（`web/features/inbox`）の変更が要るので、この WorkUnit では直さず、`states.spec.ts` に期待どおり失敗する試験（`test.fail`）として残した。直ると試験が「予想外の成功」で落ちるので、そのとき `findings` から外す。
- `forbidden`: gateway（`web/server/relay.js`）は daemon の 401/403 を 502 `daemon_auth` に変える（ブラウザの session 切れと混ぜない設計）。daemon の 403 では画面は 5xx のエラー表示になり、permission denied の表示は出ない。権限の表示を確かめるため、states.ts の `route` で gateway の応答を 403 にした。
- `stale`: SSE 切断中も各画面は取得済みのデータを出したままで、画面ごとの stale 表示（`StaleState`）は出ない。切断は header の接続状態（「再接続中」）だけで伝わる。`StaleState` は取得済みのデータがあり再取得が失敗したときに出る。
- rich profile に `/api/v1/org` の fixture は無く、`/org` は取得に失敗する。管理画面の代表には `/providers` を使った。

### 確認

- `corepack pnpm@12.6.0 -C web e2e e2e/states/`: 31 件（`long-text /inbox` は期待どおりの失敗）、exit 0
- `corepack pnpm@12.6.0 -C web e2e`: 158 passed・8 skipped、exit 0
- `typecheck`・`lint`・`test`・`check:boundaries`・`check:parity`: すべて exit 0
