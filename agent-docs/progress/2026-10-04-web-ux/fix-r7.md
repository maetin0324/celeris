---
title: fix-r7 ホーム stale の狭い幅と長い ID 1440 の実行節
status: running
tasks: [01M463G0XRS7V5CPFEY753F3GT]
updated: 2026-10-05
---

# fix-r7 ホーム stale の狭い幅と長い ID 1440 の実行節

最初に `git merge --no-ff daf78b88`（visual-qa-r2 の子 branch）を取り込み、前回の修正・`web/e2e/work/narrow-r6.spec.ts`・`reshoot-r6.md` をそのまま引き継いだ（merge commit `f083eaed`）。長い ID 1440 の実行節は別の WorkUnit（long-id-exec）が担当し、この下に節を足す。

## ホーム（/）stale 360・390・412 の初期 viewport

### 原因

- stale fixture（`streamStatus: 503`）では黄帯（3 行）・判断待ちの枠（見出し行＋受信箱の入口が 2 段に折れる＋期限の近い 3 件）・通知の入口が縦に積まれ、360 では入口の nav が 484px（y=115〜599）あった。会話枠は y=615 から始まり、宛先行の下端 667 と fixed の送信欄の上端 717 の間に会話本文が約 50px しか見えなかった（390 も 50px、412 は 87px）。
- 会話枠は「最低の見える高さ」を保つためページを 927px まで伸ばしていたので、会話は page を scroll しないと読めなかった。
- 会話枠は末尾追従のあと、宛先行の下で途中から始まる block を飛ばす（narrow-r6 (5)）。block の高さが 110/138/105px の fixture では、宛先行と送信欄の間が約 255px 未満だと最後の 1 block（105px）しか残らず、送信欄の上に余白が出る。150px を満たすには 255px 以上の空きが要る。

### 直し方（`web/routes/index.tsx`、md 未満だけ）

- 黄帯: 文を「接続を確認しています。判断待ちと会話の最新の状態は未確認です。」までにし、「再接続後に受信箱を開いて確かめてください。」は md 以上だけに出す。文字を `text-label`・上下の余白を `py-2` にして 2 行に収める（64→58px）。
- 判断待ちの枠: 受信箱の入口の「判断待ち N 件」は見出しと同じ件数なので電話幅では省き、見出し・未確認の badge・入口を 1 行に収める（−37px）。
- 未確認の間は期限の近い判断待ちを最も近い 1 件に絞る（残りは `max-md:hidden`、件数は見出しと受信箱の入口が示す。先頭の項目の divide-y の下線も消す）。表示中のデータが未確認なので、電話幅では会話の見える量を優先した。
- 入口の nav の間隔を `gap-2` に詰める。
- 結果: 360・390・412 とも nav は y=115〜384、宛先行の下端 452、送信欄の上端 717。会話本文は 2 block（約 244px）見え、ページは viewport（800px）に収まり scroll しない。1440 は変わらない（黄帯の全文・3 件・入口の件数が出る）。
- 接続状態の切替（確認中 ⇄ 再接続中）では、黄帯と「未確認」は `useUnconfirmedConnection`（fix-r6）で出たまま。header の接続状態は 1 行の pill で語の長さが変わっても高さは同じ。新 e2e で document の高さが切替の前後で同じことを確かめた。

### e2e

- 新規 `web/e2e/work/home-stale-viewport.spec.ts`（functional）: stale の / を 360・390・412 × 800 で開き、接続状態が「再接続中」になり黄帯が出るのを待ってから、scroll しないまま会話の list を会話枠・宛先行の下端・送信欄の上端・viewport で切り取った高さが 150px 以上であることを確かめる。その後 MutationObserver で接続状態の切替（connecting → reconnecting の 1 巡）を出来事として待ち、各切替時点と終了時の `document.documentElement.scrollHeight` が最初と同じで、`scrollY` が 0 のままであることを確かめる（固定時間の待ちは無い）。
- 既存 `narrow-r6.spec.ts` (6) の前提 `expect(scrolled).toBeGreaterThan(0)` は、ページが viewport に収まって scroll できなくなったため `scrollY === max(0, scrollHeight − innerHeight)`（末尾まで送った位置）に改めた。会話の 150px・最後の block が送信欄より上・切替中も黄帯と scroll 位置を保つ、の検査はそのまま残した。skip・削除した試験は無い。

