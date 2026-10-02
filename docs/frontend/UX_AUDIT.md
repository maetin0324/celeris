# web/ 全画面 UX 監査
---
tasks: [01M3XTCNKMQBCHKSZ7Y1GF6ZM4]
---

## 画面ごとの監査

現行 web/ の画面台帳（`web/e2e/support/screens.ts`）を基準にする。入口は主に shell のナビゲーションと、一覧から詳細へのリンク。主要な次操作は表示名から整理したもので、fixture の現在の状態・操作結果を個別に検証する監査は後続作業で補う。

| fixture | primary user task / 次の操作 | 優先情報 | friction の確認点 |
|---|---|---|---|
| `/org` | 組織を探して選ぶ／構成を変更する | 組織階層と選択詳細 | 階層と詳細の移動・編集操作 |
| `/projects` | 案件を探す・作る／詳細へ進む | 案件名と状態 | 一覧と作成操作の優先度 |
| `/projects/P1` | 案件の作業を把握する／task・repo 等へ進む | 状態、計画、仕事の木 | 情報量と長い構造の走査 |
| `/projects/P1/docs` | 案件文書を読む・編集する | 文書本文と保存状態 | 閲覧と編集の切替 |
| `/projects/P1/docs/maintenance` | 文書保守を起動する／結果を確認する | 起動条件と処理結果 | 実行中・失敗後の次の手掛かり |
| `/board` | 案件の進捗を列で確認する／task を開く | 列と task 状態 | 6列の小画面での移動 |
| `/knowledge` | 知識を検索して読む／保存する | 検索結果と本文 | 検索条件・本文の切替 |
| `/knowledge/inbox` | 知識候補を採用・却下する | 候補本文と判断 | 判断の文脈と破壊的操作 |
| `/knowledge/skills` | skill を探す・作成・編集する | skill 名と本文 | 一覧・詳細・編集の状態識別 |
| `/daemon` | daemon の状態を確認し replay する | 接続状態と replay 結果 | stale と停止中の識別 |
| `/providers` | provider を管理・確認する | 接続状態と設定 | 削除・check の結果と確認 |
| `/accounts` | account・認証・secret を管理する | account 状態と認証手順 | 秘密値の扱いと操作密度 |
| `/clusters` | cluster の状態と設定を確認する | 可用性・接続情報 | disconnected と stale の区別 |
| `/releases` | release の状態を確認する | version と昇格状態 | 状態遷移と影響範囲の理解 |
| `/graph` | task の依存関係をたどる | 依存と選択中 task | 小画面での graph navigation |

台帳は31 route だが `/org/secretary` と `/org/$id` は同じ fixture `/org/cos` を使うため、撮影対象は30 unique fixture。`/tasks/T1/changes` の差分表示には長い行、`/projects/P1/docs/maintenance` は処理結果、`/inbox` と `/approvals` は誤操作防止を重点確認する。

以下は担当 15 fixture の監査。before 画像では多くの取得系画面が失敗表示のため、データがある場合の密度や操作結果はコードから確認し、画像からの判断と区別する。

### / — `/`
- primary user task: CoS に依頼を送り、返答と進捗を追う。
- 入口と次の操作: shell の「ホーム」から入り、宛先を確認して入力・送信する。必要なら「新しい会話」を始める。
- 情報の優先度: 宛先と会話履歴、現在の進捗、下端の入力欄と送信結果。
- friction: 360px の空の初期画面は入力欄まで大きな空白が続く。`web/features/console/console-view.tsx` の fixed composer は `visualViewport` 補正と IME 判定を持つが、キーボード表示時の本文との重なりは静止画では判定できない。

