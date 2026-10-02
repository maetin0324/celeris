# ADR-0081: Web GUI の遷移を daemon の遅延から切り離す SPA と薄い gateway

---
tasks: [01M3MS2JRDJ4GM0D9VN9PJCB6B]
---

- Date: 2026-09-28
- Status: Accepted
- Supersedes: ADR-GUI-0002（GUI ADR-0002）の D1〜D3（framework mode、SSR、loader/action を server state の正本とし TanStack Router/Query を採用しない判断）、D5 の SSR adapter と D6 の SSR build 配布に依存する部分、および §6 の SPA 不採用判断。対象は [gui 側](../../gui/docs/adr/0002-frontend-stack.md) と [docs/gui 側](../gui/adr/0002-frontend-stack.md) の両方。ルート側 ADR-0002（state machine）は対象外。
- 適用先: 新設するリポジトリ直下の `web/`。既存 `gui/` は段階移行が完了するまで存続する。

## 文脈

現行 GUI は React Router framework mode の SSR loader/action を BFF とし、認証済みの root loader が `/health` → `/inbox` → `/daemon` を直列取得する。子画面の loader も daemon の応答を待つ。ページの遷移とデータの取得完了が結び付き、遅い daemon がナビゲーションを止める。

さらに root の `useCelerisStream` は `task.event`、`daemon`、`reset` を区別せず、250 ms のスロットルで表示中の全 loader を再検証する。既定の dispatcher tick は 2 秒であり、daemon イベントだけでも繰り返し広い再取得が発生する。これは構造の確認であり、実際の遷移時間は Phase 0 の遅延 baseline で別途測る。

旧 ADR の「SPA には BFF が別途必要」という負担は、認証と中継に責務を絞った gateway を明示的に持つことで引き受ける。daemon のドメイン判断をブラウザや gateway に移す決定ではない。

## 決定

### D1. React + Vite SPA と、データを待たないルーティング

- `web/` に React + TypeScript + Vite の SPA を置く。TanStack Router の file-based routing を採用し、path/search params とリンクを型付けする。依存の具体的な版は導入時に互換性とリポジトリの依存管理規約を確認し、lockfile で固定する。
- `routes/` は URL の宣言、純粋な param 検証、feature component の配置に限定する。daemon fetch を待つ blocking な `loader` / `beforeLoad` を禁止し、fetch を await したり、その Promise を返したりしない。`ensureQueryData` を待つ構成も禁止する。preload は non-blocking に Query に依頼して直ちに戻り、失敗を Query の画面内エラーとして扱う。認証判定は gateway のローカル session に基づき、daemon health を遷移条件にしない。
- root shell（ナビ、ヘッダ、Console の枠、接続状態、route outlet）は daemon の online/offline、query 成否、バッジ値によって mount を変えない。root に server state を待つ Suspense を置かない。route 単位のチャンク待ちは outlet 内で表示する。
- 遷移先の見出し・枠・ローディング表示を先に出し、各パネルが Query で取得する。ネットワーク待ち中も戻る/進む、タブ切替、ナビを使える。認証切れは別の境界として扱い、保護データを消してログインへ移る。

### D2. 状態を三つに分ける

| 状態 | 正本 | 例と制約 |
|---|---|---|
| URL state | Router の型付き path/search params | task/project/run ID、タブ、検索、フィルタ、sort、ページ位置。共有・再読み込み・戻る操作で復元する。秘密や入力本文を URL に入れない |
| server state | TanStack Query のメモリ cache | task、runs、project、inbox、認可、報告、設定、ファイル metadata、Console の受信データ。コンポーネントの state/context や Router cache に同じ正本を作らない |
| 一時 UI state | コンポーネント state、必要最小限の UI context | ダイアログ、展開状態、送信前の入力、スクロール、フォーカス。取得データからの表示用導出はよいが、取得結果のコピーを保存し続けない |

