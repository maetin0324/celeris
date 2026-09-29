# web/ の実装計画（Phase 1〜7）

---
tasks: [01M3MS2JRDJ4GM0D9VN9PJCB6B]
---

[ADR-0081](../adr/0081-web-spa-frontend.md) を実装するための計画。Phase 0（ADR、[feature parity matrix](feature-parity.md)、
遅延 baseline）の次に行う仕事を、**1 セッションで終わり、1 commit で戻せるタスク**に分けて並べる。
設計の正本は ADR-0081、移行の gate は parity matrix で、この計画はその順番と検査を決める。

- 照合基点: main `06e9a03cffe8`。現行 GUI の遅延 baseline（2026-09-28 計測）は、遷移が「遅延 × 3」
  （5 s で約 15 s、10 s で約 30 s）、SSE のどのイベントでも表示中の全 loader を再実行、遅延 5 s + tick 2 s で
  `/tasks` → `/tasks/:id` が 120 s 経っても終わらない、だった。Phase 5 の gate はこの 3 点と同じ手順で測る。
- **gui/ の削除・rename はこの計画のタスクに含めない**。配信切替（cutover）と dogfood が終わった後に、
  人が承認して起こす別タスクで行う（§Phase 7）。Phase 1〜6 のタスクは gui/ の追跡ファイルを 1 つも変えない。

## 1. タスクの単位と規則

| 規則 | 内容 |
|---|---|
| 大きさ | 1 タスク = 1 セッション（1 run、目安 2 時間以内）= 1 commit。終わらないと分かったら延ばさず、終わった分を commit して残りを次のタスクに分ける |
| 変更の範囲 | Phase 1〜5 は `web/` と `docs/web/` だけを変える（Phase の完了記録の `docs/PROGRESS.md` とその分割ファイルを除く）。`gui/`、`crates/`、`docs/api/`、`deploy/`、`scripts/selfdeploy/` は変えない。Phase 6 だけが `deploy/` と `scripts/selfdeploy/` に触れる |
| rollback | そのタスクの commit を `git revert` する。Phase 5 まで web/ は利用者に配信していないので、revert で利用者の画面は変わらない。Phase 6 のタスクは個別の戻し方を書く |
| 依存の追加 | `web/` は独立した pnpm の workspace と lockfile を持つ（`gui/pnpm-workspace.yaml` と同じ方針: 公開から 7 日未満の版を使わない、install スクリプトは許可リストだけ）。版は導入するタスクで確かめて lockfile で固定する |
| 設計判断 | ADR-0081 に無い判断が要るときは、先に `docs/adr/NNNN-*.md` を足してから実装する |
| 名前 | 世代名（`v2`、`next` など）を directory・package・route・script・環境変数に使わない。既存の API 契約 `docs/api/v1/` と、現行の URL の search param `/login?next=` は別 |
| テスト | 外部ネットワークに出ない。本番の celeris（`:7710`）と GUI（`:7700`）、staging（`:7701` `:7711` `:7712`）に接続しない。偽 daemon と gateway は空き port を取る |
| commit | 題は `web phase <N> <タスク ID>: <要約>`。本文に理由を書く |
| parity の更新 | 行を閉じるタスクは、`docs/web/feature-parity.md` のその行の「状態」を `完了（<commit>）` に書き換える。「確認方法」のテストが web/ にあり、通ったときだけ |
| Phase の完了 | その Phase の全タスクが終わったら `cargo test --workspace` と `cargo clippy --workspace -- -D warnings` を走らせ、`docs/PROGRESS.md` に完了日・証拠コマンドと結果・未解決事項を書く |

## 2. 検証コマンドの組

各タスクの「検証」は、次の組とそのタスク固有のコマンドを並べて書く。`<base>` はそのタスクを始めた commit。

**V1（gui/ の回帰。全タスク）**

```sh
git diff --quiet <base> -- gui                      # gui/ の追跡ファイルに差分なし
pnpm -C gui test && pnpm -C gui typecheck && pnpm -C gui build
```

**V2（web/ の共通検査。P1-01 より後の全タスク。P1-02 までに揃う）**

```sh
pnpm -C web typecheck && pnpm -C web lint && pnpm -C web test && pnpm -C web build
pnpm -C web gen:types --check        # docs/api/v1/api-v1.schema.json から再生成して差分なし
pnpm -C web check:boundaries         # gui/ を import しない、client に server/ と token の処理が入らない、
                                     # routes/ の loader・beforeLoad が fetch を待たない、世代名なし、routes/ の 1 ファイルは 150 行以下
pnpm -C web check:secrets            # build の出力・HTML・エラー本文・要求ログに fixture の token が出ない（P1-07 から）
pnpm -C web check:parity             # 「完了」の行の確認方法のテストが web/e2e/parity/ にある
```

**V3（画面の共通検査。画面を作るタスク。P2-07 で揃う）** — `<path>` はその画面の URL

```sh
pnpm -C web e2e latency/transition.spec.ts -g "<path>"      # S1
pnpm -C web e2e realtime/refetch-scope.spec.ts -g "<path>"  # S2
pnpm -C web mobile-audit --only "<path>"                     # S3（幅 360 / 390 / 412 / 1440）
pnpm -C web e2e a11y/axe.spec.ts -g "<path>"                # S4
pnpm -C web screenshots --only "<path>" --out "$ARTIFACTS/shots"   # S7
```

### 共通の受け入れ条件

全タスク:

- **C1** gui/ の追跡ファイルを変えず、gui/ の test・typecheck・build が通る（V1）。
- **C2** web/ の typecheck・lint・単体テスト・build が通る。型は schema からの生成物と一致する（V2）。
- **C3** `web/` は `gui/` を import しない。client の import graph に `server/` と token の設定が入らない。
  daemon の token が HTML・bundle・bootstrap・エラー本文・要求ログに出ない（V2）。
- **C4** 1 commit で戻せる。戻しても gui/ の配信に影響しない。

画面を作るタスク（V3）:

- **S1 遷移の独立**: 偽 daemon の JSON 応答に 0 / 5 / 10 s の遅延を入れたどの場合も、クリックから URL の変化と
  遷移先の見出し・枠の表示まで 300 ms 以内。10 s のときと 0 s のときの差は 100 ms 以内。データ待ちの間も
  ナビ・戻る・進む・タブ切替が使える。
- **S2 再取得の範囲**: その画面を開いたまま SSE の `daemon` を 2 s 周期で 10 回、無関係な task の
  `worker_progress` を 20 件流しても、その画面の Query の再取得は 0 本（ADR-0081 D5 の `daemon/rest` の 5 s、
  `inbox` の 15 s の補完取得は数えない）。関係するイベントでは ADR-0081 D6 の表の key だけを取り直す。
  遅延 5 s + tick 2 s でも進行中の取得を打ち切り続けず、daemon への要求が増え続けない。