### /inbox — `/inbox`
- primary user task: 承認待ち・質問・draft・注意を見分け、今必要な判断を処理する。
- 入口と次の操作: shell の「受信箱」から入り、対象 task を開くか、note・回答を入力して approve / reject / answer / cancel する。
- 情報の優先度: 区画ごとの件数、対象と判断理由、操作後の結果。空の区画は短く示す。
- friction: 360px の before は 0 件の区画カードが縦に連続し、次の行動が見えない。`web/features/inbox/inbox-screen.tsx` の `Section` と `TaskActions` は実データ時に判断ボタンと結果が各行へ増えるため、長文・複数件時の走査と誤押下を別途確認する。

### /login — `/login`
- primary user task: パスワードでログインして元の行き先に戻る。
- 入口と次の操作: 未認証時の転送または session 失効から入り、パスワードを入力して「ログイン」する。
- 情報の優先度: パスワード欄、送信状態、失敗理由、戻り先。
- friction: 360px ではフォームは明快だが、`web/routes/login.tsx` は戻り先 `next` を画面に示さない。認証失敗は欄外の短い文だけなので、入力ミス後の焦点と再入力の流れを確認する。

### /org/secretary・/org/$id — `/org/cos`
- primary user task: 指定した組織の人へ依頼し、その人との会話を読む。
- 入口と次の操作: 組織から人を選ぶか旧 `/org/secretary` から転送され、宛先を確認して依頼を送る。
- 情報の優先度: 人の名前・宛先、会話履歴、入力欄。fixture は両 route で同じ `/org/cos`。
- friction: 360px の空画面では人の役割や依頼可能範囲が分からない。`web/routes/org.$id.tsx` は ID を見出しに、`web/features/console/console-view.tsx` は宛先にそのまま使うため、ID が意味を持たない利用者には相手を判別しにくい。

### /tasks — `/tasks`
- primary user task: task を検索・状態で絞り、目的の task を開く。
- 入口と次の操作: shell の「タスク」から入り、検索・件数・status・並びを設定して一覧から詳細へ進む。
- 情報の優先度: 検索条件と適用状態、task のタイトル・状態、追加読み込みと取得失敗。
- friction: 360px の before では 8 個の英語 status 選択が 3 行に折り返され、一覧より先に画面を占める。`web/features/tasks/task-list-view.tsx` の検索条件フォームは URL と同期するが、選択中の条件の全体像が一目で取りにくい。

### /tasks/new — `/tasks/new`
- primary user task: 目的と検証可能な受け入れ条件を定めて task を作成する。
- 入口と次の操作: task 一覧やヘルプから入り、名前・目的・条件の種類と内容を入力し「タスクを作成」する。
- 情報の優先度: 名前と目的、条件の種類・内容、追加・削除、入力エラーと作成結果。
- friction: 360px の before では条件 1 の種類が既定で `human` だが意味の説明が無い。`web/features/tasks/create-screen.tsx` は条件ごとに select と入力を繰り返すため、複数条件の識別と 422 エラーの対応づけが課題になる。

### /tasks/$id — `/tasks/T1`
- primary user task: task の状態を理解し、次の判断・コメント・再試行を行う。
- 入口と次の操作: 一覧・受信箱などから開き、概要から timeline / 変更 / 作業ツリー / 成果物へ移るか判断パネルで操作する。
- 情報の優先度: 現在状態と要判断事項、直近の活動、操作結果、関連 tab。
- friction: 360px の before は取得失敗で本文を評価できず、5 tab のラベルが「概要」「timeline」「変更」など混在して折り返す。`web/features/tasks/task-detail-view.tsx` の横スクロール tab と `decision-panel.tsx` の多数の操作は、実データで現在位置と危険度の見分けが必要。

### /tasks/$id/files — `/tasks/T1/files`
- primary user task: 作業ツリーからファイルを選び、その内容を確認する。
- 入口と次の操作: task 詳細の作業ツリーから入り、repo・path をたどって file を選び、必要なら詳細へ戻る。
- 情報の優先度: 現在の path、階層内の項目、選択 file の本文または取得理由。
- friction: 360px の before は「作業ツリーまたは file が見つかりません」と `/` を表示するが、対象が空なのか取得に失敗したのか判断しにくい。`web/features/files/task-files-view.tsx` の `TreePane` / `FilePane` は長い path と本文を `break-all` で収めるため、コード行の読みやすさを確認する。

