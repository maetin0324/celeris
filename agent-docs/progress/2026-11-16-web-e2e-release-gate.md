---
title: web の e2e の既存の不合格 18 件を直し、release gate に web の e2e を入れる
tasks: [01M7H1RNG5C5AA76HNB2M9M098]
status: done
updated: 2026-11-16
---
# web の e2e の既存の不合格 18 件を直し、release gate に web の e2e を入れる

完了日: 2026-11-16

人の決定（2026-11-16、運用 session の問いへの答え「起票し gate に e2e も足す」）による task。
設計は [ADR 2026-11-16-release-gate-web-e2e](../adr/2026-11-16-release-gate-web-e2e.md)（実装より先に commit）と、
[ADR 2026-10-20-full-tests-at-release-only](../adr/2026-10-20-full-tests-at-release-only.md) の付記 A1（D4 と実際の gate の食い違いの訂正）。

## 変更（commit の件名）
- `ADR: release gate の web 段で e2e（functional）を流す`
- `web e2e: board の偽 daemon を本物と同じ status・order・limit にし、releases の制御を通す`
- `web e2e: browser run の試験を Live View の有無の条件で分ける`
- `web e2e: chat の queue 試験を固定の thread で流し、legacy が読むだけであることを確かめる`
- `web e2e: chat-home と Console の試験を固定の CoS thread の画面に合わせる`
- `web e2e: 狭い画面の tree の状態を scroll してから横の収まりで確かめる`
- `web e2e: mount の scroll 復元の揺れを、一覧を読み込み済みにしてから測って直す`
- `release gate: web 段に web-e2e-chromium と web-pnpm-e2e を足す`
- `docs: gate の web 段の記述と進捗`（testing.md・selfdeploy.md・project-controller.md、この進捗ファイル）

### 組ごとの直し方（どれも assert を弱めず、skip もしていない）

| 組 | 直し方 | いま確かめていること |
|---|---|---|
| board（parity/projects・work/projects・work/wait-reasons） | `web/e2e/support/fake-daemon.mjs` に `taskListPage` を置いた。本物の daemon（`crates/task-api/src/handlers/tasks.rs`）と同じく `status`（繰り返しと comma 区切り）・`order`（`created_desc`・`updated_desc`）・`limit`（既定 100、上限 500、0 は 400）・`cursor` で絞る。静的な `/api/v1/tasks` fixture には偽 daemon が自動で使い、browser backend の一覧の独自の status の絞り込みもこれに置き換えた（二重の実装を残さない）。関数の fixture（board の 2 件）は関数の中で呼ぶ | 各 task が 1 回だけ出る（`[data-task-id='<id>']` が 1 件ずつ、件数が 6・2・2）。parity は board が終端でない 5 状態と終端の 3 状態の 4 種の要求を送ること |
| browser run（lease 2 件・links・narrow-a11y） | 下の「browser の control bar・lease の試験を残した根拠」のとおり、control bar の試験は Live View の映像が無い run（`liveUpstream: false` = 中継の無い `runtime = "daemon"`）の fixture で流す。消えた iframe の assert は、映像の枠が無く「イベントで監視」の案内（`data-reason="relay_unavailable"`）を出すことに替えた。links は Live View の映像の枠と読み取りだけの帯を確かめる | 映像が無い run: lease の取得・延長・返却・離脱時の release・期限切れ、360 px で sticky の control bar が先頭。映像がある launcher の run（新しい試験、`framesAvailable: true`）: 読み取りだけの帯、WebSocket `/browser/live/T1/R1/frames` の接続、iframe・control bar の状態・「一時停止」「引き継ぐ（60 秒）」「エージェントに返す」が無いこと、360 px の並びと横溢れ無し |
| chat queue（5 件） | `?thread=chat-legacy` を統括 CoS の `chat-main` に替えた。1 件足した | 送信（Enter・Shift+Enter・IME）・queue の順と取消・停止と resume_queue・キューの再開・割り込み（mode=interrupt）を固定の thread で。足した試験は案件の CoS（chat-history）の送信が queue で届くこと、legacy の thread は履歴を読めて入力欄・送信の button が無く注記を出し、投稿を送らないこと |
| chat-home | thread の作成の試験を、いまの挙動の試験に書き換えた | 統括 CoS・案件の CoS の archive が 409、legacy の archive は 200、ホームは `?thread=chat-main` を開き `POST /api/v1/chat/threads` を送らない |
| Console（home-layout 1440） | 固定の座標（消えた「新しい会話」「会話を検索」のあった所）で探って 0 件だった。枠（`[data-home-console]`）と宛先の行（`[data-console-toolbar]`）の位置を DOM から読んで探る | 枠の上端の行の右の点がすべて宛先の行に当たる（点の数 > 0）、border 付きの要素が枠の上端で切れず文字を持つ、宛先の行が枠の上端から始まる |
| 狭い画面（narrow-layout 360・390） | `toBeInViewport` の前に行を scroll して見せる | badge が全部見える（ratio 1）、左端 0 以上、右端が幅以下。表と枠の横の収まりは元のまま |
| releases（parity/ops :45・:59・:68） | 迷い込み接続の filter が `/__fake/` を通すようにした。試験の制御は応答 200 と設定後の値を assert する | :45 click から成功の表示まで pendingMs（2500）以上かかる、:59 失敗の表示（exit 1）、:68 gateway の入れ替わりの後の巻き戻しの成功。`fake-server-noise.test.ts`（vitest）に `/__fake/releases` が filter を通り台本が効く単体試験を足した |
| mount（shell/mount :49 の揺れ） | 下の「mount の揺れの原因」のとおり、rich の偽 daemon を付け、一覧が読み込み済みになるのを待ってから scroll する | 見出しへの focus、戻るで scroll 位置が 1200 ±20 に戻る、戻った画面の高さが離れる前と同じ |

