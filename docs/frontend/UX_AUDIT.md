# web/ 全画面 UX 監査
---
tasks: [01M3XTCNKMQBCHKSZ7Y1GF6ZM4]
---

## 画面ごとの監査

現行 web/ の画面台帳（`web/e2e/support/screens.ts`）を基準にする。入口は主に shell のナビゲーションと、一覧から詳細へのリンク。

台帳は31 route だが `/org/secretary` と `/org/$id` は同じ fixture `/org/cos` を使うため、撮影対象は30 unique fixture。`/tasks/T1/changes` の差分表示には長い行、`/projects/P1/docs/maintenance` は処理結果、`/inbox` と `/approvals` は誤操作防止を重点確認する。

以下は台帳の全 30 fixture の監査。before 画像では多くの取得系画面が失敗表示のため、データがある場合の密度や操作結果はコードから確認し、画像からの判断と区別する。

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

### /org — `/org`
- primary user task: 組織の木から担当（CoS・部・課）を選び、profile・skills を確かめて構成を変える。
- 入口と次の操作: shell の「組織」から入り、木の担当を選ぶ（`?selected=` に入る）。詳細で「話す」から `/org/<id>` へ進むか、担当を変更・削除・追加する。
- 情報の優先度: 木での現在位置、選んだ担当の役割と実効 profile・skills、変更の結果。追加フォームは補助。
- friction: before の 360/1440 は「取得に失敗しました。」と再試行だけで、木と詳細は評価できない（コードから判断）。`web/features/org/org-screen.tsx` は `CreateForm`（担当を追加）を木の section の中に置くため、狭い幅では木の直後に追加フォームが入り、選択した担当の詳細が下に押し下がる。`Tree` は全ノードを `Button` で積むので階層が深いと縦に長い。`NodeForm` の削除は `window.confirm` だけで、`project-ops.tsx` の `ConfirmButton`（dialog）と確認の作りが揃っていない。

### /projects — `/projects`
- primary user task: 案件を状態で見分けて開くか、新しい案件を作る。
- 入口と次の操作: shell の「案件」から入り、必要ならアーカイブを含めて一覧からタイトルを選び `/projects/<id>` へ進む。作成すると詳細へ移る。
- 情報の優先度: 案件名と状態（進行中・提案など）、アーカイブの有無、作成フォーム。
- friction: 360px の before では取得失敗の 1 行の下に「案件を作る」フォームが大きな枠で続き、作成が主操作に見える。`web/features/projects/project-list-screen.tsx` の一覧 `li` はタイトルと状態だけで、未処理の task 数・最終更新・担当の情報が無いので、どの案件を開くべきか判断しにくい。アーカイブの checkbox は 44px の四角だけが大きく、絞り込みというより入力欄に見える。

### /projects/$id — `/projects/P1`
- primary user task: 案件の状態・計画・仕事の木を把握し、案件・計画・リポジトリを操作する。
- 入口と次の操作: 案件一覧から入り、概要を読み、計画の DAG・仕事の木のリンクから task へ進む。「文書」「ボード」へ移るか、`ProjectOps` / `PlanOps` / `RepoOps` で変更する。
- 情報の優先度: 案件名と状態、根の task の件数と状況、計画の進み（段と途中目標）、仕事の木、成果物。操作は状態の後。
- friction: 360px の before は取得失敗で本文が無い（コードから判断）。`web/features/projects/project-detail-view.tsx` は概要の直後に `ops`（`project-ops.tsx` の 3 つの操作 section）を差し込むため、計画の DAG と仕事の木より先に長い編集フォームが並ぶ。`Overview` の件数は `/ ${status} ${count}` の生の status 名で、`PlanDag` も `milestone_status ?? task_status` をそのまま出す。「ボード」リンクは案件で絞らず `/board` 全体へ飛ぶ。

### /projects/$id/docs — `/projects/P1/docs`
- primary user task: 案件の文書を検索して読み、編集・保存・削除する。
- 入口と次の操作: 案件詳細の「文書」から入り、検索して左の一覧から文書を選ぶ。`?edit=1` で編集して保存するか、削除する。文書リポジトリが無ければ「文書を用意する」。
- 情報の優先度: 文書の一覧と現在の文書、本文、編集中の保存状態・etag の衝突。
- friction: 360px の before は「案件詳細」「文書の保守」の 2 リンクと取得失敗だけ。`web/features/projects/project-docs-screen.tsx` は lg 未満で一覧 `aside` を本文の上に縦積みにするので、文書が多いと本文へ届くまで長くスクロールする。`Editor` は textarea 1 つで、閲覧と編集の切替は URL の `edit=1` に頼る。削除は `window.confirm` で、409（文書リポジトリが無い）は空状態として出るが、他の取得失敗との違いは文言でしか分からない。