- **S3 mobile**: 幅 360 / 390 / 412 / 1440 px でページ全体の横溢れ 0、タップ領域 44×44 px 以上。長い表・ログ・
  DAG・差分の横スクロールは枠の中に閉じる。
- **S4 a11y**: axe の critical / serious が 0。ダイアログの focus trap と閉じた後の focus 復帰、遷移後の見出しへの
  focus、label、色だけに頼らない状態表示。IME 変換中の Enter で送信しない。
- **S5 URL**: 現行と同じ path と search param の意味。深い URL の再読み込み、戻る・進むで同じ画面に戻る。
- **S6 遅延と失敗の表示**: 未取得は枠の中の skeleton、1 s 超で待機表示、5 s 超で「取得に時間がかかっています」と
  再試行。取得失敗を 0 件や空の一覧に置き換えない。再取得中は今の内容を保ち「更新中」と最終取得時刻を出す。
- **S7 スクリーンショット**: 現行 gui/ の同じ画面と web/ の画面を幅 360 / 390 / 412 / 1440 で撮り、run の
  artifacts に残す。
- **S8 操作の結果**: 変更系は結果が確定するまで同じ操作を二重送信しない。先に成功と表示しない。409 は該当 key を
  取り直して「状態が変わりました」、422 は celeris の文言を欄の横に出す。結果の表示は再取得で消えない。
  timeout で結果が不明な操作を「失敗」と断定しない。

## 3. 人の判断を待つ点と、決まるまでの扱い

選択肢と推奨は Phase 0 の run の成果物 `web-decisions.md`（リポジトリの外）にある。ここには、決まるまで
この計画がどう進めるかだけを書く。人が別の決定をしたら、下の「待つタスク」を書き直す。

| # | 決めること | 決まるまでの扱い（ADR-0081 の範囲内） | 待つタスク |
|---|---|---|---|
| H1 | SSE の event payload の拡張（`EventRow.project_id` など）の要否 | API を変えない。project は Query cache から解決し、分からなければ project の集計 key を stale にする（ADR-0081 D6） | P2-05。P5-01 で fallback の回数を測って報告する |
| H2 | 並行運用の port | 開発と検査は `127.0.0.1:7720`。本番の port は決めない | P6-02 |
| H3 | 並行運用での認証・session の共有方法 | web/ は自分の cookie 名と署名鍵を持ち、gui/ の cookie を読まない | P1-05、P6-02 |
| H4 | 取得済みデータの永続化の可否 | 永続化しない（ADR-0081 D2）。保存するのは表示の好みだけ | P2-01、P2-06 |
| H5 | 遅延 gate の閾値 | S1 の 300 ms / 100 ms | P2-07、P5-01 |
| H6 | dogfood の期間と cutover の合格条件 | 決まるまで cutover を始めない | P6-03、P6-04、P6-05 |
| H7 | release と selfdeploy に web/ を入れる時期 | Phase 5 まで入れない。selfdeploy の gate は gui/ のまま | P6-01、P6-02 |
| H8 | HTML の成果物の見せ方 | 同一オリジンでは実行させず、download にする（ADR-0081 D3） | P1-08、P3-11 |
| H9 | 並行運用中のブラウザ通知 | web/ の通知は利用者が web/ の origin で許可したときだけ出す | P3-15、P6-03 |
| H10 | 実 celeris に対する結合検査を必須にする時期 | Phase 1〜5 は偽 daemon で検査し、P6-02 の staging で実 celeris に対して確かめる | P6-02 |

## 4. タスクの一覧

「閉じる行」は parity matrix の行。Phase と slice は parity matrix と同じ。Router の package の導入だけは、
`/login`（R04）を Phase 1 で閉じるために P1-01 で行う（shell と Query は Phase 2）。