QueryClient は認証 session 内で一つ。SSE の受信結果も Query に反映する。接続の成否・再接続タイマー・cursor は transport の一時状態とし、ドメインデータの独立した store を作らない。logout、session 失効・切替時は query/mutation cache と入力・受信バッファを破棄し、進行中の fetch/stream を止める。

機密データ（token、資格情報、task/Console 本文、ログ、成果物、API 応答、フォーム下書き）を localStorage / IndexedDB / sessionStorage に永続化しない。Query persistence、service worker による API/file cache も導入しない。テーマ等の非機密な表示設定のみ保存を許す。ブラウザの session cookie は gateway が発行する HttpOnly cookie とする。

### D3. Express 5 の薄い gateway

`web/server/` は静的 SPA の配信、auth/session、セキュリティ境界、daemon への中継を担う。SSR の画面生成、ドメインロジック、画面別の集約 loader は持たない。

| 境界 | 契約 |
|---|---|
| HTML / bootstrap | HTML は daemon を呼ばず配信する。bootstrap はビルド識別子・ローカル session の認証有無等、daemon 非依存の最小情報のみ。health、counts、reports、provider 一覧、daemon token を含めない。JSON を埋め込む場合は安全に escape し CSP と整合させる |
| 静的配信 | fingerprint 付き asset は immutable。HTML/session 応答は no-store。深い画面 URL の再読み込みは SPA に返す。未知の `/api/*`、file、stream と asset の要求は HTML fallback に流さず 404/適切なエラーにする |
| JSON API | ブラウザは same-origin の `/api/*` のみを利用し、gateway が daemon API に対応付ける。`/api/session` 等の gateway 専用 route は明示予約し、proxy より先に扱う。upstream origin は起動設定で固定し、任意 URL や任意ヘッダの proxy にしない |
| token | ファイルからサーバ側で読み、gateway → daemon の Authorization にだけ付ける。ブラウザの Authorization を daemon 資格情報として転送しない。token は HTML・JS bundle・bootstrap・エラー・要求ログに出さない。クライアント用環境変数にも置かない |
| auth/session | 非 loopback bind は認証必須。ローカルで検証する署名付き session、HttpOnly / SameSite=Strict / HTTPS 時 Secure、期限切れ・logout の契約を引き継ぐ。API/file/SSE は未認証なら 401、保護画面は login へ案内する。login/logout にも CSRF を適用する |
| Host / CSRF | 静的配信・404・エラーも含め全要求に Host 許可リスト検査（不正は 400）。変更系は Origin と Sec-Fetch-Site を検査し拒否は 403。既存のヘッダ無し CLI 要求の契約もテストで固定する。trust proxy は既定 false、転送ヘッダを無条件に信用しない |
| security headers | 全応答に nosniff、Referrer-Policy、CSP / frame 制限。HTML の script/connect は same-origin を基本とし、必要な inline は nonce 等で限定する。本番で script の unsafe-inline を追加しない。API/file/session は no-store |
| file relay | same-origin の許可済み file route に限定する。Range、offset/length/download、206/416、Content-Range、Content-Disposition、size/hash 等の許可ヘッダを保ち、nosniff と安全な Content-Type を維持する。能動的 HTML を同一 origin で実行しない。raw HTML は download または隔離した preview にする。全量 buffering を避け、切断時 upstream を abort する |
| SSE relay | `/events` → daemon `/stream`。`Last-Event-ID`、`after_id`、`task_id` を検証して転送する。`text/event-stream`、no-store、buffering 無効、切断時 abort。長い SSE に通常 JSON の request timeout を適用しない。認証・Host 検査を通し、daemon の 401/503 等は隠さない |
| Console relay | `/api/console/stream` → daemon `/console/stream` を独立して中継する。scope/since を保つ。Console の既存の差分配信・再開契約を移し、汎用 `/events` の再取得処理と混同しない |