### fake daemon の `/__fake/` の修正を外すと parity/ops.spec.ts が落ちることの確認
`fake-daemon.mjs` の `!pathname.startsWith("/__fake/") &&` の行を消して `WEB_E2E_SCOPE=functional pnpm -C web exec playwright test --workers=2 e2e/parity/ops.spec.ts`
→ 3 failed（:45・:59・:68 がどれも制御の応答で `Expected: 200 / Received: 404`）、5 passed。行を戻して全件合格。

### browser の control bar・lease の試験を残した根拠
- `web/features/browser/browser-run-screen.tsx` は、run があり Live View の状態が `frame` でないとき `ControlBar`（状態・一時停止・引き継ぎ・延長・返却）を出す。
  `frame` になるのは gateway が `live: {state: "link"}` を返すときで（`web/server/browser-live.js` の `liveAvailability`）、launcher の frame がある run か、
  daemon runtime で dashboard の中継（`CELERIS_WEB_LIVE_VIEW_UPSTREAM`）がある run。
- control bar が出る run は今もある: `[browser] runtime = "daemon"`（`crates/celeris/src/config/mod.rs` の `runtime_kind`）で中継が無い run（relay_unavailable）、
  実行中でない run、旧い launcher protocol の run（`launcher_protocol_no_live_frames`）。daemon の takeover の口（`crates/task-api/src/browser_control.rs` の
  `/api/v1/tasks/{id}/browser/control/{run}/{session}`）も現行で、CoS の操作面（`crates/task-api/src/cos/ops/surface.rs`）にも載っている。
- よって control bar・lease は旧い仕組みの残りではなく現行の経路なので消さず、その条件（Live View が無い run）の fixture で確かめる。
  launcher の Live View の読み取りだけの帯は ADR 2026-10-10-browser-launcher-live-view-frames D4 のとおりで、別の試験で確かめる。

### mount の揺れの原因
- `--repeat-each=40` で 40 回中 27 回落ち、どれも戻った後の `scrollY` が 1252（1200 との差 52）。
- ResizeObserver で測ると、試験は daemon の無い gateway で流していたため、戻るたびに一覧の取得がやり直されて失敗し、`#main` の
  読み込み中の枠（64 px）が失敗の表示（116 px）に替わっていた。替わる時刻が scroll の復元（`components/shell/scroll-memory.ts`、
  onResolved の次の frame）の後だと、伸びた 52 px を browser の scroll anchoring が足していた（画面の不具合ではなく、試験の前提の欠け）。
- 直した後は `--repeat-each=40` で 40 回中 40 回合格。時計や待ち時間は足していない（一覧の行と、読み込み中・失敗の枠が無いことを待つ）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
受け入れの検査の結果は、この task の完了報告（transcript）に書く。ここには計測と確認だけを書く。
- 直す前（作業の開始時）: 12 の spec を `--workers=4` で流すと 18 failed・31 passed。落ちた 18 件は背景の表と同じ。
- `bash scripts/selfdeploy/tests/release_web_required.sh` → exit 0（`release_web_required: ok`）。release.sh から 2 段の行を消すと
  `FAIL: web steps did not all run/pass` で exit 1。