| ID | slice | タスク | 依存 | 閉じる行 | 人の判断 |
|---|---|---|---|---|---|
| P1-01 | gateway | web/ の scaffold | — | — | — |
| P1-02 | gateway | 型の生成と境界・parity の検査 | P1-01 | X16 | — |
| P1-03 | gateway | 偽 daemon と遅延の注入 | P1-01 | — | — |
| P1-04 | gateway | gateway の骨格（静的配信・Host・CSRF・header・healthz） | P1-02、P1-03 | R03、X2、X3、X4 | — |
| P1-05 | gateway-auth | auth / session と logout | P1-04 | R05 | H3 |
| P1-06 | gateway-auth | login 画面と認証の境界 | P1-05 | R04、X1 | — |
| P1-07 | gateway-relay | JSON API の中継と token の秘匿 | P1-05 | X5 | — |
| P1-08 | gateway-relay | file の中継 | P1-07 | R40、R41、X6 | H8 |
| P1-09 | gateway-relay | SSE の中継 | P1-07 | R37（中継の部分） | — |
| P2-01 | shell | API client・QueryClient・key factory | P1-07 | — | H4 |
| P2-02 | shell | route の宣言と shell、404 | P2-01、P1-06 | R42 | — |
| P2-03 | shell | shell の server state と遅延表示の部品 | P2-02 | X13 | — |
| P2-04 | realtime | SSE の transport | P2-01、P1-09 | — | — |
| P2-05 | realtime | SSE → invalidate の対応表 | P2-04 | R37、X12 | H1 |
| P2-06 | shell | 時刻帯と storage の規律 | P2-03、P2-04 | X7、X8 | H4 |
| P2-07 | shell | 画面の共通検査（V3）の枠 | P2-03、P2-05 | — | H5 |
| P3-01 | console | Console の中継と cache | P2-07 | R38、R39 | — |
| P3-02 | console | Console の画面 | P3-01 | R01、R08、R27 | — |
| P3-03 | inbox | 受信箱と操作の共通部品 | P2-07 | R02、X14 | — |
| P3-04 | inbox | 認可 | P3-03 | R19 | — |
| P3-05 | tasks | タスクの一覧 | P2-07 | R21 | — |
| P3-06 | tasks | タスクと計画の作成 | P3-03、P3-05 | R22、R28 | — |
| P3-07 | tasks | 依存グラフ | P3-05 | R35 | — |
| P3-08 | task-detail | タスク詳細の枠と overview・timeline | P3-05 | — | — |
| P3-09 | task-detail | タスク詳細の操作（判断） | P3-08、P3-03 | — | — |
| P3-10 | task-detail | タスク詳細の操作（実行と routing） | P3-09 | — | — |
| P3-11 | runs-files | 作業ツリーと成果物の viewer | P3-08、P1-08 | R20、R24 | H8 |
| P3-12 | runs-files | run ログ | P3-08、P1-08 | R26 | — |
| P3-13 | task-detail | 変更と、5 tab の結合 | P3-10、P3-11 | R23、R25 | — |
| P3-14 | reports | 報告 | P3-03 | R17、R18 | — |
| P3-15 | reports | ブラウザ通知 | P3-14 | X9 | H9 |
| P4-01 | projects | 案件の一覧と作成 | P3-03 | R09 | — |
| P4-02 | projects | 案件詳細の表示（計画 DAG・仕事の木） | P4-01、P3-11 | — | — |
| P4-03 | projects | 案件詳細の操作（案件と作業場所） | P4-02 | — | — |
| P4-04 | projects | 案件詳細の操作（計画と途中目標） | P4-03 | — | — |
| P4-05 | projects | 案件詳細の操作（リポジトリ） | P4-03 | R10 | — |
| P4-06 | projects | 案件の文書と文書保守 | P4-01 | R11、R12 | — |
| P4-07 | projects | ボード | P4-01、P3-09 | R13 | — |
| P4-08 | org | 組織の木と課の操作 | P3-03 | R06 | — |
| P4-09 | org | 組織の skill の操作 | P4-08 | R07 | — |
| P4-10 | knowledge | 知識と候補 | P3-03 | R14、R15 | — |
| P4-11 | knowledge | skill | P4-10 | R16 | — |
| P4-12 | ops | daemon と provider | P3-03 | R29、R30 | — |
| P4-13 | ops | account とログイン | P3-03 | — | — |
| P4-14 | ops | secret・LLM source・MCP クライアント | P4-13 | R31、R32 | — |
| P4-15 | ops | クラスタ | P3-03 | R33 | — |
| P4-16 | ops | リリース | P3-03 | R34 | — |
| P4-17 | help | ヘルプ | P2-07 | R36 | — |
| P5-01 | latency-gate | 遅延の gate（全画面、baseline と同じ手順） | Phase 3、4 の全タスク | X11 | H1、H5 |
| P5-02 | security-gate | security の gate（全経路） | Phase 3、4 の全タスク | —（X1〜X6、X8 の再確認） | — |
| P5-03 | mobile-gate | mobile・a11y の gate（全画面・4 幅） | Phase 3、4 の全タスク | X10 | — |
| P5-04 | mobile-gate | parity の総点検 | P5-01、P5-02、P5-03 | — | — |
| P6-01 | cutover | web/ の配布物 | P5-04 | X15 | H7 |
| P6-02 | cutover | 並行運用の unit と selfdeploy への組み込み | P6-01 | — | H2、H3、H7、H10 |
| P6-03 | cutover | dogfood | P6-02 | — | H6、H9 |
| P6-04 | cutover | 配信切替 | P6-03 | — | H6（人の承認） |
| P6-05 | cutover | 切替後の観察と戻しの確認 | P6-04 | — | H6 |
| P7-00 | gui-removal | gui/ の削除の起票（**この計画では実行しない**） | P6-05 | — | 人の承認 |

## 5. Phase 1 — scaffold と gateway

Phase の目的: daemon が止まっていても HTML と login が返り、token がブラウザに出ない gateway を作る。
画面は login だけ。

### P1-01 web/ の scaffold
- 作るもの: `web/` の package（React、TypeScript、Vite、TanStack Router の file-based routing、Tailwind CSS 4、
  vitest、Playwright、biome）、独立した `pnpm-workspace.yaml` と lockfile、ADR-0081 D8 の directory、
  `index.html` と `main.tsx`、`__root` と `/login` の空の枠。script は `typecheck` `lint` `test` `build` `e2e`。
- 受け入れ条件: C1、C4。`pnpm -C web build` が fingerprint 付きの asset を出す。directory と package 名に世代名が無い。
  リポジトリ直下と gui/ の設定ファイルを変えない。
- 検証: V1、`pnpm -C web install --frozen-lockfile && pnpm -C web typecheck && pnpm -C web lint && pnpm -C web test && pnpm -C web build`、
  `git diff --quiet <base> -- . ":!web" ":!docs/web"`

### P1-02 型の生成と境界・parity の検査
- 作るもの: `web/scripts/gen-types.mjs`（`docs/api/v1/api-v1.schema.json` → `web/api/generated/`、`--check` 付き）、
  `web/scripts/check-boundaries.mjs`、`web/scripts/check-parity.mjs`（parity matrix の行と `web/e2e/parity/` の
  テストの題を照合する。`--require-phase <N>` でその Phase までの行が全部「完了」であることも見る）。
- 受け入れ条件: C1〜C4。生成を 2 回走らせて差分 0。`gui/` の import、client からの `server/` の import、
  fetch を待つ `loader` / `beforeLoad`、世代名、150 行を超える route ファイルを、それぞれわざと入れた fixture で検出する。
- 検証: V1、V2（`check:secrets` を除く）、`pnpm -C web test scripts/`、
  `pnpm -C web e2e parity/gateway.spec.ts -g "parity-x: 型の再生成"`

### P1-03 偽 daemon と遅延の注入
- 作るもの: `web/e2e/support/fake-daemon.mjs`。schema に合う fixture（gui/ の fixture は参照するだけで import しない）、
  `/api/v1/*` の JSON 応答への 0 / 5000 / 10000 ms の遅延、SSE のフレームを検査側から好きな時に送る口、
  2 s 周期の `daemon` の tick、受けた要求の記録（path・時刻・打ち切り）。
- 受け入れ条件: C1〜C4。fixture の応答が生成した型の schema で検証を通る。遅延は JSON だけに掛かり SSE は素通し
  （baseline と同じ条件）。loopback 以外と `:7700` `:7701` `:7710` `:7711` `:7712` への接続を拒む。
- 検証: V1、V2、`pnpm -C web test e2e/support/`

### P1-04 gateway の骨格
- 作るもの: `web/server/` の Express 5 の app。bind の解釈（既定 `127.0.0.1:7720`）、全経路の Host 許可リスト、
  変更系の CSRF 検査（Origin 一致、`Sec-Fetch-Site` は `same-origin` と `none` だけ）、security headers、静的配信と
  SPA の fallback、`/healthz`、要求ログ（path・status・所要だけ）。
- 受け入れ条件: C1〜C4。HTML は daemon を呼ばずに返る（偽 daemon を止めて 200）。許可外の Host は asset・HTML・
  API・file・stream・404 のどれでも 400。別 origin と別 port（`same-site`）からの変更は 403、ヘッダ無しの CLI の
  要求は現行と同じ扱い。全応答に nosniff・no-referrer・frame 拒否、HTML に inline script なしの CSP、HTML と API は
  no-store、fingerprint 付き asset だけ immutable。未知の `/api/*`・`/files/*`・`/events`・asset は HTML を返さず 404。
  非 loopback の bind でパスワードのファイルが無ければ起動しない。