通常 JSON fetch は timeout と AbortSignal を持つ。GET のみ上限付きの再試行を許し、401/403/validation error は再試行しない。変更系の自動再送はしない。timeout で実行結果が不明な操作は「失敗」と断定せず、対象を再取得してから再操作できるようにする。gateway 障害、daemon 到達不能、daemon の認証エラーは UI で区別する。

### D4. 型とコードの境界

型の入力はリポジトリの `docs/api/v1/api-v1.schema.json` のみとし、`web/scripts/` の生成処理から `web/api/generated/` に TypeScript を生成する。EventRow/Event/StreamHello/StreamReset も同 schema の定義を使う。schema は API 契約であり、任意 JSON を型 assertion しただけで検証済みにしない。特に SSE は envelope と event discriminant を検証し、不明/不正な値を安全に扱う。

`gui/` の実装・生成型を import しない。既存 GUI は挙動・fixture・テスト観点の参照元とし、web の実装と生成手順は独立させる。client の import graph に `server/`、Node 専用処理、token 設定が入らないことを build と静的検査で確認する。

### D5. domain 別 Query key と更新規律

key factory を domain ごとに一箇所に定義し、ID、正規化した search/filter/page/offset を key に含める。同一 API 応答をバッジ用と画面用に重複 cache しない。例えば inbox の counts は inbox query の select、報告・認可バッジは REST daemon query の select とする。server state の派生値を別の正本にしない。

以下の staleTime は初期値（ミリ秒）。これは「再利用してよい期間」であり、定期取得の周期ではない。イベント invalidate は staleTime より優先する。

| domain / key 例 | staleTime | 取得と更新 |
|---|---:|---|
| `['tasks','list',filters]`、`['tasks','detail',taskId]` | 5,000 | task の変更で対象 detail と該当一覧を無効化。包含を確定できないフィルタ一覧は tasks/list 全体を stale にする |
| `['tasks','timeline',taskId,filters]`、`['tasks','runs',taskId]`、`['tasks','run',taskId,runId]` | 1,000 | task/run の識別子で絞る。表示中のログは offset ごとの bounded buffer とし、終了済みログは 60,000 |
| `['tasks','execution',taskId]`、`['tasks','files',taskId]`、`['tasks','changes',taskId]`、`['tasks','artifacts',taskId]` | 5,000 | execution/file/artifact イベントまたは操作で該当 key を無効化 |
| `['projects','list',filters]`、`['projects','detail',projectId]`、`['projects','tasks',projectId,filters]`、`['projects','plan',projectId]` | 10,000 | project 所属を D6 の規則で解決。docs は `['projects','docs',projectId,path]` で 60,000 |
| `['inbox',filters]`、`['board',filters]`、`['reports',filters]`、`['approvals',filters]` | 5,000 | 一覧・バッジの出典を共有し、関係する task/event/mutation のみで更新 |
| `['daemon','rest']`、`['health']` | 5,000 | REST の正本。可視・認証済みの間、最大 5 秒ごとの非重複取得で通知/認可のイベント欠落を補う。inbox も可視時に最大 15 秒ごとの非重複取得で補う |
| `['daemon','stream']` | 0 | SSE の生 snapshot 専用。REST の代替にしない。受信したフィールドからの表示のみ。REST と合成して「新しい完全 snapshot」を捏造しない |
| `['providers',filters]`、`['accounts',filters]`、`['clusters',filters]`、`['releases',filters]`、`['metrics',filters]` | 10,000 | mutation 後と表示中の上限付き polling。全 daemon tick では更新しない |
| `['org',params]`、`['knowledge',params]`、`['skills',params]`、`['config',params]`、`['mcp',params]` | 60,000 | 変更成功時に関係 domain を無効化。SSE のない外部変更は可視時の定期取得（60 秒）と手動更新で補う |
| `['console',scope,conversationId]` | 0 | REST 初回取得と専用 stream の差分を同じ cache に反映。順序・重複・再開を保証する |