### /tasks/$id/changes — `/tasks/T1/changes`
- primary user task: 変更ファイルと差分を読み、取り込み方法を判断する。
- 入口と次の操作: task 詳細の「変更」から入り、repo と file を選び、差分を見て merge / PR / 破棄を選ぶ。
- 情報の優先度: 変更の規模と統合状態、選択中の file と差分、破壊的操作の確認と結果。
- friction: before は取得失敗で差分自体を評価できない。`web/features/changes/changes-view.tsx` は破棄を他の取り込み方法と同じ select に置き、確定ボタンも共通の「取り込む」なので、選択後の危険度と実行結果の識別が重要。

### /tasks/$id/runs/$runId — `/tasks/T1/runs/R1`
- primary user task: run の会話・コマンド・エラーを時系列で追う。
- 入口と次の操作: task 詳細の run から入り、発話を読み、必要な tool / command の詳細を展開する。
- 情報の優先度: run の識別と実行中かどうか、発話順、エラー、追記と省略行数。
- friction: 360px の before は「run ログを取得できませんでした」だけで再試行導線が無い。`web/features/runs/run-log-view.tsx` は多数の event を同形の枠で積むため、長いログで発話と tool の区別・現在位置を失いやすい。

### /plans/new — `/plans/new`
- primary user task: 目標を入力し、計画として扱う root task を作る。
- 入口と次の操作: 計画の作成へのリンクから入り、目標を書いて「計画を作成」し、作成された task 詳細へ進む。
- 情報の優先度: 目標、作成した後の行き先、入力・送信エラー。
- friction: 360px の before は目標欄だけで、通常の task 作成との違いや作成後の流れを示さない。`web/features/tasks/create-screen.tsx` の `PlanCreateScreen` も単一フォームなので、計画生成が後続工程であることが伝わりにくい。

### /approvals — `/approvals`
- primary user task: 保留中の認可を判断し、常設ルールを管理する。
- 入口と次の操作: shell の「承認」または受信箱から入り、対象を読んで一回許可・常設許可・拒否を選ぶ。必要なら既存ルールを削除する。
- 情報の優先度: 認可対象と根拠、判断の範囲、決定済みとの区別、常設ルールの影響範囲。
- friction: 360px の before は 3 区画それぞれに取得失敗が並ぶ一方、常設ルール追加フォームは使える。`web/features/approvals/approvals-screen.tsx` の `ApprovalRow` は三つの判断ボタンを並列に置くため、一回限りと常設の影響差を押す前に示す必要がある。

### /artifacts — `/artifacts`
- primary user task: 案件に属する task の成果物を探して開く。
- 入口と次の操作: shell の「成果物」から入り、案件を選んで絞り込み、task または成果物 preview / download を開く。
- 情報の優先度: 対象案件、task と成果物名、閲覧可否と形式。
- friction: 360px の before では初期状態で案件選択が必須だが、最近の成果物へ直接進めない。`web/features/artifacts/artifacts-view.tsx` の `ArtifactsList` は案件単位の一覧なので、名前だけでなく task との関係を読み取りやすくする必要がある。

### /reports — `/reports`
- primary user task: 報告を重要度・階層で絞って読み、既読にする。
- 入口と次の操作: shell の「報告」から入り、通知設定と絞り込みを確認し、行を展開して元の報告を読むか既読にする。
- 情報の優先度: 未読と重要度、見出し、元の報告、既読結果。通知の許可状態は補助情報。
- friction: 1440px の before は取得失敗でも通知設定・一括既読・試験ボタンが先に並び、報告を読む主目的より設定が目立つ。`web/features/reports/reports-screen.tsx` の `ReportRow` は種別と level を生の値で示すため、判断に必要な意味が伝わりにくい。