- 検証: V1、V2、`pnpm -C web test server/`、
  `pnpm -C web e2e parity/gateway.spec.ts -g "parity: /healthz|parity-x: CSRF|parity-x: Host|parity-x: 全応答の security header"`

### P1-05 auth / session と logout
- 作るもの: `GET /api/session`、`POST /login`、`POST /logout`。パスワードの照合（定数時間、失敗は 1 s 待つ）、
  署名付き cookie（HttpOnly、SameSite=Strict、HTTPS で Secure、24 h）。cookie 名と署名鍵は web/ のもの。
- 受け入れ条件: C1〜C4。未認証の `/api/*`・`/files/*`・`/events` は 401。`/login` `/logout` `/healthz` は未認証で通る。
  login と logout にも CSRF 検査が効く。logout で cookie が消える。`next` は同一オリジンの絶対パスだけを受ける。
  期限切れと改ざんした cookie は未認証。gui/ の cookie を受け入れない。
- 検証: V1、V2、`pnpm -C web test server/auth`、`pnpm -C web e2e parity/gateway-auth.spec.ts -g "parity: /logout"`

### P1-06 login 画面と認証の境界
- 作るもの: `/login` の画面（パスワード欄は `autocomplete="current-password"`）、gateway のローカル session だけで
  決まる認証の境界（未認証の保護画面は `/login?next=` へ。daemon の health を条件にしない）。
- 受け入れ条件: C1〜C4、S3、S4、S5、S7。成功・失敗・`next`・偽 daemon を止めた状態の 4 通りで動く。
  キーボードが出てもボタンが隠れない。session 失効で保護データの cache を捨てて login へ移る。
- 検証: V1、V2、`pnpm -C web e2e parity/gateway-auth.spec.ts -g "parity: /login|parity-x: auth"`、
  `pnpm -C web mobile-audit --only "/login"`

### P1-07 JSON API の中継と token の秘匿
- 作るもの: same-origin の `/api/*` → daemon の `/api/v1/*` の中継。upstream は起動設定で固定。token はファイルから
  読んで gateway → daemon の Authorization にだけ付ける。timeout と切断時の abort。`check:secrets`。
- 受け入れ条件: C1〜C4。ブラウザの Authorization を daemon に転送しない。任意の URL・任意のヘッダを中継しない。
  gateway 専用の route（`/api/session` など）は中継より先に扱う。gateway の障害、daemon に届かない、daemon の認証
  エラーを応答で区別できる。token が HTML・bundle・`/api/*` の応答・エラー本文・要求ログに出ない。
- 検証: V1、V2、`pnpm -C web test server/relay`、`pnpm -C web e2e parity/gateway-relay.spec.ts -g "parity-x: token"`

### P1-08 file の中継
- 作るもの: `/files/tasks/:id/runs/:runId/:name` と `/files/tasks/:id/artifacts/:idx`。
- 受け入れ条件: C1〜C4。`:name` `:idx` の `..`・区切り文字・NUL を拒む。Range・`offset`・`length`・`download`、
  206 / 416、`Content-Range` を保つ。daemon の応答ヘッダは許可リストだけ転送し、nosniff を付け直す。HTML の成果物は
  同一オリジンで実行されない。全量を buffer に貯めず、ブラウザが切断したら upstream を abort する。
- 検証: V1、V2、`pnpm -C web test server/files`、
  `pnpm -C web e2e parity/gateway-relay.spec.ts -g "parity: files runs|parity: files artifacts|parity-x: file"`

### P1-09 SSE の中継
- 作るもの: `/events` → daemon の `/stream`。
- 受け入れ条件: C1〜C4。`Last-Event-ID`・`after_id`・`task_id` を検証して転送する。`text/event-stream`、no-store、
  buffering なし。JSON 用の timeout を掛けない（60 s を超えて流れ続ける）。ブラウザが切断したら upstream を abort する。
  未認証は 401、daemon の 401 / 503 を隠さない。R37 は P2-05 で閉じる（状態は `実装中`）。
- 検証: V1、V2、`pnpm -C web test server/events`、`pnpm -C web e2e parity/gateway-relay.spec.ts -g "parity: /events 中継"`

## 6. Phase 2 — shell と realtime

Phase の目的: daemon の状態で mount が変わらない shell と、イベントの種類ごとに範囲を絞った再取得を作る。
画面の中身はまだ枠だけ。

### P2-01 API client・QueryClient・key factory
- 作るもの: `web/api/client.ts`（same-origin の fetch、timeout 15 s、AbortSignal、エラーの種類）、session に 1 つの
  QueryClient、`web/api/queries/` の domain 別 key factory と staleTime（ADR-0081 D5 の表の全 domain）。
- 受け入れ条件: C1〜C4。GET だけ上限付きで再試行し、401 / 403 / 検証エラーは再試行しない。変更系を自動で再送しない。
  logout と session 失効で query と mutation の cache を捨て、進行中の fetch を止める。Query の persist を入れない。
  key は ID と正規化した search / filter を含み、同じ API 応答をバッジ用と画面用に二重に持たない。
- 検証: V1、V2、`pnpm -C web test api/`

### P2-02 route の宣言と shell、404
- 作るもの: 画面 32 本の route の宣言（path と search param の型、中身は見出しと枠）、root の shell（ナビ、ヘッダ、
  Console の置き場、接続状態、outlet）、`components/ui/` の基本部品（Base UI 系の shadcn/ui）、404、
  起動に失敗したときの静的な案内。
- 受け入れ条件: C1〜C4、S3、S4、S5、S7。偽 daemon を止める → 起こす、の間で shell の DOM の node が入れ替わらない。
  root に server state を待つ Suspense が無い。未定義の path は shell の中の 404 で、ナビへ戻れる。
  遷移後に見出しへ focus が移り、戻る操作でスクロール位置が戻る。
- 検証: V1、V2、`pnpm -C web e2e parity/shell.spec.ts -g "parity: \* 未定義パス"`、
  `pnpm -C web e2e shell/mount.spec.ts`、`pnpm -C web mobile-audit --only "/no-such-page"`

### P2-03 shell の server state と遅延表示の部品
- 作るもの: `['health']`・`['daemon','rest']`・`['inbox']` の query とナビのバッジ（select で導出）、celeris 断の
  バナー、取得の状態を出す共通の枠（skeleton、「更新中」、1 s / 5 s の表示、再試行）。
- 受け入れ条件: C1〜C4、S3、S4、S6。バッジの値が不明のとき 0 と表示しない。偽 daemon を止めるとバナーが出て、
  shell・ナビ・login は使える。起こすと表示中の key だけを取り直す。補完取得は画面が見えていて認証済みの間だけ、
  重ならずに走る（`daemon/rest` 5 s、`inbox` 15 s）。
