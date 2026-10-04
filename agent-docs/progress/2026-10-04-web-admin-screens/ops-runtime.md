---
tasks: [01M43Z27WF8WE678SKSYWPEX7G]
status: running
started: 2026-10-04
---
# 実行系 ops: clusters・daemon・releases の状態表と昇格の確認表示

基点 `ac78fad613ad`。この file は WU `status`（clusters・daemon）が作り、releases 節は後続の WU `releases` が足す。

## clusters（/clusters）

### 変更点
- `web/features/ops/clusters-screen.tsx`: 一覧の先頭に Table「クラスタの状態」（クラスタ・接続・最終確認・最後の切断・失敗理由）を置き、操作はクラスタごとのカード（`listitem`「クラスタ <id>」）に分けた。
  - 接続状態は Badge の文字で示す（接続中・未接続・コード入力待ち・ログインが必要・不明）。色だけには頼らない。
  - 失敗理由は `stats.last_24h.last_lost_cause`（無ければ `since_start`）、`tunnel_forwards[].last_error`、`cooldown_until` を文字で並べる。無ければ「なし」と書く。
  - 最終確認は API に欄が無いので、この画面が状態を取得した時刻（`dataUpdatedAt`）を出し、表の上の説明文にそう書いた。
  - カードの値は DataList（host・認証・使用中・作業ディレクトリの値）。
  - 操作が 403 を返したら、画面の全操作（接続・コード送信・取り消し・作業ディレクトリ）と入力欄を無効にし、`role=alert` で「権限がありません（403）… 理由: <daemon の detail>」と出す。
  - 生の色・任意値は無い。入力欄の枠は `@layer base` の `--color-input` のまま（色を指定しない）。
- `web/e2e/parity/ops.spec.ts`: 作業ディレクトリの表示が DataList の値（`/work/me（db）`）になったので、その 2 行の locator を合わせた。h1・accessible name・URL は変えていない。

## daemon（/daemon）

### 変更点
- `web/features/ops/daemon-screen.tsx`: 「状態」区画の先頭に稼働状態の Badge と理由の文（稼働中: 最後の tick が 1 分以内 / 応答なし: 1 分以上前 / 状態なし: snapshot が無い）。続く DataList に版・最終 poll・最後の tick・host・instance・開始・tick・各件数。
  - 版は `/api/releases` の `running.release（role）` から読む（`releaseKeys.list()` を /releases 画面と共有）。403 なら「権限がなく読めません（403）」、他の失敗は「取得できません」と文字で出す。
  - 最終 poll はこの画面の最終取得時刻（相対と絶対）。自動 polling の上限（30 回）に達したら「自動更新は上限に達したので止めました」と出す。
  - replay が 403 を返したらボタンを無効にし、理由を alert で出す。結果は差あり・差なしの Badge と件数の文。
  - 生の色（`text-red-800`・`text-amber-900`・`text-green-800`・`border-neutral-300`）を token に置き換えた。
- `web/features/ops/daemon-poll.ts`: `DAEMON_STALE_TICK_MS`・`daemonLiveness`・`daemonPollExhausted` を足した（既存の `daemonPollInterval` は不変）。

## 検査結果（clusters・daemon）

| 検査 | コマンド | 結果 |
| --- | --- | --- |
| typecheck | `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| lint | `corepack pnpm@12.6.0 -C web lint` | exit 0（既存の warning 4・info 1 は今回の差分と別） |
| test | `corepack pnpm@12.6.0 -C web test` | exit 0（vitest 299 passed、server node --test pass 42 / fail 0） |
| e2e | `corepack pnpm@12.6.0 -C web build && corepack pnpm@12.6.0 -C web e2e e2e/admin/ops-runtime.spec.ts e2e/parity/ops.spec.ts` | 15 passed |
| 生の色・任意値 | FRONTEND_CONTRACT §66 の grep を clusters-screen.tsx・daemon-screen.tsx・daemon-poll.ts・routes/clusters.tsx・routes/daemon.tsx に | 0 件 |

`web/e2e/admin/ops-runtime.spec.ts`（新設、7 件）: clusters の状態・失敗理由の文字、403 で操作無効と理由、360px で横溢れ 0、daemon の稼働中・版・最終 poll、古い tick で「応答なし」と理由、403 で replay 無効と版の理由、360px で横溢れ 0。fixture に無い状態は `page.route` で応答を差し替えた。

crates/ は無差分なので cargo test・clippy は走らせていない。

## screenshot

`corepack pnpm@12.6.0 -C web screenshots --out <WU artifacts>/after-status`（exit 0、32 画面 × 4 幅）。
該当の file は WU artifacts の `after-status/_clusters-{360,390,412,1440}.png` と `after-status/_daemon-{360,390,412,1440}.png`。
360px では状態表がその枠（region「クラスタの状態」）の中だけで横にスクロールし、ページは溢れない。

## 要望（fixture・API）
- `ClusterView` に最終確認時刻（例 `last_checked_at`）と直近の接続失敗理由（例 `last_error`）が無い。今は画面の取得時刻と、切断原因・転送エラーで代用している。API に足せば表の列をそのまま差し替えられる。
- `DaemonSnapshot` に daemon の版（release sha）が無い。今は `/api/releases` の `running` を別に読んでいる。
- 偽 daemon（`web/e2e/support/fake-daemon.mjs`）の `/api/v1/daemon` は `snapshot` を返さず `now` が `"fixture"`。screenshot でも「状態なし」になる。snapshot 付きの fixture と、`/api/v1/clusters` の切断・転送エラー付きの cluster があると、screenshot で表の中身を確かめられる。

## 未解決
- releases 節（昇格・巻き戻しの ConfirmDialog）は後続の WU `releases` が書く。