### /projects/$id/docs/maintenance — `/projects/P1/docs/maintenance`
- primary user task: 文書の監査結果と整理案を確かめ、承認・適用し、文書管理ポリシーを採用する。
- 入口と次の操作: 文書画面の「文書の保守」から入り、監査結果を保存し、整理案を承認 → 適用する。結果の task が返れば「結果のタスクを開く」へ進む。
- 情報の優先度: 監査で見つかった問題、整理案の中身、承認済みかどうか、実行結果と後続 task。
- friction: 360px の before は説明 1 行と取得失敗だけ。`web/features/projects/project-docs-maintenance-screen.tsx` は監査結果・保存済みレポート・結果を `JSON.stringify` の `pre` で出し、整理案とポリシーは JSON を直接書く textarea なので、人が読んで判断する画面になっていない。JSON の parse に失敗すると `run()` は何も出さずに return する。「承認済み案を適用」は確認なしの破壊的操作で、`h2` も無装飾のため節の境目が見えない。

### /board — `/board`
- primary user task: 案件の task を状態の列で見渡し、優先度・レベル・担当を直し、task を開く。
- 入口と次の操作: shell の「ボード」か案件詳細から入り、案件・検索・ラベルなどで絞り込み、列のカードから task へ進むかカードの中で編集する。
- 情報の優先度: 列ごとの件数、カードの題名・状態・優先度・担当、現在の絞り込み。
- friction: 360px の before では 9 個の絞り込み欄（`board-filter-form`）が 1 画面を占め、列は取得失敗の下にある。`web/features/projects/board-screen.tsx` の各欄は自由入力で、レベル・種類は候補が出ない。列は `w-64` の横スクロール枠（`board-scroll-frame`）で 6 列あり、狭い幅では 1 列ずつしか見えず、今どの列かの手掛かりが無い。`BoardCard` は全カードに編集フォームを常に出すので、カードが高くなり一覧性が落ちる。

### /knowledge — `/knowledge`
- primary user task: 知識ページを検索して読み、必要なら編集する。
- 入口と次の操作: shell の「知識」から入り、検索して結果のリンクを選び、本文を読む。「編集」で `edit=1` にして保存する。「候補」「skills」へも移れる。
- 情報の優先度: 検索結果と選んだページの本文、path、編集の保存結果。
- friction: 1440px の before は検索欄が全幅に伸び、結果は 1 件、本文の枠は「知識を選択してください。」だけで右側が大きく空く。`web/features/knowledge/knowledge-screen.tsx` の結果は題名のリンクだけで、scope（user / projects）・更新日・抜粋が無い。lg 未満では結果の枠の下に本文が来るので、ページを選んだ後に本文までスクロールが要る。候補の未処理件数は「候補」リンクに出ない。

### /knowledge/inbox — `/knowledge/inbox`
- primary user task: 知識の候補を読み、取り込み先を決めて採用するか却下する。
- 入口と次の操作: 知識の「候補」から入り、候補の本文を読み、取り込み先を直して採用・却下する。既存ページがあれば上書きを選ぶ。
- 情報の優先度: 候補の題名と本文、出典、取り込み先と既存ページとの衝突、判断の結果。
- friction: 360px の before では題名「New knowledge」が `h2` と本文の `Markdown` の見出しで 2 回出る。`web/features/knowledge/knowledge-screen.tsx` の `Candidate` は出典（source）と既存ページとの差分を示さず、「却下」は確認なしで、採用と同じ見た目の `Button` が並ぶ。操作の結果は画面上部の `aria-label="操作の結果"` の section に候補 id 付きで出るため、押した候補から離れた場所に結果が出る。

### /knowledge/skills — `/knowledge/skills`
- primary user task: skill を一覧から選んで SKILL.md と付属ファイルを読み、作成・編集・削除する。
- 入口と次の操作: 知識の「skills」から入り、一覧から選んで本文を読む。「作成」で新しい skill を作り、「編集」で直すか「削除」する。
- 情報の優先度: skill 名と説明、SKILL.md の本文、付属ファイル、どの課に mount されているか。
- friction: 360px の before は一覧の枠と「skill を選択してください。」の枠が同じ輪郭で続き、空の右枠が主役に見える。`web/features/knowledge/skills-screen.tsx` の `SkillDetail` の「削除」は確認なしで即 DELETE する。skill がどの課（`/org` の `OrgSkills`）で使われているかがこの画面からは分からず、削除の影響範囲を判断できない。

### /daemon — `/daemon`
- primary user task: dispatcher が動いているか・詰まっていないかを確かめ、必要なら replay で状態の不整合を調べる。
- 入口と次の操作: shell の「daemon」から入り、状態を読む（自動で再取得する）。必要なら「replay を実行」して mismatch を見る。
- 情報の優先度: 最後の tick がいつか（stale かどうか）、実行中・人の待ち・担当なしの件数、replay の結果。
- friction: 360px の before は「dispatcher の状態はまだありません。」「取得: fixture」と replay の枠だけ。`web/features/ops/daemon-screen.tsx` の `Row` は `last_tick_at` を ISO の文字列で出すだけで、何秒前か・stale かを判定して示さない。実行中・人の待ち・担当なしは件数だけで、該当 task への導線が無い。replay の結果は英語の「mismatches across tasks」で、取得時刻「取得:」は状態の枠の外に小さく出る。