- 検証: V1、V2、`pnpm -C web e2e parity/shell.spec.ts -g "parity-x: daemon 停止中"`、`pnpm -C web test components/`

### P2-04 SSE の transport
- 作るもの: `web/api/realtime/` の購読（shell で 1 本）、フレームの envelope と event の種類の検証、cursor、
  再接続（`Last-Event-ID` / `after_id`、上限付き backoff）、復帰（visibilitychange・pageshow・online・focus を束ねる）、
  接続状態の表示。
- 受け入れ条件: C1〜C4。`hello` と `heartbeat` は server state を invalidate しない。`daemon` は `['daemon','stream']`
  だけを更新し、`['daemon','rest']` を上書きしない。401 で再接続を止める。不正な payload を画面に出さない。
  タスク詳細を開いても購読は増えない。
- 検証: V1、V2、`pnpm -C web test api/realtime/transport`

### P2-05 SSE → invalidate の対応表
- 作るもの: `web/api/realtime/invalidation-map.ts`（フレーム 5 種と `task.event` の 36 種 → key の集合。
  ADR-0081 D6 の表のとおり）、key ごとの 250 ms の束ね、project の解決と fallback、`reset` の再同期。
- 受け入れ条件: C1〜C4。schema の event の種類が対応表に 1 つでも無ければテストが落ちる。36 種すべてに fixture の
  テストがある。`worker_progress` で project・設定・一覧を取り直さない。重複と逆順のイベントで二重に取り直さない。
  1 s に 20 件の burst と遅延 10 s を重ねても、key ごとの進行中の取得は 1 本で、要求が増え続けない。
  `reset` は全 server query を stale にし、active なものだけ取り直す。
- 検証: V1、V2、`pnpm -C web test api/realtime/invalidation-map`、
  `pnpm -C web e2e parity/realtime.spec.ts -g "parity: /events|parity-x: SSE"`

### P2-06 時刻帯と storage の規律
- 作るもの: 時刻の表示（ブラウザの時刻帯、相対時刻は `hello.now` との差で補正）、表示の好みの保存、
  storage の検査（route の一覧から全画面を開き、localStorage・sessionStorage・IndexedDB・Cache Storage を調べる）。
- 受け入れ条件: C1〜C4。保存するのは表示の好み（時刻帯、テーマ）だけ。token・API 応答・本文・ログ・下書きが storage に
  無い。service worker を登録しない。画面が増えても検査が自動で対象にする。
- 検証: V1、V2、`pnpm -C web e2e parity/shell.spec.ts -g "parity-x: timezone|parity-x: storage"`

### P2-07 画面の共通検査（V3）の枠
- 作るもの: `web/e2e/support/screens.ts`（画面の一覧: path、fixture、見出し）、`latency/transition.spec.ts`、
  `realtime/refetch-scope.spec.ts`、`a11y/axe.spec.ts`、`web/scripts/mobile-audit.mjs`（幅 360 / 390 / 412 / 1440、
  44×44、横溢れ、名前、構造、focus の順）、`web/scripts/screenshots.mjs`。
- 受け入れ条件: C1〜C4。P2-02 の枠だけの画面で S1〜S4 が通る。計り方は baseline と同じ（クリックから URL まで、
  見出しの表示まで、データの表示までを別々に記録）。`screens.ts` に無い route があれば `check:parity` が落ちる。
- 検証: V1、V2、V3（`/tasks` と `/inbox`）

## 7. Phase 3 — 中核の画面

Phase の目的: 毎日使う画面（Console、受信箱、認可、タスク、run とファイル、報告）を移す。
ここからの画面のタスクは、C1〜C4 と S1〜S8 の全部が受け入れ条件で、下には画面固有の条件だけを書く。
画面の実装は `web/features/<domain>/` に置き、`routes/` には配置だけを書く。

### P3-01 Console の中継と cache
- 作るもの: gateway の `/api/console/stream`（daemon の `/console/stream`、scope と since を保つ）、
  `['console',scope,conversationId]` の cache（REST の初回取得と stream の差分を同じ cache に入れる）、送信と新しい会話の mutation。
- 固有の条件: block の順序が保たれ、重複しない。切断の後は続きから再開する。`/events` の再取得の処理と混ざらない。
  未認証は 401。
- 検証: V1、V2、`pnpm -C web test features/console server/console`、
  `pnpm -C web e2e parity/console.spec.ts -g "parity: /console/stream|parity: /console/new-conversation"`

### P3-02 Console の画面
- 作るもの: `/` と `/org/:id` の Console、下端に固定した composer、progress の「すべて見る」。
- 固有の条件: composer は safe-area を守り、キーボードが出ても隠れない。入力中の文と展開の状態は daemon の再取得と
  画面遷移で消えない。送信は 202 を受けるまで二重に出ない。`/org/:id` は宛先がその人になる。
- 検証: V1、V2、V3（`/`、`/org/cos`）、
  `pnpm -C web e2e parity/console.spec.ts -g "parity: / Console|parity: /org/:id Console|parity: runs/:runId/events"`

### P3-03 受信箱と操作の共通部品
- 作るもの: `/inbox`（区画ごとの approve / reject / answer / cancel）、操作の結果・409・422・二重送信の防止の共通部品、
  Markdown の表示、成果物の preview。
- 固有の条件: 複数の task への操作を直列に送り、途中の失敗を項目ごとに出す。2 つのページで同じ項目を操作した
  ときの 409 で、該当 key を取り直して理由を出す。note と answer の入力は失敗の後も残る。
- 検証: V1、V2、V3（`/inbox`）、`pnpm -C web e2e parity/inbox.spec.ts -g "parity: /inbox|parity-x: 409"`

### P3-04 認可
- 作るもの: `/approvals`（判定、常設ルールの作成と削除）。
- 固有の条件: バッジは `GET /daemon` の値から出す。SSE の生 snapshot の `approvals_pending: 0` でバッジが消えない。
- 検証: V1、V2、V3（`/approvals`）、`pnpm -C web e2e parity/inbox.spec.ts -g "parity: /approvals"`

### P3-05 タスクの一覧
- 作るもの: `/tasks`（絞り込み・並び・検索は search param、続きの読み込み）。
- 固有の条件: 幅 360 で表が横溢れしない。絞り込みを変えても入力の focus が外れない。続きを読み込んだ後に
  イベントで取り直しても、読み込んだ分が消えない。
- 検証: V1、V2、V3（`/tasks`）、`pnpm -C web e2e parity/tasks.spec.ts -g "parity: /tasks 絞り込み"`