invalidate の通常動作は「一致する cache を stale にし、active query だけ再取得」。inactive query は次の表示時に取得する。SSE の連続到着は key ごとに 250 ms で束ね、進行中の同一 fetch を cancel/restart し続けない。必要なら完了後一度だけ再取得する。daemon の 5〜10 秒遅延下でも要求を積み上げない。

mutation は成功応答の完全な対象データだけを cache に反映し、関係する一覧/集計を targeted invalidation する。全 Query の無条件 invalidate はしない。409/422 後も該当データを再取得し、入力と理由を保つ。承認・認可・昇格等の確定操作は先に成功表示しない。optimistic update を使う場合は rollback と競合時の表示を設計する。

### D6. SSE イベント種類ごとの invalidate 範囲

`/events` は認証済み shell で一本購読する。task 詳細ごとに重複購読しない。EventRow の `task_id` は全 `task.event` に存在するが、`project_id` は `project_plan_proposed` / `project_plan_decided` と `created.task.project_id` にしか存在しない。SSE の server filter は task 単位のみであり、project filter があると仮定しない。

**project の解決規則**: event の明示値 → Query cache の task detail/list に含まれる所属を使う。別の永続対応表は作らない。所属不明のときは projects の list/detail/tasks/plan 集計 key 群を stale にし、active なものだけ取得する（docs は除く）。所属変更の可能性がある編集は旧・新両方、不明ならこの fallback を使う。event handler が所属確認 fetch を await して UI を止めることはない。

表中の `T` は対象 task detail と timeline、`L` は tasks/list・inbox・board、`P` は上記規則で絞る project 集計、`N` は reports・approvals・daemon/rest、`R` は対象 task の runs と該当 run（run_id 不明ならその task の run 群）、`E` は対象 task の execution。timeline は **全 task.event** で対象 task だけ stale にする。実装では表の集合を key factory に展開する。

| SSE frame | invalidate / cache 更新 | 禁止・補足 |
|---|---|---|
| `hello` | cursor と接続時刻を記録。daemon があれば daemon/stream にだけ反映。初回は active query が独立して取得 | hello.daemon も REST daemon の代入に使わない |
| `heartbeat` | transport の生存時刻のみ | server query を invalidate しない |
| `daemon` | daemon/stream の生 snapshot のみ更新 | 全 route 再取得は禁止。reports/approvals_pending/counts の根拠にしない。REST 補完は D5 の独立した周期 |
| `task.event` | 下表の対象 key を束ねて invalidate | event.id と task_id/seq で重複を除き、順序逆転に備える。受信 payload は完全な REST 応答の代用にしない |
| `reset`（cursor_ahead / cursor_too_old） | 新 cursor を採用、受信差分の連続性を破棄し、全認証済み server query を stale にする。active のみ再取得 | 全 domain invalidate の例外。古い一覧は更新待ちと明示し、差分を継ぎ足して完了扱いしない |
| 不明な frame / event.type / 不正 payload | 内容を UI に注入せず契約不一致を表示。識別できる task があればその task と L/P/N、不明なら active server query を再同期 | 再同期は頻度制限し、異常イベントによる取得ループを防ぐ |

