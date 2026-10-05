---
title: record-r5 look-a — after-r4 目視 A（/ 〜 /org の 16 画面 × 4 幅）
tasks: [01M45TA2085GTJVFNFR8CQ1G43]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# record-r5 look-a — after-r4 目視 A（/ 〜 /org の 16 画面 × 4 幅）

after-r4（`/local/celeris/data/workspaces/01M45PRPACBEP4X1FWPNP4FRMG/wu/fix-r5/artifacts/after-r4/`、撮影は [fix-r5](../fix-r5.md)）のうち、基本画面の前半 16 個（`_` `_accounts` `_approvals` `_artifacts` `_board` `_clusters` `_daemon` `_graph` `_help` `_inbox` `_knowledge` `_knowledge_inbox` `_knowledge_skills` `_login` `_notifications` `_org`）× 1440/360/390/412 の **64 枚**を、1 枚ずつ Read で開いて目視した。before は `/local/celeris/data/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before/`。判定の観点は ui-ux-quality-gate（階層・密度・折返し・切れ・状態表示・タップ領域・a11y の見え方）。検査結果は [gates-r5](../gates-r5.md)。

縮小表示で細部が読めない長い画面は、同じ png を無損失で切り出して等倍で見直した（`_inbox-360.png` 4 分割、`_board-360.png` 上部、`_artifacts-360.png` 上部、`_-1440.png` の x 900–1440 / y 280–480、`_board-1440.png` 上部。切り出しは WU の artifacts `crops/` に置き、repo には入れない）。

## reviewer 4 点のうち担当分の直否

| 点 | after-r4 の file | 直否 | 根拠（見えたもの） |
| --- | --- | --- | --- |
| (a) /artifacts 360 の題名 | `_artifacts-360.png` | **直った** | 題名「複数の画面にまたがる長いタスク名と依存関係を確認する作業 複数の画面にまたがる長いタスク…」が全幅で 2 行、末尾は省略記号。before（`before/_artifacts-360.png`）は案件未選択の空画面で比較対象の表自体が無い。path は `reports/src/very-long-workspace-name/…/report-0.md` が全幅 4 行で途切れず読める（等倍の切り出しでも同じ）。1 行 5〜6 字で 20 行近く折れる姿は無い |
| (d) ホーム 1440 の浮く空枠 | `_-1440.png` | **直った** | 「通知 未読 3 件」button の右（x 900–1440、y 280–480 を等倍で確認）にあるのは「新しい会話」button だけで、枠の下辺だけが浮く要素は無い。「宛先: CoS」「新しい会話」の行は会話枠の上端に留まっている。残り: 会話の先頭 block の見出し行（「あなた → cos」と日時）が枠の上端で上半分だけ切れて見える。末尾追従の scroll 位置によるもので (d) とは別。下の「残った課題」に保留として記す |

(b)（/projects/P1）と (c)（error 変種）は look-b・look-c の担当。

## 見た枚数

- after-r4: **64 / 64 枚**（担当範囲の全部。見ていない枚数 **0**）。
- before: 15 画面 × 1440・360 の **30 枚**を開いて並べた。`_notifications-*` は before に無い（4 幅とも「before なし」）。before の 390・412（15 画面 × 2 = **30 枚**、`before/_-390.png` `before/_-412.png` … `before/_org-390.png` `before/_org-412.png`）は開いていない。after-r4 の 390・412 は 360 と同じ積み方かを 360 と見比べて判定した。

## 判定表（1 枚 1 行）

判定は『可』『要修正』『保留』。『可』には見えた根拠を添える。