### P3-06 タスクと計画の作成
- 作るもの: `/tasks/new`（受け入れ条件の 4 型）、`/plans/new`。
- 固有の条件: 成功で `/tasks/:id` へ移る。422 は欄の横に出て入力が残る。条件の行の追加と削除のボタンが 44×44 以上。
- 検証: V1、V2、V3（`/tasks/new`、`/plans/new`）、
  `pnpm -C web e2e parity/tasks.spec.ts -g "parity: /tasks/new|parity: /plans/new"`

### P3-07 依存グラフ
- 作るもの: `/graph`（`?root=`、`?depth=`）。
- 固有の条件: SVG が枠に収まり、ページは横溢れしない。
- 検証: V1、V2、V3（`/graph`）、`pnpm -C web e2e parity/tasks.spec.ts -g "parity: /graph"`

### P3-08 タスク詳細の枠と overview・timeline
- 作るもの: `/tasks/:id` の枠と `?tab=overview|timeline`（表示だけ）。
- 固有の条件: tab は search param で、切替は取得を待たない。その task のイベントでは detail と timeline だけを
  取り直す。他の task のイベントでは取り直さない。R23 の状態は `実装中`。
- 検証: V1、V2、V3（`/tasks/:id?tab=overview`、`?tab=timeline`）、`pnpm -C web test features/tasks`、
  `pnpm -C web e2e parity/task-detail.spec.ts -g "parity: /tasks/:id 表示"`

### P3-09 タスク詳細の操作（判断）
- 作るもの: approve / reject / answer / cancel / retry / edit / comment / reopen と判断パネル。
- 固有の条件: `expected_status` を送り、409 で取り直す。判断パネルがスマホで操作でき、固定の要素が本文を隠さない。
- 検証: V1、V2、V3（`/tasks/:id`）、`pnpm -C web e2e parity/task-detail.spec.ts -g "parity: /tasks/:id 判断"`

### P3-10 タスク詳細の操作（実行と routing）
- 作るもの: rereview / promote / phase_gate / execution_decompose、routing パネル、execution の表示。
- 固有の条件: promote と phase_gate は結果が確定するまで成功と出さない。execution のイベントで execution の key を取り直す。
- 検証: V1、V2、V3（`/tasks/:id`）、`pnpm -C web e2e parity/task-detail.spec.ts -g "parity: /tasks/:id 実行"`

### P3-11 作業ツリーと成果物の viewer
- 作るもの: `/tasks/:id/files`、`/artifacts`、成果物の一覧と表示の部品（P3-03 の preview と同じ部品）。
- 固有の条件: path と選択したファイルは URL にある。不正な path は枠の中のエラー。長い path と長い行は枠の中で折り返すか
  スクロールする。HTML の成果物は実行されない。大きいファイルを全部は読み込まない。
- 検証: V1、V2、V3（`/tasks/:id/files`、`/artifacts`）、
  `pnpm -C web e2e parity/runs-files.spec.ts -g "parity: /tasks/:id/files|parity: /artifacts"`

### P3-12 run ログ
- 作るもの: `/tasks/:id/runs/:runId`（会話形式、harness ごとの adapter、`offset` の追い掛け）。
- 固有の条件: 表示の行数が fixture の行数と一致する。実行中の run は上限付きの polling で追記を追い、buffer に上限がある。
  終わった run は取り直さない。追記で読んでいる位置が動かない。
- 検証: V1、V2、V3（`/tasks/:id/runs/:runId`）、`pnpm -C web e2e parity/runs-files.spec.ts -g "parity: /tasks/:id/runs/:runId"`

### P3-13 変更と、5 tab の結合
- 作るもの: `/tasks/:id/changes`（integrate、pr_merge）、`?tab=changes|files|artifacts` に P3-11 の部品を置く。
- 固有の条件: 差分の横スクロールは枠の中に閉じる。5 tab と全 intent を 1 本のテストで通す。
- 検証: V1、V2、V3（`/tasks/:id/changes`、`/tasks/:id?tab=changes`、`?tab=files`、`?tab=artifacts`）、
  `pnpm -C web e2e parity/task-detail.spec.ts -g "parity: /tasks/:id 5 tab|parity: /tasks/:id/changes"`

### P3-14 報告
- 作るもの: `/reports`（`?filter=`、`?level=`、既読、通知の試験、行の展開）、`/reports/:id` の取得。
- 固有の条件: 報告は `GET /daemon` の値から出す。SSE の生 snapshot の `reports: None` で一覧が消えない。
  展開した行は取り直しても閉じない。
- 検証: V1、V2、V3（`/reports`）、`pnpm -C web e2e parity/reports.spec.ts -g "parity: /reports"`

### P3-15 ブラウザ通知
- 作るもの: shell の通知の見張り、通知の許可の部品。
- 固有の条件: 同じ報告を 1 回だけ通知する（2 つのタブを開いても 1 回）。通知した記録は daemon に送る。
  報告の本文を storage に置かない。許可が無いときは何も出さず、画面の表示は変わらない。
- 検証: V1、V2、`pnpm -C web e2e parity/reports.spec.ts -g "parity-x: 通知"`

## 8. Phase 4 — 管理の画面

Phase の目的: 案件・ボード・組織・知識・運用設定を移す。条件の書き方は Phase 3 と同じ。

### P4-01 案件の一覧と作成
- 作るもの: `/projects`（一覧、絞り込み、作成）。
- 固有の条件: 作成の成功で `/projects/:id` へ移る。422 は欄の横に出る。
- 検証: V1、V2、V3（`/projects`）、`pnpm -C web e2e parity/projects.spec.ts -g "parity: /projects 一覧"`

### P4-02 案件詳細の表示（計画 DAG・仕事の木）
- 作るもの: `/projects/:id` の表示（概要、計画の DAG、仕事の木、成果物の一覧）。
- 固有の条件: DAG と木は枠の中でスクロールし、ページは横溢れしない。project に属さない task のイベントでは
  取り直さない。所属が分からないイベントでは project の集計 key だけを stale にする。R10 の状態は `実装中`。
- 検証: V1、V2、V3（`/projects/:id`）、`pnpm -C web test features/projects`、
  `pnpm -C web e2e parity/projects.spec.ts -g "parity: /projects/:id 表示"`

### P4-03 案件詳細の操作（案件と作業場所）
- 作るもの: `project_edit` `project_status` `project_pause` `project_resume` `project_cancel` `project_archive`
  `project_unarchive` `project_workspace_save` `project_workspace_clear` `task_create`。
- 固有の条件: cancel と archive は確認のダイアログを通す。
- 検証: V1、V2、V3（`/projects/:id`）、`pnpm -C web e2e parity/projects.spec.ts -g "parity: /projects/:id 案件の操作"`