| `task.event` の `event.type`（現行 schema の全36種） | invalidate 対象（timeline は共通） |
|---|---|
| `created` | T、L、P。created.task から所属を得る。親があれば親 task の detail/children 相当も対象 |
| `transitioned` | T、L、P、N、E（終端・blocked・途中目標/報告の集計変化） |
| `worker_started` | T、R、E、L、P（途中目標の進行を含む） |
| `worker_progress` | 対象 run の progress/log と対象 task timeline。一覧に進捗を出す場合はその欄も対象 task で更新。全一覧 refetch はしない |
| `artifact_produced` | T、R、対象 task の artifacts/files、成果物一覧、P |
| `worker_finished` | T、R、E、L、P、N、対象 task の changes/files/artifacts、metrics |
| `review_verdict` | T、R、E、L |
| `approval_requested`, `approval_decided`, `approvals_withdrawn` | T、L、N、E、P。task の承認と個別の認可を同一操作と仮定しない |
| `answered`, `question_raised` | T、L、N、R、E |
| `delegated` | T、R、E、L、P と delegated.task_ids の detail（子の作成を反映） |
| `cluster_unavailable`, `cluster_master_exited` | T、L、clusters（特定できる cluster）、daemon/rest |
| `provider_throttled` | T、providers（特定できる provider）、accounts、daemon/rest |
| `retried` | T、L、P と from の task detail/timeline |
| `edited`, `assigned` | T、L、P、E。所属変更時は旧・新 project を対象 |
| `workspace_mode_downgraded`, `workspace_pruned` | T、対象 task の files/changes/artifacts、metrics |
| `routing_decided` | T、R |
| `checkpoint_saved` | T、R、E |
| `execution_planned`, `work_unit_transitioned`, `execution_gated`, `execution_hint_set`, `repair_scheduled` | T、E、R、L、P |
| `quota_estimated` | T、R、E、accounts、providers、metrics、P |
| `work_unit_committed`, `phase_integrated` | T、E、対象 task の changes/files/artifacts、P |
| `work_units_serialized`, `pause_points_resolved` | T、E |
| `phase_reported` | T、E、対象 task の artifacts、L、N、P |
| `project_plan_proposed`, `project_plan_decided` | T、L、N と明示 project_id の detail/plan/tasks/list 集計 |

成果物一覧の key は `['artifacts',filters]`（staleTime 5,000）、log の key は `['tasks','log',taskId,runId,name,offset]` とする。変更通知のない stdout 追記は表示中・実行中 run のみ上限付き polling で補い、worker_progress が全ログ追記を通知するとは仮定しない。

EventSource の自動再接続では Last-Event-ID、作り直す場合はメモリ上の cursor を `after_id` に載せる。visibility/pageshow/online/focus の復帰を一度に束ね、stream を再開し active な stale query を再取得する。cursor が失われた場合は active query を再同期する。接続失敗時は上限付き backoff、切断中は更新停止を表示する。401 の session 失効時は再接続ループを止める。手動再読込でも UI 全体を remount しない。

SSE の生 snapshot は dispatcher が `reports: None` / `approvals_pending: 0` を設定し、GET `/daemon` の handler だけが DB 等から補う。従って SSE で REST cache を上書きしない。認可・報告の全変更に task.event があるとも仮定せず D5 の補完取得を残す。通知は補完済み REST データから導出し、既存の通知許可・重複抑止契約を引き継ぐ。

### D7. 遅延・失敗・スマホの表示契約

- データ未取得はパネル内 skeleton、取得済みの再取得は現在の内容を保ち「更新中」と最終取得時刻を示す。取得失敗を空配列や 0 件に置き換えない。バッジ不明は 0 と区別する。
- 1 秒超の取得には待機表示、5 秒超には「取得に時間がかかっています」と再試行/接続状況を表示する。timeout、offline、API error は対応するパネルに置き、ナビ・入力・別パネルを使えるままにする。変更系は結果確定までその操作の重複送信を防ぐ。
- 既存 URL と search param の意味を維持し、明示的な redirect を除きリンクを壊さない。画面幅 360/390/412px と desktop 1440px を gate にし、縦長のログ・DAG・表でもページ全体の横溢れを防ぐ。タップ領域は最低 44×44px。
- Tailwind CSS 4 と shadcn/ui の Base UI 系部品を採用する。`components/ui/` に部品、CSS variables に色・余白等の token を置く。キーボード操作、focus 復帰、ダイアログの focus trap、label、色だけに依存しない状態表示を検査する。live region はイベントごとの読み上げ洪水を避ける。IME 入力中に送信しない。
- shell の Console 入力や展開状態は daemon 再取得で失わない。画面遷移時の見出し focus とスクロール復元を定め、戻る操作も Playwright で確認する。