### ホーム `/`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_-1440.png` | `before/_-1440.png` | 可 | 判断待ち 5 件の枠（3 件の題名・期限・案件）→「通知 未読 3 件」→ 宛先と「新しい会話」→ 会話 → 入力の順で階層が通る。before は宛先と空の入力欄だけ。(d) の浮く枠は無い（上表）。残り: 先頭 block の見出し行が枠上端で半分切れる（保留、下記） |
| `_-360.png` | `before/_-360.png` | 可 | 判断待ちの 3 行が 1 列に積まれ、「58 分前 まもなく期限」が題名の下の行へ落ちて切れない。「受信箱 判断待ち 5 件」「通知」「新しい会話」「送信」は指で押せる高さ。会話枠は「手順 2 件」「run の作業」が読める |
| `_-390.png` | `before/_-390.png` | 可 | 360 と同じ積み方で、右端の切れ・横 scroll なし。入力欄と「送信」が下に固定 |
| `_-412.png` | `before/_-412.png` | 可 | 390 と同じ。判断待ちの期限 badge と案件名が 1 行に収まる |

### アカウント `/accounts`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_accounts-1440.png` | `before/_accounts-1440.png` | 可 | 「main ログイン済み」の card に 状態/道具/認証情報/最終確認/実行中の run/これまでの run の label: value が並び、確認・ログイン開始・削除（赤）の順。before は「claude-code / ログイン済み / 使用中 0」の 1 行。追加 form・secret・LLM source・MCP クライアントの見出しに説明文が付く。気づき: account card と LLM source/MCP の card が約 580px、form は全幅で幅が 2 種類ある |
| `_accounts-360.png` | `before/_accounts-360.png` | 可 | label と value が縦に積まれ、3 つの button は 44px 級で横に並んで収まる。説明文は折返しで切れない。before は form の下が viewport 外 |
| `_accounts-390.png` | （before 未確認・390） | 可 | 360 と同じ積み方、右端の切れなし |
| `_accounts-412.png` | （before 未確認・412） | 可 | 同上。「状態なし」等の badge 無しで 1 件の card が読める |

### 承認 `/approvals`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_approvals-1440.png` | `before/_approvals-1440.png` | 可 | 説明文 → 認可待ち（1 件、「受信箱で認可を判断する」primary）→ 決めたもの 2 件（「今後も認めた」「今回だけ認めた」badge、回答、依頼元 `ui-ux`、元のタスクを開く、決めた日時）→ 常設ルール 1 件（削除）→ 追加 form（placeholder と例文）。before は 3 節とも「取得に失敗しました。再試行」 |
| `_approvals-360.png` | `before/_approvals-360.png` | 可 | 全て 1 列、badge は題名の上に落ち、「元のタスクを開く」は下線 link。「削除」が全幅の赤 button で目立つが常設ルール 1 件の直下にあり対象は明確。before は失敗の帯 3 つ |
| `_approvals-390.png` | （before 未確認・390） | 可 | 360 と同じ。規則文の例文が 3 行で折り返し、切れなし |
| `_approvals-412.png` | （before 未確認・412） | 可 | 同上 |

### 成果物 `/artifacts`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_artifacts-1440.png` | `before/_artifacts-1440.png` | 可 | 案件 P1 の 12 件が表（タスク / 成果物 / 大きさ / 記録）。タスク列の題名は 2 行で省略、path は等幅で 3 行に折返し。before は案件未選択の空画面。気づき: 「本文をここで見る」button が 12 回繰り返され、行ごとの視線の起点が button に寄る |
| `_artifacts-360.png` | `before/_artifacts-360.png` | 可 | (a) 直った（上表）。各成果物が name・button・path（4 行）・「128 B・日時」の順に積まれ、横 scroll なし |
| `_artifacts-390.png` | （before 未確認・390） | 可 | 題名 2 行＋省略記号、path 4 行。360 と同じ |
| `_artifacts-412.png` | （before 未確認・412） | 可 | 題名 2 行、path 3 行。切れなし |