### P4-04 案件詳細の操作（計画と途中目標）
- 作るもの: `project_plan` `project_plan_decide`、`milestone_create` `milestone_status` `milestone_pause`
  `milestone_resume` `milestone_cancel` `milestone_decide`。
- 固有の条件: `project_plan_proposed` / `project_plan_decided` のイベントで、その project の detail と plan を取り直す。
- 検証: V1、V2、V3（`/projects/:id`）、`pnpm -C web e2e parity/projects.spec.ts -g "parity: /projects/:id 計画と途中目標"`

### P4-05 案件詳細の操作（リポジトリ）
- 作るもの: `repo_create` `repo_patch` `repo_delete` `repo_primary`。
- 固有の条件: 全 intent を 1 本のテストで通す。
- 検証: V1、V2、V3（`/projects/:id`）、`pnpm -C web e2e parity/projects.spec.ts -g "parity: /projects/:id 全 intent"`

### P4-06 案件の文書と文書保守
- 作るもの: `/projects/:id/docs`（init、save、delete）、`/projects/:id/docs/maintenance`（起動と結果）。
- 固有の条件: 編集中の文は取り直しで消えない。編集欄が幅に収まる。
- 検証: V1、V2、V3（`/projects/:id/docs`、`/projects/:id/docs/maintenance`）、
  `pnpm -C web e2e parity/projects.spec.ts -g "parity: /projects/:id/docs"`

### P4-07 ボード
- 作るもの: `/board`（案件の選択、6 列、絞り込みは search param、カードの編集）。
- 固有の条件: 6 列の横スクロールは列の枠の中に閉じる。背景から戻ったとき、表示中の key だけを取り直す。
- 検証: V1、V2、V3（`/board`）、`pnpm -C web e2e parity/projects.spec.ts -g "parity: /board"`

### P4-08 組織の木と課の操作
- 作るもの: `/org`（木、`?selected=`、`org_create` `org_patch` `org_delete`）、`/org/secretary` → `/org/cos`。
- 固有の条件: スマホでは木と詳細が縦に積まれる。R07 の状態は `実装中`。
- 検証: V1、V2、V3（`/org`、`/org?selected=cos`）、`pnpm -C web e2e parity/org.spec.ts -g "parity: /org/secretary"`

### P4-09 組織の skill の操作
- 作るもの: `skill_mount` `skill_unmount`、profile と skill の Markdown。
- 固有の条件: mount と unmount の後、その課の詳細と skill の一覧だけを取り直す。
- 検証: V1、V2、V3（`/org?selected=cos`）、`pnpm -C web e2e parity/org.spec.ts -g "parity: /org 木"`

### P4-10 知識と候補
- 作るもの: `/knowledge`（検索、閲覧、save）、`/knowledge/inbox`（accept、reject）。
- 固有の条件: 検索語と開いている知識は search param にある。保存の 422 は欄の横に出て入力が残る。
- 検証: V1、V2、V3（`/knowledge`、`/knowledge/inbox`）、
  `pnpm -C web e2e parity/knowledge.spec.ts -g "parity: /knowledge 検索|parity: /knowledge/inbox"`

### P4-11 skill
- 作るもの: `/knowledge/skills`（`?create=1`、`?name=`、`&edit=1`、`skill_put`、`skill_delete`）。
- 固有の条件: 検証エラーは欄の横に出る。files の入力がスマホで使える。
- 検証: V1、V2、V3（`/knowledge/skills` と search param の 3 通り）、
  `pnpm -C web e2e parity/knowledge.spec.ts -g "parity: /knowledge/skills"`

### P4-12 daemon と provider
- 作るもの: `/daemon`（状態、replay）、`/providers`（create、patch、delete、check）。
- 固有の条件: daemon の tick では取り直さず、表示中だけ上限付きの polling と操作の後の取り直しで更新する。
- 検証: V1、V2、V3（`/daemon`、`/providers`）、`pnpm -C web e2e parity/ops.spec.ts -g "parity: /daemon|parity: /providers"`

### P4-13 account とログイン
- 作るもの: `/accounts`（create、delete、check、`login_start` `login_code` `login_cancel`）。
- 固有の条件: デバイス認証の途中の状態は取り直しで消えない。R31 の状態は `実装中`。
- 検証: V1、V2、V3（`/accounts`）、`pnpm -C web e2e parity/ops.spec.ts -g "parity: /accounts ログイン"`

### P4-14 secret・LLM source・MCP クライアント
- 作るもの: `secret_put` `secret_delete`、LLM source と MCP クライアントの表示、`/mcp/clients/:id/calls` の取得。
- 固有の条件: 秘密の値の欄は値を表示せず、自動補完しない。送った値を cache・URL・storage・ログに残さない。
- 検証: V1、V2、V3（`/accounts`）、`pnpm -C web e2e parity/ops.spec.ts -g "parity: /accounts|parity: mcp/clients"`

### P4-15 クラスタ
- 作るもの: `/clusters`（`cluster_connect` `cluster_connect_code` `cluster_connect_cancel`、
  `cluster_work_dir_save` `cluster_work_dir_clear`）。
- 固有の条件: 接続の途中の状態（コードの入力待ち）は取り直しで消えない。
- 検証: V1、V2、V3（`/clusters`）、`pnpm -C web e2e parity/ops.spec.ts -g "parity: /clusters"`

### P4-16 リリース
- 作るもの: `/releases`（`release_promote`、非同期の結果と失敗の表示）。
- 固有の条件: 昇格は結果が確定するまで成功と出さない。昇格で gateway が入れ替わっても、画面は再接続して結果を出す。
- 検証: V1、V2、V3（`/releases`）、`pnpm -C web e2e parity/ops.spec.ts -g "parity: /releases"`

### P4-17 ヘルプ
- 作るもの: `/help`（6 節、見出しの id、アンカー）。
- 固有の条件: この画面は daemon を呼ばない。アンカー付きの URL を開くとその節へ移る。
- 検証: V1、V2、V3（`/help`）、`pnpm -C web e2e parity/help.spec.ts -g "parity: /help"`

## 9. Phase 5 — 横断 gate

Phase の目的: 全画面がそろった状態で、遅延・security・mobile を通しで検査する。ここで落ちた項目は、
その画面のタスクと同じ大きさの修正タスクを足して直す。

### P5-01 遅延の gate
- 作るもの: 全画面の遷移を遅延 0 / 5 / 10 s で測る検査と、baseline と同じ 3 点（7 経路の遷移、SSE のイベントごとの
  再取得の本数、遅延 5 s + tick 2 s の `/tasks` → `/tasks/:id`）の計測。結果は run の artifacts に JSON で残す。