- release.sh を呼ぶ他の self-test 13 本（release_web_bundle_requires_deps・release_uses_scratch_lease・release_lock_bundled_scripts・
  release_notes_promote・release_parallel_test_gate・prepare_timeout_test・release_prune_stale_test_binaries・release_target_settled_after_build・
  browser_ledger_release_stages・release_web_required・repo_health_bundle・bundle_includes_verify_helpers・sync_skills）と local_state_layout → すべて exit 0。
- `node web/scripts/check-e2e-chromium.mjs` → exit 0（chromium 153.0.8010.12、1.4 秒）。`PLAYWRIGHT_BROWSERS_PATH` を空の dir にすると exit 1 で、
  Playwright の `Executable doesn't exist at …/chromium_headless_shell-1243/…` と入れ方を出す。

### 所要時間の計測（2026-11-16、この host（24 core）、load average 17〜23 の下、既定の worker 数 8）
task の指示による計測として 1 回ずつ流した（受け入れの検査ではない）。

| コマンド | 結果 | 時間 |
|---|---|---|
| `pnpm -C web e2e`（functional。gate の `web-pnpm-e2e` と同じ） | exit 0、318 passed、8 skipped、0 failed | 120 秒 |
| `pnpm -C web e2e:nfr` | exit 1、123 passed、2 failed（下の「未解決」） | 39 秒 |
| `pnpm -C web e2e:all` | exit 1、441 passed、2 failed、8 skipped、41 did not run（nfr の失敗で nfr-latency・release が走らない） | 128 秒 |
| `playwright test latency/transition.spec.ts parity/latency-gate.spec.ts parity/cutover.spec.ts`（nfr-latency と release） | exit 1、35 passed、5 failed、1 did not run（cutover） | 63 秒 |

## 本番への移行
- 次の release から、gate の web 段に `web-e2e-chromium`（約 2 秒）と `web-pnpm-e2e`（この host の計測で約 120 秒）が増える。
  release の全体の時間はその分だけ延びる。
- release を作る user（本番は celeris の release を回す user）の `~/.cache/ms-playwright` に、`web/package.json` の `@playwright/test`（1.63.0）に合う
  chromium（`chromium_headless_shell-1243` と `chromium-1243`）が要る。この run を流した host・user の `~/.cache/ms-playwright` にはある。
  本番の release を回す user で同じかは未確認。無い host では gate が
  `web-e2e-chromium` で落ちるので、人が `pnpm -C web exec playwright install chromium` を実行する。gate は download しない。
- `@playwright/test` の版を上げる変更の後は、上の install を人が実行してから release を回す（版に合う chromium が要る）。
- config・DB の変更は無い。

## 未解決
- e2e:nfr の既存の不合格 2 件（この task の 18 件の外）: `parity/mobile-gate.spec.ts:27`（axe /browser/settings。期待 2 件の要素が 0 件）と
  `realtime/refetch-scope.spec.ts:31`（S2 /accounts。関係の無い SSE で再取得が 5 回、上限 2）。
- nfr-latency の既存の不合格 5 件: `latency/transition.spec.ts:24` の S1（/・/knowledge・/knowledge/inbox・/tasks/$id/runs/$runId・/cron/$id）。
  どれも `toHaveURL("/")` に対して `/?thread=chat-main` で、ホームが統括 CoS を開くようになった変更に追いついていない古い試験（時間の閾値ではない）。
- release project（`parity/cutover.spec.ts`）は nfr-latency の失敗で走らず、時間は未測定。
- この 7 件の修正は followup の task として起票した（`followups.json`）。

## 提案
- `e2e:nfr` は 39 秒なので、上の 2 件を直した後に gate に入れてよい（入れるかは別の task で決める。ADR 2026-11-16-release-gate-web-e2e D3）。
- `e2e:all` は functional と nfr を並べて流し、続けて nfr-latency（時間の閾値）と release（dist/ の作り直し）を流す。nfr-latency の 5 件が
  直るまでは gate に入れられない。直した後も、時間の閾値の試験は release gate の host の負荷に左右されるので、入れるなら nfr-latency を
  別の段にして failed_step で区別する案を勧める。
- WU の check が自分の spec だけを流すため不合格がたまった。gate に e2e が入ったので、今後の退行は次の release で見つかる。