### ボード `/board`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_board-1440.png` | `before/_board-1440.png` | 可 | 案件・検索・「絞り込む」→「条件を足す」→ 状態 chip（すべて 24 / 待ち 24 / …）→「待ち 24 件」の群見出し付き表（状態 badge・優先度・レベル・担当・種類）。長い題名は 2 行で折り返す。before は 9 項目の filter 枠と「取得に失敗しました」。気づき: 24 行とも 通常 / 標準 / なし / 機能 で列の情報量が低い |
| `_board-360.png` | `before/_board-360.png` | 可 | 等倍で確認: 案件・検索・全幅「絞り込む」、chip は 2 段に折返し、行は 題名 link → 案件 → 実行待ち badge → 「通常・標準・担当 なし」に積まれる。長い題名は 5 行で全文表示（切れなし）。before は filter 枠だけで一覧なし |
| `_board-390.png` | （before 未確認・390） | 可 | 360 と同じ積み方。chip 2 段、右端の切れなし |
| `_board-412.png` | （before 未確認・412） | 可 | 同上 |

### クラスタ `/clusters`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_clusters-1440.png` | `before/_clusters-1440.png` | 可 | 一覧（クラスタ / 接続 / 最終確認 / 最後の切断 / 失敗理由）と 操作 card（接続先・認証・使用中・作業ディレクトリの値、接続 primary、作業ディレクトリ入力と 2 button）。before は「pegasus.example / auth totp / 未接続」の 1 行 |
| `_clusters-360.png` | `before/_clusters-360.png` | 可 | 一覧は card（失敗理由: なし、最終確認 / 最後の切断）、操作は label: value を縦積み。button は 1 列、保存は値が空のため無効色 |
| `_clusters-390.png` | （before 未確認・390） | 可 | 360 と同じ |
| `_clusters-412.png` | （before 未確認・412） | 可 | 同上 |

### daemon `/daemon`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_daemon-1440.png` | `before/_daemon-1440.png` | 可 | 状態 card に「状態なし」badge と「実行管理の状態はまだありません。」、版（aaaaaaaaaaaa（active））、最終取得（10 秒ごとに自動で取り直す注記）。「保存履歴の照合」と「状態を照合」の和名。before は「dispatcher の状態はまだありません。」「replay を実行」。気づき: 「取得: fixture」の診断語が card の外に裸で残る（before も同じ） |
| `_daemon-360.png` | `before/_daemon-360.png` | 可 | badge と文が 2 行に積まれ、版・最終取得が縦に並ぶ。切れなし |
| `_daemon-390.png` | （before 未確認・390） | 可 | 同上 |
| `_daemon-412.png` | （before 未確認・412） | 可 | badge と文が 1 行に収まる |

### 依存グラフ `/graph`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_graph-1440.png` | `before/_graph-1440.png` | 可 | 「起点の task id（root）」「深さ（depth）」の和名 label、「8 ノード / 2 辺」、T1→T2→T3 の矢印と各 node の 実行待ち badge。題名は 1 行省略。before は「取得に失敗しました」 |
| `_graph-360.png` | `before/_graph-360.png` | 保留 | node は読めるが T2 が枠の右端で切れ（「子タ」まで）、横に続くことの手がかり（scroll bar・端のぼかし・「→ 右へ」）が無い。(b) と同じ「枠の横 scroll に手がかりが無い」型。before は失敗の帯だけ |
| `_graph-390.png` | （before 未確認・390） | 保留 | 同上（T2 は「子タスク:」まで） |
| `_graph-412.png` | （before 未確認・412） | 保留 | 同上（T2 は「子タスク: 画面」まで） |

### ヘルプ `/help`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_help-1440.png` | `before/_help-1440.png` | 可 | 目次 chip 6 件 → 6 節が本文幅約 640px で 1 列。状態の表は label / 説明の 2 列、「進んでいる」「終わった」で群分け。before は全幅の枠 6 個に 2 列の状態表 |
| `_help-360.png` | `before/_help-360.png` | 可 | chip は 3 段に折返し、節と表は 1 列。長い説明も切れずに折り返す |
| `_help-390.png` | （before 未確認・390） | 可 | 同上 |
| `_help-412.png` | （before 未確認・412） | 可 | 同上（chip は 3 段） |