- 受け入れ条件: C1〜C4。全画面で S1 と S2。遅延 5 s + tick 2 s で遷移が終わり、打ち切られる要求が増え続けない。
  project の fallback が起きた回数を結果に含める。計っていない値を実績として書かない。
- 検証: V1、V2、`pnpm -C web e2e latency/`、`pnpm -C web e2e realtime/`、
  `pnpm -C web e2e parity/latency-gate.spec.ts -g "parity-x: 遅延 10 s"`

### P5-02 security の gate
- 作るもの: 全経路（asset、HTML、404、`/api/*`、`/files/*`、`/events`、`/api/console/stream`）の Host・CSRF・session・
  header・token の検査と、全画面を開いた後の storage の検査。
- 受け入れ条件: C1〜C4。X1〜X6 と X8 のテストが全画面・全経路を対象にして通る。偽 daemon を止めても login と
  shell が返る。
- 検証: V1、V2、`pnpm -C web e2e parity/gateway.spec.ts parity/gateway-auth.spec.ts parity/gateway-relay.spec.ts`、
  `pnpm -C web e2e parity/shell.spec.ts -g "parity-x: storage"`

### P5-03 mobile・a11y の gate
- 作るもの: 全画面（現行の mobile-audit が見ていない 11 本を含む）の 4 幅の監査と axe。
- 受け入れ条件: C1〜C4。全画面で S3 と S4。全画面の 4 幅のスクリーンショットが run の artifacts にある。
- 検証: V1、V2、`pnpm -C web mobile-audit`、`pnpm -C web e2e a11y/`、
  `pnpm -C web e2e parity/mobile-gate.spec.ts -g "parity-x: axe"`

### P5-04 parity の総点検
- 作るもの: parity matrix の Phase 1〜5 の全行の状態の確認と、`web/e2e/parity/` の全テストの通し実行。
- 受け入れ条件: C1〜C4。R01〜R42 と X1〜X14、X16 が `完了（<commit>）`。行の数は 42 のまま。
- 検証: V1、V2、`pnpm -C web e2e parity/`、`pnpm -C web check:parity --require-phase 5`、
  `grep -c '^| R[0-9][0-9] |' docs/web/feature-parity.md`（42）

## 10. Phase 6 — 並行運用と配信切替

Phase の目的: gui/ を動かしたまま web/ を本番で使い、人が承認したら配信を切り替える。
**この Phase は人の判断（H2、H3、H6、H7）が出るまで始めない**。gui/ は切替の後も配布と起動ができる状態で残す。

### P6-01 web/ の配布物
- 作るもの: `pnpm -C web release`（build、gateway、lockfile、unit の雛形をまとめた配布物）。
- 受け入れ条件: C1〜C4。展開して `pnpm install --prod --offline` だけで起動する。`/healthz` が名前と release を返す。
- 検証: V1、V2、`pnpm -C web e2e parity/cutover.spec.ts -g "parity-x: web の配布物"`
- rollback: commit を revert する。

### P6-02 並行運用の unit と selfdeploy への組み込み
- 作るもの: 並行運用の ADR（port、認証、release への入れ方。人の決定を書く）、`deploy/systemd/` の web 用の unit、
  selfdeploy の build と検査に web/ の段を足す。
- 受け入れ条件: C1、C4。gui/ の unit と `:7700` の配信を変えない。selfdeploy の既存の検査と gui/ の段が通る。
  staging で gui/ と web/ が同時に動き、実 celeris に対して `web/e2e/parity/` の読み取りだけのテストが通る。
  web/ の段が落ちても、gui/ だけのリリースの昇格を止めない。
- 検証: V1、`cargo test --workspace`、`cargo clippy --workspace -- -D warnings`、`scripts/selfdeploy/tests/` の全テスト、
  `scripts/selfdeploy/verify.sh <sha12>`（staging）
- rollback: web 用の unit を止めて disable し、commit を revert する。gui/ には触れていないので配信は変わらない。

### P6-03 dogfood
- すること: 人が web/ を本番の daemon に対して日常の仕事に使う。見つかった問題は 1 件ごとに小さい修正タスクにする。
- 受け入れ条件: 人が決めた期間、人が決めた合格条件を満たす。期間中の問題と対応を `docs/PROGRESS.md` に残す。
  スマホと PC の両方で使う。
- 検証: `curl -s http://127.0.0.1:<web の port>/healthz`、`pnpm -C web e2e parity/`（修正のたびに）、人の確認
- rollback: web 用の unit を止める。利用者は gui/（`:7700`）を使い続ける。

### P6-04 配信切替
- すること: 利用者の入口を web/ にする。gui/ は別の port で起動できる状態で残す。切替と戻しの手順を
  `docs/selfdeploy.md` に書く。**人の承認の後に行う**。
- 受け入れ条件: parity matrix の全行が `完了`（X15 を含む）。staging で切替と戻しを 1 回ずつ通す。戻しは 1 コマンドで、
  gui/ の配信に戻る。切替の後も gui/ の test・typecheck・build が通る。
- 検証: V1、`pnpm -C web check:parity --require-phase 6`、`scripts/selfdeploy/tests/` の全テスト、
  `curl -s http://127.0.0.1:7700/healthz`、人の承認
- rollback: 戻しの手順で入口を gui/ に戻す。

### P6-05 切替後の観察と戻しの確認
- すること: 人が決めた期間、web/ を入口として使う。戻しの手順が動くことを期間の終わりにもう一度確かめる。
- 受け入れ条件: 期間中に戻しを要する問題が無い。結果と未解決事項を `docs/PROGRESS.md` に残す。
- 検証: `curl -s http://127.0.0.1:7700/healthz`、`pnpm -C web e2e parity/`、人の確認

## 11. Phase 7 — gui/ の削除（この計画では実行しない）

gui/ の削除・rename、`celeris-gui@` の unit と selfdeploy の gui/ の段の廃止は、**この計画のタスクに含めない**。
cutover（P6-04）と切替後の観察（P6-05）が終わった後に、人が承認して別のタスクとして起こす。
Phase 1〜6 のどのタスクも、その準備としての gui/ の変更をしない。

### P7-00 gui/ の削除の起票
- すること: 下の開始条件がそろったことを人に報告する。削除するかどうか、いつするかは人が決める。
- 依存: P6-05
- 開始条件（受け入れ条件）: parity matrix の全行が `完了`。P6-05 の期間が終わり、戻しを要する問題が無い。
  gui/ を参照している場所（`deploy/`、`scripts/selfdeploy/`、`scripts/sync-gui-docs.sh`、`docs/`）の一覧がある。
  人の承認がある。
- 検証: `pnpm -C web check:parity --require-phase 6`、`git grep -n "gui/" -- deploy scripts docs ':!docs/progress'`、
  `test -d gui`（起票の時点で gui/ が残っている）