### D8. ディレクトリ構成と段階移行

```text
web/
  index.html
  main.tsx
  routes/                 # file-based route、param 検証、feature の配置
  features/               # tasks、projects、console、knowledge 等の画面と操作
  components/             # Shell、共通表示、ui/（Base UI 系 shadcn 部品）
  api/
    generated/            # schema から生成した型
    client.ts             # same-origin fetch、abort、error の共通化
    queries/              # domain 別 key / query options / mutation
    realtime/             # SSE 契約、cursor、invalidation map
  lib/                    # 純粋な変換、URL/UI の補助
  server/                 # Express、auth、security、API/file/SSE relay
  e2e/                    # parity、遅延、security、mobile/a11y
  scripts/                # 型生成、依存境界検査、計測・表示検査
```

世代名を directory、package、route path に使わない（`v2`、`next` 等を追加しない）。既存 API 契約の `docs/api/v1/` はそのまま参照する。巨大な route ファイルへのロジック集積は再現しない。

Phase 0 は本 ADR、全 route の parity matrix、遅延 baseline と実装計画を確定する段階。以降は gateway/shell/Query の基盤から domain ごとに移す。parity matrix の配置は `docs/web/feature-parity.md` とし、各操作の担当 Phase と検証が閉じるまで移行完了と扱わない。`gui/` の削除・rename、配信切替、旧配布の廃止はこの ADR の実装完了と同一視しない。削除は別タスクで人の承認を必要とする。

## 前提の訂正

照合基点は `06e9a03cffe8`。元計画の `0658547a1f9d0997c2a6a7707507543029a17a84` から `gui/` と `docs/api/` に差分はない。ただし計画に書かれた採用方針と、実際に導入済みの状態を区別する。

**ADR 番号の前提を訂正**: premise-check の時点の記述（ルートの ADR は 0077 まで）は誤りで、main `36ea922` では 0080 まであり、ADR-0078 は browser execution capability と SSH master persist の 2 本がある。新しい Web GUI ADR は空き番号 0081 とする。

| 前提・補足 | 現行コードでの確認と本 ADR への反映 |
|---|---|
| Base UI が導入済み | **訂正**: `gui/components.json` は base-nova 設定だが `gui/package.json` / lockfile に Base UI の依存はない。既存部品を Base UI 実装と見なさず、web で導入して挙動を検証する |
| mobile-audit が 360/390/412px を検査 | **訂正**: `gui/scripts/mobile-audit.mjs` の通常 audit は **393×851 のみ**。個別 check script の複数幅撮影と区別し、新 gate では D7 の幅を明示する |
| mobile-audit が全画面を網羅 | **訂正**: `/login`、`/tasks`、`/tasks/new`、`/tasks/:id/files`、`/tasks/:id/changes`、`/tasks/:id/runs/:runId`、`/plans/new`、`/daemon`、`/providers`、`/artifacts`、`/graph` の **11 本が対象一覧にない**。これを新 GUI の合格範囲から落とさない |
| route の単位 | `gui/app/routes.ts` は 42 定義（画面32、resource10）。`/org/secretary` redirect と catch-all を含む。task の tab、knowledge/skills の create/name/edit、org の selected 等の search state も parity の対象 |
| 現行 stack とコード規模 | React Router 8.3.1 framework mode / SSR、React 19.2.8、Vite 8.2.2、Express 5.2.1、Tailwind 4.3.3。TanStack は react-virtual のみ。tasks 詳細 123,564 byte、projects 詳細 74,749 byte、org 65,077 byte、root 39,214 byte で、薄い route への分割が必要 |
| root loader と再検証 | `/health` → `/inbox` → `/daemon` の直列（正常到達時）、既定 timeout 15 秒。SSE は種別を捨て全 loader 再検証、250 ms スロットル、既定 daemon tick 2 秒。復帰時も全再検証。Console は別 stream。D1/D5/D6 で置換する |
| task.event から project が分かる | **訂正**: task_id は常にあるが project_id は上記3種類の経路に限られる。D6 の Query cache 解決と domain 限定 fallback を採用する。EventRow に新フィールドを足す API 変更は本 ADR の前提にしない |
| SSE daemon と GET /daemon が等価 | **訂正**: raw snapshot と補完済み REST 応答は異なる。reports / approvals_pending を生 snapshot から更新するとバッジ・通知が誤る。D5/D6 で cache の契約を分ける |
| ADR-0002 の所在 | **訂正**: `gui/docs/adr/` と `docs/gui/adr/` の2箇所。後者には改名反映がある。どちらも本文を保存して本 ADR への追記のみ行う |
| security / 検査資産 | auth/session、Host、CSRF、security headers、token 秘匿、file/SSE relay は既にある。SPA 化で不要にならない。既存 unit / Playwright / axe / mobile script の観点を web に引き継ぐ |

