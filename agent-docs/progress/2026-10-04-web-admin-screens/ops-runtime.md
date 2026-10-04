---
tasks: [01M43Z27WF8WE678SKSYWPEX7G]
status: done
completed: 2026-10-04
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

## releases（/releases）

### 変更点
- `web/features/ops/releases-promotion.ts`: `isRollback`・`promotionActionLabel`・`promotionStatus`・`promotionConfirmCopy` を足した（既存の `judgePromotion`・`pollDelayMs`・`canPromote` は不変）。
  - 前の版（`is_previous`）を昇格するのは巻き戻し。操作名は「<sha12> を昇格する」/「<sha12> に巻き戻す」。一覧のボタンと確認の確定ボタンで同じ名前。
  - 確認の文言: 対象版（ref 付き）、現在版（`current`、無ければ「なし」）、影響「本番の daemon が <sha12> に引き継がれ、引き継ぎの間は画面と API が一時的に切れます」、戻し方（元の版を再び昇格）。
- `web/features/ops/releases-screen.tsx`:
  - 区画を 3 つに分けた: 「稼働中の版」（DataList: 稼働中・current・previous）、「昇格の結果」、「リリースの一覧」（Table、region 名「リリースの一覧」）。
  - 一覧の列: 版・状態（sha12・ref・current/previous/実行中/gate の Badge）、操作、ビルド、昇格、変更、問題・直近の失敗。360px でも版と操作が最初の画面に入るよう、状態は版の列にまとめた。
  - 昇格・巻き戻しは ConfirmDialog を挟む。押せないときはボタンを無効にし、理由（稼働中の版です・昇格中は操作できません・gate を通っていません・問題があります・権限がありません）を下に文字で出す。
  - 結果は StatusBadge（実行中・完了・失敗）と文で出す（testid `promote-pending`・`promote-succeeded`・`promote-failed` は保った）。
  - POST が 403 を返したら、全ての昇格・巻き戻しを無効にし、`role=alert`（`releases-denied`）で「権限がありません（403）… 理由: <daemon の error>」と出す。
  - 生の色（`text-red-800`・`text-green-800`）を token と Badge に置き換えた。入力欄は無い。
- `web/e2e/parity/ops.spec.ts`: /releases の 3 件を確認表示を経る流れに合わせた（一覧のボタン → alertdialog に「本番の daemon」→ 同じ名前の確定ボタン）。ボタン名は「bbbbbbbbbbbb を昇格する」と、1 件目の後で前の版になる「aaaaaaaaaaaa に巻き戻す」。h1「リリース」・URL `/releases`・testid は変えていない。
- `web/e2e/admin/ops-runtime.spec.ts`: releases の 4 件を足した（確認表示に対象版・現在版・影響と確定ボタン名、巻き戻しの確認、結果の失敗表示、403 で無効化と理由、360px で横溢れ 0 と確認表示が幅に収まる）。応答は `page.route` で差し替えた。

### 自己レビュー（ui-ux-quality-gate、Tiny change gate）
本番に効く操作の確認と戻し方 ✓、状態は色でなく文字 ✓、403 の理由と回復（権限の付与）✓、長い ref の折り返し ✓、360px で主操作が見える ✓。`current`/`previous` は API の語のまま出している（運用者の語彙として残した）。

### screenshot
`corepack pnpm@12.6.0 -C web screenshots --out <WU artifacts>/after-releases`（exit 0）。該当は WU artifacts の `after-releases/_releases-{360,390,412,1440}.png`。

### 要望（fixture・API）
- 偽 daemon の `/api/v1/releases` は `previous` が常に `null`、`changes`・`problem`・`gate_ok: false` の行が無い。巻き戻し・gate 未通過・問題ありの行があると screenshot で無効理由の表示を確かめられる。
- screenshot 台本は確認表示（ConfirmDialog）を開いた状態を撮らない。開いた状態の 360px は e2e で幅を確かめている。
- 巻き戻し専用の API は無い（前の版の promote で代用）。巻き戻しを別の操作として記録したいなら API に区別が要る。

## 検査結果（全体、releases の後）

| 検査 | コマンド | 結果 |
| --- | --- | --- |
| typecheck | `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| lint | `corepack pnpm@12.6.0 -C web lint` | exit 0（既存の warning 4・info 1 は今回の差分と別） |
| test | `corepack pnpm@12.6.0 -C web test` | exit 0（vitest 302 passed、server node --test pass 42 / fail 0） |
| e2e | `corepack pnpm@12.6.0 -C web build && corepack pnpm@12.6.0 -C web e2e e2e/admin/ops-runtime.spec.ts e2e/parity/ops.spec.ts` | 19 passed |
| 生の色・任意値 | FRONTEND_CONTRACT §66 の grep を ops の 5 file と routes 3 file に | 0 件 |

crates/ は無差分なので cargo test・clippy は走らせていない。

## 未解決
- なし（統合は WU `integrate-build`）。