### /providers — `/providers`
- primary user task: provider の接続状態と同時実行数・model・tier を確かめて直し、新しい provider を足す。
- 入口と次の操作: shell の「プロバイダ」から入り、カードで値を変えて「変更を保存」か「接続を確認」、不要なら削除する。下のフォームで追加する。
- 情報の優先度: provider ごとの使用可否（cooldown・前回の確認）、tier と model、変更の結果。
- friction: 1440px の before では `ProviderCard` の concurrency・model 欄が全幅の 2 列に伸び、tier の checkbox と 3 つのボタンが同じ重みで並ぶ unstyled admin の見た目。`web/features/ops/providers-screen.tsx` は状態（cooldown・前回の確認）を小さな文の行で出し、使えるかどうかが一目で分からない。保存・削除のたびに `/api/reload` を続けて呼び、reload の失敗は一覧の下に `role="alert"` で出るため、押したカードから離れる。削除は `window.confirm`。

### /accounts — `/accounts`
- primary user task: account の認証状態を確かめてログインし、secret・LLM source・MCP クライアントを管理する。
- 入口と次の操作: shell の「アカウント」から入り、account の「確認」「ログイン開始」を押してコードを入れる。account・secret を追加・削除し、MCP クライアントの呼び出しを見る。
- 情報の優先度: account のログイン状態と使用中の数、ログイン待ちの手順、secret の有無。
- friction: 360px の before では account 1 件の下に「アカウントを追加」「secret」の入力フォームが続き、主題がページの途中で変わる。`web/features/ops/accounts-screen.tsx` の `AccountsScreen` は `SecretsSection`（`secrets-section.tsx`）と `McpClientsSection`（`mcp-clients.tsx`）を同じページに縦に積み、節の切れ目が `h2` だけ。`AccountCard` の「確認」「ログイン開始」「削除」は同じ見た目で並び、削除は `window.confirm`。

### /clusters — `/clusters`
- primary user task: cluster に接続（TOTP などのコード入力）し、作業ディレクトリを設定する。
- 入口と次の操作: shell の「クラスタ」から入り、「接続」を押してコードを送るか取り消す。作業ディレクトリを保存するか上書きを消す。
- 情報の優先度: 接続中かどうか、コード待ちの状態と prompt、作業ディレクトリとその出どころ（config か上書きか）。
- friction: 360px の before では「pegasus.example / auth totp / 未接続」が 1 行の文で、接続状態が色や badge で区別されない。`web/features/ops/clusters-screen.tsx` の `ClusterCard` は作業ディレクトリ欄を常に空で出し、現在の値は上の文にしか無い。「上書きを消す」は確認なしで、「作業ディレクトリを保存」は空のとき disabled だが理由を示さない。接続の失敗と disconnected・stale の違いを示す欄が無い。

### /releases — `/releases`
- primary user task: release の gate と変更の量を確かめ、選んだ release を昇格し、結果を追う。
- 入口と次の操作: shell の「リリース」から入り、稼働中・current・previous を確かめ、行の「<sha> を昇格」を押して昇格の状態（pending → 成功・失敗）を待つ。
- 情報の優先度: 稼働中の release、各 release の gate・問題・直近の昇格失敗、変更の量、昇格の進み。
- friction: 1440px の before では稼働中・current・previous が 1 行の小さな文で、sha12 だけの行が並ぶ。`web/features/ops/releases-screen.tsx` の `ReleaseRow` の昇格ボタンは確認なしで本番の release を切り替える（後戻りしにくい操作）。current の行も同じ見た目の disabled ボタンを出し、なぜ押せないか（`canPromote`）を示さない。`PromotionStatus` は一覧の上にしか出ず、どの行の昇格かが離れる。

### /graph — `/graph`
- primary user task: 指定した task を根に依存関係をたどり、関係する task を見つける。
- 入口と次の操作: shell の「依存グラフ」から入り、root と depth を入れて絞り込み、ノードを見る。
- 情報の優先度: 根の task、ノードの状態、辺の向き、ノード数・辺数。
- friction: 360px の before では root・depth の英語ラベルと取得失敗だけで、root に何を入れるか（task id）の説明が無い。`web/features/tasks/graph-view.tsx` の SVG ノードは `<title>` だけでリンクでもボタンでもなく、task 詳細へ進めない・キーボードで辿れない。状態は `<text>` の生の値で色分けが無く、辺は矢印の無い `line` なので向きが分からない。題名は 23 文字で切る。

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