## 検証と移行の gate

1. 遅延 fixture は daemon JSON 応答に 5 秒/10 秒を挿入する。クリックから URL・遷移先枠表示までと、データ表示までを別々に測る。遅延注入の有無で shell/遷移時間が比例して伸びず、データ待ち中に別画面へ移れることを確認する。絶対的な性能値は baseline と同じ環境で比較し、未計測値を実績として記載しない。
2. SSE 全 frame と全36 event type を fixture で検査する。worker_progress で無関係な project/設定が再取得されないこと、project 不明時の fallback、reset の再同期、daemon 生 snapshot で通知バッジが消えないこと、重複/逆順/再接続を検証する。イベント burst と10秒遅延の組合せで request 数が増え続けないことも gate とする。
3. gateway は token が HTML/bundle/エラーに出ないこと、Host/CSRF/session/security headers が asset・404・API・file・stream 全経路に効くことを検査する。daemon 停止中でも login と HTML/shell を取得できることを確認する。
4. 各画面は parity matrix の主要操作、深い URL、戻る/進む、通知、file viewer、Console、mobile/a11y を閉じる。変更前後のスクリーンショットを指定の成果物ディレクトリに残す。既存 GUI の test/typecheck/build は移行期間の回帰 gate とする。
5. schema から型を再生成して差分がないこと、client に server/token 処理が混入しないこと、gui import がないこと、loader/beforeLoad が daemon 応答を待たないことを検査する。

## 帰結

ページ遷移と daemon 待機を分離でき、イベントの影響範囲を domain/key 単位で検査できる。一方で client の取得・再同期・cache 寿命と、gateway の認証境界を明示的に保守する責任が増える。SSR に依存した非 JavaScript 時の操作性は引き継がず、起動失敗時の静的案内を用意する。daemon の通知契約が不完全な domain には限定した polling が残る。

## 根拠となる現行コード

- [route 一覧](../../gui/app/routes.ts)、[root loader](../../gui/app/root.tsx)、[useCelerisStream](../../gui/app/hooks/useCelerisStream.ts)、[GUI package](../../gui/package.json)
- [gateway 起動と Host 検査](../../gui/server.js)、[auth/session](../../gui/app/auth.server.ts)、[CSRF / security headers](../../gui/app/middleware/security.server.ts)、[token を保持する client](../../gui/app/celeris/client.server.ts)
- [file relay](../../gui/app/routes/files.runs.ts)、[artifact relay](../../gui/app/routes/files.artifacts.ts)、[SSE relay](../../gui/app/routes/events.ts)、[Console relay](../../gui/app/routes/console.stream.ts)
- [API schema](../../docs/api/v1/api-v1.schema.json)、[SSE producer](../../crates/task-api/src/sse.rs)、[REST snapshot 補完](../../crates/task-api/src/handlers.rs)、[dispatcher snapshot](../../crates/task-dispatch/src/dispatcher.rs)、[Event 定義](../../crates/task-core/src/model.rs)
- [mobile-audit](../../gui/scripts/mobile-audit.mjs)、[対象画面 fixture](../../gui/scripts/lib/celeris-fixture.mjs)、[a11y 検査](../../gui/e2e/g5-a11y.spec.ts)