### 受信箱 `/inbox`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_inbox-1440.png` | `before/_inbox-1440.png` | 可 | 説明文 → 案件・種類の filter → 「判断待ち（5）」と内訳（決定 1 / 計画の承認 1 / 失敗 1 / 認可 1 / 知識の候補 1）→ 5 件。各件に 種類 badge・題名・理由欄・選択肢（推奨は primary、取り下げ系は赤）・推奨・期限（「期限切れ」badge）・案件・「止めている範囲と関連」。before は 4 枠とも「ありません。」 |
| `_inbox-360.png` | `before/_inbox-360.png` | 可 | 等倍で 4 分割して確認: 選択肢 button は 1 列で各 44px 級、補足文は button の直下、期限 badge は題名側に残らず推奨行の下へ。切れ・横 scroll なし。before は空の 4 枠 |
| `_inbox-390.png` | （before 未確認・390） | 可 | 360 と同じ積み方。内訳行が 1 行に収まる |
| `_inbox-412.png` | （before 未確認・412） | 可 | 同上 |

### 知識 `/knowledge`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_knowledge-1440.png` | `before/_knowledge-1440.png` | 可 | 候補 / 手順書（skills）の link、検索、「検索結果」表（タイトル / 出典 / scope / 更新日）と 1 件、「1 件」。before は「知識を選択してください。」の空枠が右に大きく残る。after に空枠は無い |
| `_knowledge-360.png` | `before/_knowledge-360.png` | 可 | 検索欄と「検索」が 1 行、結果は label: value の縦積み。before は空枠が残る |
| `_knowledge-390.png` | （before 未確認・390） | 可 | 同上 |
| `_knowledge-412.png` | （before 未確認・412） | 可 | 同上 |

### 知識の候補 `/knowledge/inbox`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_knowledge_inbox-1440.png` | `before/_knowledge_inbox-1440.png` | 可 | 「1 件。採用すると取り込み先のページとして正本に入ります。」→ card（題名、path、出典 / scope / 更新日 / 提案された取り込み先、「本文は見出しだけです。」、取り込み先入力、採用・却下）。before は題名が 2 回出て情報の並びが無い |
| `_knowledge_inbox-360.png` | `before/_knowledge_inbox-360.png` | 可 | 縦積みで切れなし。採用・却下は横並びで押せる大きさ |
| `_knowledge_inbox-390.png` | （before 未確認・390） | 可 | 同上 |
| `_knowledge_inbox-412.png` | （before 未確認・412） | 可 | 同上 |

### 手順書（skills） `/knowledge/skills`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_knowledge_skills-1440.png` | `before/_knowledge_skills-1440.png` | 可 | h1 が「手順書（skills）」、説明文、「手順書の一覧」表（名前 / 状態（登録済み・配送なし badge）/ 更新日）と「1 件」。before は h1「skills」と「skill を選択してください。」の空枠 |
| `_knowledge_skills-360.png` | `before/_knowledge_skills-360.png` | 可 | 名前 link の下に badge 2 つと説明。空枠なし |
| `_knowledge_skills-390.png` | （before 未確認・390） | 可 | 同上 |
| `_knowledge_skills-412.png` | （before 未確認・412） | 可 | 同上 |

### ログイン `/login`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_login-1440.png` | `before/_login-1440.png` | 可 | 題名の下に「ログイン後はホームを開きます。」、card の中に パスワード（focus ring 付き）と「ログイン」primary。before は説明なし・黒 button |
| `_login-360.png` | `before/_login-360.png` | 可 | card が全幅、入力と button が 44px 級 |
| `_login-390.png` | （before 未確認・390） | 可 | 同上 |
| `_login-412.png` | （before 未確認・412） | 可 | 同上 |