### 検査結果（HEAD は merge `f083eaed` ＋ この変更）

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` exit 0
- `corepack pnpm@12.6.0 -C web typecheck` / `lint` / `test` / `build` すべて exit 0（lint の警告は既存の states.spec.ts・styles.css の分だけ）
- `corepack pnpm@12.6.0 -C web e2e e2e/work/ e2e/shell/` exit 0、45 passed（home-stale-viewport 3 件・narrow-r6 全件・home-layout を含む）
- 修正前は同じ新 spec が 3 件とも失敗（50.3 / 50.3 / 87.3px < 150）
- `git merge-base --is-ancestor daf78b88 HEAD` exit 0
- `git log --format= --name-only 7eab1be6a4e3..HEAD --not main | grep -E '^(crates|web/server)/'` 出力なし
- 画像（run の artifacts、追跡しない）: `/local/celeris/data/workspaces/01M463G0XRS7V5CPFEY753F3GT/wu/home-stale/artifacts/` の `before-360.png`、`after-360.png`・`after-390.png`・`after-412.png`・`after-1440.png`（stale の /、viewport で撮影）

### 未解決・提案

- 会話の block が全部 150px 未満で、宛先行と送信欄の間が 2 block 分に満たない幅・高さ（例: 高さ 700 の電話）では、(5) の揃えで 1 block だけになり得る。会話枠の揃えを「飛ばすと余白が block の半分を超えるなら飛ばさない」にする案があるが、(5) の基準との両立を確かめてから決めたい。

## 長い ID の task 詳細 1440 の実行節（WU long-id-exec）

### 原因

- reshoot-r6.md で保留の `long-id-_tasks_<LONG_TASK_ID>-1440` は、実行節に取得失敗帯（role=alert と「再試行」）が残っていた。
- `web/e2e/support/states.ts` の long-id 状態は `/api/v1/tasks/<LONG_TASK_ID>` と `/timeline` だけを長い ID の path に写していた。詳細画面が取る `/execution`・`/routing`（ほか `/changes`・`/tree`・`/artifacts`・`/runs/R1/stdout.jsonl`）は T1 の path にしか無く、偽 daemon の既定の 404 になっていた。画面の不具合ではなく fixture の欠けだった。

### 直し方

- `longIdFixtures()` で、rich fixture の `/api/v1/tasks/T1/` の下の path をすべて `/api/v1/tasks/<LONG_TASK_ID>/` に写す。値の中の `task_id: "T1"`（execution の plan・routing とその runs）は `withLongTaskId` で長い ID に差し替える（関数の fixture はそのまま使う）。fake-daemon.mjs は変えていない（rich の T1 の execution・routing を共有する）。
- 1440 で他に崩れが無いことを確かめた: h1 の長い ID は 1 行に収まる。`#main` の下に横 overflow する要素は無く（scrollWidth > clientWidth の要素 0 件）、document の横 scroll も無い。layout の修正は要らなかった。

### e2e

- 新規 `web/e2e/states/long-id-execution.spec.ts`（functional）: long-id 状態の偽 daemon で `/tasks/<LONG_TASK_ID>` を 1440×900 で開き、次を確かめる。
  - `execution-view` の phase が verifying で「v1（2 件）」が出る。routing panel に「担当 ui-ux」が出る。
  - 実行節に role=alert と「再試行」ボタンが無い。`data-fetch-state` に loading・error が無い。
  - document の横 scroll が無い。
- 修正前の states.ts では同じ spec が失敗する（`execution-view` の `[data-phase]` が見つからない）ことを確かめた。既存の試験は skip・削除していない。

### 検査結果

- `corepack pnpm@12.6.0 -C web e2e e2e/states/ e2e/work/ e2e/shell/` exit 0、80 passed（long-id-execution・states.spec・rich-data.spec・home-stale-viewport・narrow-r6 を含む）
- `corepack pnpm@12.6.0 -C web typecheck` / `lint` / `test` / `build` exit 0
- `git log --format= --name-only 7eab1be6a4e3..HEAD --not main | grep -E '^(crates|web/server)/'` 出力なし
- 画像（追跡しない）: `/local/celeris/data/workspaces/01M463G0XRS7V5CPFEY753F3GT/wu/long-id-exec/artifacts/long-id-1440.png`