## 付記（2026-10-02）: gateway の dotfiles と web の release 追従

本付記は ADR-0081（web SPA と gateway）と web ADR-W3（`docs/web/adr/web-0003-parallel-operation.md`、unit や docs で ADR-0096 と呼ばれている文書）の両方に同じ内容で置く。

### (A) 事象と原因
- 本番 release `ea86af6307f8` の web を `~/.local/celeris/releases/<sha12>/web/app`（`celeris-web@<sha12>` の WorkingDirectory）から起動すると、`/healthz` は ok だが `/`・`/login`・`/inbox` など全画面が `not found`（404）になった。同じ release を `/var/lib/celeris/web/<sha12>/app` にコピーして起動すると 200 だった。
- 原因: `web/server/app.js` の `res.sendFile(path.join(distDir, "index.html"))` と `express.static(path.join(distDir, "assets"))` は、send の既定 `dotfiles: "ignore"` で動く。root を渡さない `sendFile` は**絶対 path 全体**を dotfiles 判定にかけるため、途中の `.local` を隠しファイルと見なして 404 を返す。開発・試験の path にドットの dir が無かったため表に出なかった。

### (B) 決定: root を渡し、dotfiles は既定のまま
- SPA の HTML は `res.sendFile("index.html", { root: distDir })` で送る（相対名 + root）。
- `express.static` は assets の dir を root にし、`dotfiles` は既定（`ignore`）のまま。`dotfiles: "allow"` は使わない。
- send は root より上の path を dotfiles 判定に入れないので、配置先の path（`.local` 等を含んでも）に依らず動く。dist の中のドットファイル（`.env` 等）は引き続き配信しない。
- 試験: `.local` を含む一時 dir に release 相当の dist を置いて gateway を起動し、`/` と SPA の path（例 `/inbox`）が 200、dist 内の `.env` 等のドットファイルが 404 になることを確かめる。

### (C) web の release 追従
- 事象: 前日（2026-10-01）の dogfood の web は、task の作業場所（staging 成果物）への symlink を持つ release から起動されていた。作業場所の片付けで中身が消え、全画面 404 になった。
- 決定: `scripts/selfdeploy/promote.sh` は昇格に成功した**後**に `scripts/selfdeploy/web-follow.sh <new_sha12> <old_sha12>` を呼ぶ。
- `web-follow.sh` は `celeris-web@<old_sha12>` が active のときだけ動く。新 release の `gate.json` の `web.ok=true` と `web/app/server/index.js` の存在を確かめてから、`celeris-web@<new_sha12>` を起動・有効化し、`celeris-web@<old_sha12>` を停止・無効化する。
- 条件を満たさないとき（旧 web が動いていない・新 release の web 段が不合格・配布物が無い）は何もせず、理由をログに出す。web の追従の失敗で昇格を失敗にしない（D3/H7 の非 blocking を保つ）。

### (D) unit は daemon を起こさない
- `deploy/systemd/celeris-web@.service` から `Wants=celeris@%i.service` を外し、`After=` だけを残す。web の起動が `celeris@<sha12>` を起こして daemon の handoff を引き起こさないため（2026-10-01 の事故）。

### (E) 本番 host の操作は人が行う
- 一時回避の `~/.config/systemd/user/celeris-web@ea86af6307f8.service.d/override.conf` の撤去、`systemctl --user daemon-reload`、web の再起動は、人が `docs/selfdeploy.md` の手順で行う。worker の run は本番 host を操作しない。
- web ADR-W3 D3 の「promote.sh は web/ の unit を起こさない」という既存の記述は、本付記 (C) で上書きする（promote.sh は旧 web が動いているときに限り web を新 release へ追従させる）。