### /help — `/help`
- primary user task: task・受信箱・状態・失敗時の操作を調べる。
- 入口と次の操作: shell の「ヘルプ」から入り、目次アンカーで節へ移り、関連画面へのリンクから実行する。
- 情報の優先度: 目的別の入口、短い手順、状態の意味、関連画面への導線。
- friction: 360px の before では目次と各節が同じ輪郭のカードとして長く続き、探す手掛かりが弱い。`web/features/help/help-screen.tsx` の目次は節アンカーのみで検索がなく、失敗した task の直し方を状況から探しにくい。

## component hierarchy

全画面は共通 shell（ナビゲーション、状態通知、main 領域）を土台に、route 固有の見出し・操作・データ表示を置く。詳細画面では一覧からの入口、現在位置、次に可能な操作が追える必要がある。画面固有の component 境界と再利用実態は後続監査でコードに照合する。

## responsive の振る舞い

基準幅は 360/390/412/1440 px。狭い幅では一覧の情報優先度を保ち、表・ログ・差分・依存図の横スクロールを局所化し、操作対象は十分なタップ領域を持たせる。画面全体の横溢れ、固定入力欄とソフトキーボードの重なり、長文による操作の押し下げを before 画像で確認する。

## 状態の現状

この撮影は fixture の初期表示を記録するもので、状態遷移の試験ではない。loading、empty、error、stale、disconnected、permission-denied と破壊的操作の確認状況は、before 画像だけでは判定できず未監査として扱う。各画面でデータが無い状態と取得失敗を区別し、変更操作は対象・結果・取り消し可否を確認できることが必要。

## cosmetic と IA/component 層の切り分け

余白、文字階層、色、境界、折返しなど情報構造を変えない問題は cosmetic として調整できる。主要操作の発見性、情報の優先順、一覧と詳細の移動、複数操作の混在、状態の誤認は IA または component 境界から見直す。管理画面を共通カードの寄せ集めにせず、設定対象と危険度に沿ってまとまりを作る。

## 4 群の割り当て

| 群 | fixture |
|---|---|
| foundation | `/`, `/login`, `/help` |
| task・run 系 | `/tasks`, `/tasks/new`, `/tasks/T1`, `/tasks/T1/files`, `/tasks/T1/changes`, `/tasks/T1/runs/R1`, `/plans/new`, `/graph` |
| inbox・project 系 | `/inbox`, `/projects`, `/projects/P1`, `/projects/P1/docs`, `/projects/P1/docs/maintenance`, `/board`, `/knowledge`, `/knowledge/inbox`, `/knowledge/skills`, `/reports`, `/approvals`, `/artifacts` |
| 管理系 | `/org`, `/org/cos`, `/daemon`, `/providers`, `/accounts`, `/clusters`, `/releases` |

## quality gate critique

品質ゲートでは、Celeris 固有の高密度な ops workbench として、現在の作業・判断・復旧に必要な情報が先に読めるかを確かめる。generic AI dashboard の統計ヒーロー、同じ形の card wall、用途のない過剰余白、unstyled admin の単調なフォーム列になっていないかを全画面で見る。状態を色だけに頼らず示し、キーボード focus、狭幅での操作、エラーからの復帰、破壊的操作の確認を画面ごとに検証する。撮影画像は初期状態の視覚資料であり、これらの操作性・アクセシビリティ gate の合格証明ではない。

## before screenshot

- 保存先: `artifacts/before`（WU artifacts 絶対 path: `/var/lib/celeris/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before`）。
- 撮影コマンド: `corepack pnpm@12.6.0 -C web screenshots -- --out /var/lib/celeris/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before`
- PNG: **120 枚**（31 route / 30 unique fixture × 360/390/412/1440）。幅は各 viewport の CSS px。
- 命名規則: fixture の英数字以外を `_` に置換し、末尾に `-<width>.png` を付ける。root `/` は `_` なので `_-360.png` 等。同一 fixture の route は同じファイル名になる。