### 通知 `/notifications`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_notifications-1440.png` | before なし | 可 | 説明文、「受信箱を開く」「すべて既読にする」、表示・種類・案件の filter と「未読 3 件」、通知 4 件（未読 badge・種類 badge・束の件数・最終日時・題名・要約・関連 link・「既読にする」）、既読 1 件は薄く。「通知の届け方」に拒否状態と「通知を試す」 |
| `_notifications-360.png` | before なし | 可 | filter は 2 段、各通知は縦積みで「既読にする」が本文の下に 44px 級。切れなし |
| `_notifications-390.png` | before なし | 可 | 同上 |
| `_notifications-412.png` | before なし | 可 | 同上 |

### 組織 `/org`

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `_org-1440.png` | `before/_org-1440.png` | 保留 | 本体（組織の木）が写っていない。赤の帯「取得に失敗しました。表示を更新できません。再取得してください。」と「再試行」だけ。before も「取得に失敗しました。再試行」。[web-ux.md](../../2026-10-04-web-ux.md) の既知事項どおり既定 fixture に `/api/v1/org` が無いためで画面の退行ではないが、after-r4 からは組織画面の UI を判定できない。失敗の帯そのものは理由・影響・次の一手の 3 点が揃い、左の縦線と薄い赤地で状態が分かる |
| `_org-360.png` | `before/_org-360.png` | 保留 | 同上。帯は 2 行に折返し、「再試行」は 44px 級 |
| `_org-390.png` | （before 未確認・390） | 保留 | 同上 |
| `_org-412.png` | （before 未確認・412） | 保留 | 同上 |

## 集計

| 判定 | 枚数 | file |
| --- | --- | --- |
| 可 | 57 | 上表の『可』の行 |
| 要修正 | 0 | — |
| 保留 | 7 | `_graph-360.png` `_graph-390.png` `_graph-412.png`（横 scroll の手がかり無し）、`_org-1440.png` `_org-360.png` `_org-390.png` `_org-412.png`（fixture 不足で本体が写らない） |

## 残った課題（担当範囲）

1. **ホーム 1440 の会話枠の先頭行が半分切れる**（`_-1440.png`、360〜412 でも「確認を始めます。」の行が上端で半分切れる）。末尾追従で scroll した結果で (d) とは別物だが、初見では「切れた文字」に見える。会話枠の上端に数 px の余白か、sticky toolbar の下にぼかしを置くと落ち着く。保留。
2. **依存グラフの狭い幅で横 scroll の手がかりが無い**（`_graph-360/390/412.png`）。(b) と同じ型。枠の右端にぼかしか「→ 横に続く」の注記、または狭い幅では node を 1 列に積む。保留。
3. **組織画面は fixture 不足で判定できない**（`_org-*.png`）。`fake-daemon.mjs` の rich profile に `/api/v1/org` を足して撮り直さないと after の UI を見られない。保留。
4. 軽微な気づき（判定は可）: /accounts の card 幅が 2 種類、/artifacts 1440 の「本文をここで見る」の反復、/board の 優先度 / レベル / 担当 / 種類 列が全行同値、/daemon の「取得: fixture」の裸の診断語、/board 360 の長い題名 5 行（clamp なし）。

## 検査

| コマンド | 結果 |
| --- | --- |
| `sh scripts/dev/check-doc-links.sh` | exit 0（`check-doc-links: ok`） |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0（`ok (138 files)`） |
| `sh scripts/dev/progress-index.sh --check` | exit 0（`ok`） |
| 範囲 check（`git log --format= --name-only $CELERIS_WU_BASE..HEAD` が `agent-docs/progress/` のみ） | exit 0（変更は本 file 1 つ。web/・crates/ の変更なし） |

## 提案

- 狭い幅で横に伸びる枠（依存グラフ・/projects/P1 の木・表）に共通の「横に続く」手がかり（端のぼかし＋`aria-label`）を `components/ui` に 1 つ置く。
- screenshots.mjs の既定 fixture に `/api/v1/org` を足し、組織画面を after に含める。
