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

web/ の部品は少なく、共通の層は shell・page header・取得の枠・操作の結果の 4 つだけ。list/table・detail・form・dialog は共通部品が無く、各 `web/features/*/*-screen.tsx` が Tailwind の class を直接書いている。

```
__root.tsx（認証 gate）
└─ Shell                         web/components/shell/shell.tsx
   ├─ header: ロゴ・「メニュー」ボタン（md 未満）・接続状態・最終受信（md 以上）
   ├─ nav（17 項目の平らな列）     web/components/shell/nav-items.ts
   ├─ celeris 停止の帯（role=alert） shell.tsx の server.down
   ├─ main#main
   │  └─ route（web/routes/*.tsx）→ feature の画面（web/features/<領域>/*-screen.tsx / *-view.tsx）
   │     ├─ page header: ScreenFrame の h1        web/components/shell/screen-frame.tsx
   │     ├─ 取得の枠: FetchFrame / ErrorNotice     web/components/fetch-state/fetch-frame.tsx
   │     ├─ 区画: Panel（h2 付きの枠）             web/components/ui/panel.tsx
   │     ├─ list/table / detail / form            各 feature に直書き
   │     ├─ 操作と結果: useActionResult / ActionResultView  web/components/actions/use-action-result.tsx
   │     └─ 本文: Markdown / ArtifactPreview      web/components/content/
   └─ aside[data-console-slot]（Console の置き場。中身は features/console/console-view.tsx）
```

| 層 | 実体の file | 現状 |
|---|---|---|
| shell | `web/components/shell/shell.tsx`, `scroll-memory.ts`, `use-server-state.ts`, `not-found.tsx` | md（768px）で左 224px の縦 nav と上の横 header を切り替える 1 本だけの shell。遷移後に `#main h1` へ focus を移す。 |
| nav | `web/components/shell/nav-items.ts`（`shell.tsx` が描く） | 17 項目を業務・管理の区別なく 1 列に並べる。badge は受信箱・承認・報告の 3 つで、未取得のときは `?`。 |
| page header | `web/components/shell/screen-frame.tsx` | h1 だけ。現在位置（パンくず）・状態・主操作の置き場が無く、`/tasks/T1` の tab（`features/tasks/task-detail-view.tsx` の `nav`）や絞り込みは各画面が自前で置く。 |
| list/table | `features/tasks/task-list-view.tsx`（唯一の `<table>`、`min-w-112` で横 scroll）、他は `<ul>`/`<li className="rounded border p-3">` の直書き（`inbox-screen.tsx`, `approvals-screen.tsx`, `project-list-screen.tsx`, `reports-screen.tsx` 等） | 一覧の行の形が画面ごとに違い、列・密度・並びの共通の型が無い。`/board` は `board-screen.tsx` の横 scroll の列。 |
| detail | `features/tasks/task-detail-view.tsx` + `overview-view.tsx`・`timeline-view.tsx`・`execution-panel.tsx`・`decision-panel.tsx`、`features/projects/project-detail-view.tsx`、`features/runs/run-log-view.tsx` | 詳細は縦に積んだ区画。list-detail の 2 列は `lg:grid-cols-[…]` の `knowledge-screen.tsx`・`skills-screen.tsx`・`org-screen.tsx`・`project-docs-screen.tsx` の 4 画面だけ。 |
| form | 直書きの `<label className="flex flex-col">` + input（`create-screen.tsx`, `board-screen.tsx`, `providers-screen.tsx`, `accounts-screen.tsx`, `secrets-section.tsx` 等）。button は `web/components/ui/button.tsx` の 1 種類 | field・説明・検証の部品が無い。button は主・副・危険の区別が無く、「削除」も「追加」も同じ見た目。 |
| dialog | `features/projects/project-ops.tsx` の `<dialog>`（showModal）だけ。他は `window.confirm`（`org-screen.tsx`, `project-docs-screen.tsx`, `providers-screen.tsx`, `accounts-screen.tsx`, `secrets-section.tsx`） | 確認の部品が共通化されていない。 |
| Console | `features/console/console-view.tsx`（`routes/index.tsx` と `routes/org.$id.tsx` で使う） | 入力欄は `fixed inset-x-0`（md 以上は `md:left-56`）で画面下に固定。shell の `aside[data-console-slot]` は空のまま。 |

`web/styles.css` は `@import "tailwindcss";` の 1 行だけで、色・余白・文字の token が無い。灰色は `neutral-*` を各 file で直接指定している。

## responsive の振る舞い

根拠は `artifacts/before` の PNG（360・1440 を中心に 390・412 も確認）と、上の file の class。全 120 枚とも PNG の幅は viewport と同じ（360/390/412/1440）で、ページ全体の横溢れは撮れていない。before の大半の画面は fixture の取得が失敗し「取得に失敗しました。」で止まるため、データがある時の折り返しはコードから読んだ。

| 幅 | shell | 折り返す | 隠れる | 横 scroll（枠の中） |
|---|---|---|---|---|
| 360 | 上の header（「Celeris」と「メニュー」）だけ。nav は「メニュー」を押すと 2 列の grid で main に重なる | `/tasks` の状態 8 ボタンが 3 段（`_tasks-360.png`）。`/board` の絞り込み 9 項目が縦 1 列で 952px の縦長（`_board-360.png`）。`/providers` は既存の行の編集 form と追加 form が縦に積まれ 1158px（`_providers-360.png`）。`/help` の目次 6 リンクが 4 段・本文は 1800px（`_help-360.png`）。`/tasks/new` の受け入れ条件 1 件が 1 画面の半分を使う（`_tasks_new-360.png`） | 接続状態と最終受信（`shell.tsx` の `hidden md:block`）が全画面で見えない。badge 付きの nav（受信箱・承認・報告）もメニューを開くまで見えない | `/tasks` の表（`min-w-112`）、`/tasks/T1` の tab、`/tasks/T1/changes` の差分（`whitespace-pre`）、`/tasks/T1/runs/R1` の出力（`max-h-64`）、`/board` の列（`w-max`）、`/graph` の SVG、`/projects/P1` と `/projects/P1/docs/maintenance` の枠 |
| 390・412 | 360 と同じ（md 未満） | 360 より 1 段減る程度。`/board` は 876px、`/providers` は 800px に収まる。`/help` は 1752px・1728px | 360 と同じ | 360 と同じ |
| 1440 | 左 224px（`md:w-56`）の縦 nav に 17 項目。下端に「接続状態: 未確認」 | `/tasks` の絞り込みは 2 段（`_tasks-1440.png`）。`/help` の区画は `sm:grid-cols-` で並ぶ | なし | `/tasks/T1/changes` の差分と run の出力は枠の中だけ。main は `max-width` が無く、1440 では入力・ボタンが左に寄り右 1000px 以上が空く（`_tasks-1440.png`, `_tasks_T1-1440.png`） |

画面ごとの要点:

- md（768px）未満と以上で切り替わるのは shell だけ。画面の中で 2 列になるのは lg（1024px）以上の `/knowledge`・`/knowledge/skills`・`/org/cos`・`/projects/P1/docs` の 4 画面で、360〜412 では一覧の下に詳細が積まれ、選んだ項目の詳細が画面外に出る。
- `/` と `/org/cos` の Console 入力欄は `fixed` で下端に固定し、本文は `pb-40` で逃がしている。ソフトキーボードが出たときの重なりは before 画像では確かめられない。
- 360 でも main の余白は `p-4`（16px）で、`/board`・`/providers`・`/accounts` のように form の項目ごとに label を上に置く画面は 1 項目 76px 前後を使い、一覧の行が最初の画面に出ない。
- 1440 では `/tasks/T1` の tab・`/graph` の root/depth・`/tasks/new` の form が左上の狭い範囲に集まり、高密度の表示にも 2 列の配置にもなっていない。

## 状態の現状

各状態の共通の実装は 3 か所だけ: 取得の枠 `web/components/fetch-state/fetch-frame.tsx`（loading・error・再取得失敗）、操作の結果 `web/components/actions/use-action-result.tsx`（409 を「状態が変わりました」、timeout を「結果を確認できません」）、shell の停止の帯 `web/components/shell/shell.tsx`（`server.down`）。empty・permission-denied・破壊的操作の確認は各画面の直書きで、型が揃っていない。

| 画面（fixture） | loading | empty | error | stale | disconnected | permission-denied | 破壊的操作の確認 |
|---|---|---|---|---|---|---|---|
| 共通（shell・全画面） | `FetchFrame`: 灰色の棒 + 「読み込み中…」「時間がかかっています」（`fetch-frame.tsx`, `delay-tracker.ts`） | 共通部品なし | `ErrorNotice`「取得に失敗しました。」+ 再試行。理由・状態コードは出さない | データを残したまま再取得に失敗したとき `ErrorNotice` を重ねる。いつの値かは出さない | 停止の帯（`shell.tsx`）。「接続状態: 未確認」は md 以上だけで、値が更新されない | `api/client.ts` が 403 を `forbidden` に分けるが共通の表示は無い。401 は `routes/__root.tsx` の認証 gate で `/login` へ | 共通部品なし |
| `/` | `FetchFrame` | — | `ErrorNotice` | Console は SSE の `since` から再開（`features/console/stream.ts`） | 帯のみ | — | — |
| `/inbox` | `FetchFrame` | 「ありません」の文（`inbox-screen.tsx`） | `ErrorNotice` | `expected_status` 付きで送り、409 は再取得 | 帯のみ | — | 却下・中止・一括承認は確認なしで即送信 |
| `/tasks` | `FetchFrame` | 0 件の文（`task-list-view.tsx`） | `ErrorNotice` | 同上 | 帯のみ | — | 操作なし |
| `/tasks/T1` | `FetchFrame` | timeline・実行の 0 件文 | `ErrorNotice` | 409 は再取得（`execution-panel.tsx`, `decision-panel.tsx`） | 帯のみ | — | phase gate・再レビュー・分解は確認なしで送信 |
| `/tasks/T1/files` | `FetchFrame` | 「ありません」の文 | `ErrorNotice` | — | 帯のみ | 403 `path_forbidden` を「作業ツリーの外か、読めない場所です」（`task-files-view.tsx`） | — |
| `/tasks/T1/changes` | `FetchFrame` | 差分なしの文（`changes-view.tsx`） | `ErrorNotice` | — | 帯のみ | — | 「取り返しがつかないことを確認した」checkbox を入れるまで実行不可（唯一の段階的確認） |
| `/tasks/T1/runs/R1` | 灰色の棒だけで文言なし（`run-log-view.tsx`） | — | run log 固有の error 文（`run-log-view.tsx`） | 「古い N 行は省略」（`run-log-buffer.ts`） | 帯のみ | 権限で拒否された tool を「権限で拒否」と表示（`run-log.ts`） | — |
| `/artifacts`・`/tasks/T1` の成果物 | `FetchFrame` | 0 件の文 | `ErrorNotice` | — | 帯のみ | 「読めない場所です」（`artifacts-view.tsx`, `task-artifacts-view.tsx`） | — |
| `/approvals` | `FetchFrame` | 0 件の文 | `ErrorNotice` | — | 帯のみ | — | 拒否・常設ルール削除は確認なし（`approvals-screen.tsx`） |
| `/projects`・`/projects/P1` | `FetchFrame` | 0 件の文 | `ErrorNotice` | — | 帯のみ | — | 中止・アーカイブ・削除は `<dialog>`（`project-ops.tsx`） |
| `/projects/P1/docs`・`/projects/P1/docs/maintenance` | `FetchFrame` | 0 件の文 | `ErrorNotice` | — | 帯のみ | — | 文書の削除は `window.confirm`（`project-docs-screen.tsx`）、整理の実行は説明文のみ |
| `/board` | `FetchFrame` | 列ごとの 0 件 | `ErrorNotice` | 409 は再取得 | 帯のみ | — | 操作は優先度の編集だけ |
| `/knowledge`・`/knowledge/inbox`・`/knowledge/skills` | `FetchFrame` | 0 件の文 | `ErrorNotice` | — | 帯のみ | — | 知識候補の却下・skill の削除は確認なし（`knowledge-screen.tsx`, `skills-screen.tsx`） |
| `/org`・`/org/cos` | `FetchFrame` | 0 件の文（`org-skills.tsx`） | `ErrorNotice` | 409 の文言を直書き（`org-skills.tsx`） | 帯のみ | — | 課の削除は `window.confirm`（`org-screen.tsx`） |
| `/providers`・`/accounts` | `FetchFrame` | 0 件の文 | `ErrorNotice` | — | 「接続を確認」の結果を行に出す | — | 削除は `window.confirm`。secret の削除も同じ（`secrets-section.tsx`） |
| `/clusters`・`/daemon` | `FetchFrame` | 0 件の文 | `ErrorNotice` | — | 帯のみ | — | 再計算は説明文のみ（`daemon-screen.tsx`） |
| `/releases` | `FetchFrame` | 0 件の文 | `ErrorNotice` | 昇格の途中停止を `promote_stale` で表示（`releases-promotion.ts`） | 帯のみ | — | 昇格は確認なしで送信、結果は「昇格中」で追う（`releases-screen.tsx`） |
| `/reports`・`/graph`・`/plans/new`・`/tasks/new`・`/help`・`/login` | `FetchFrame`（取得のある画面） | 0 件の文 | `ErrorNotice`。`/login` は「gateway に接続できません」「パスワードが違います」を自前で出す（`routes/login.tsx`） | — | 帯のみ | — | なし |

まとめ:

- loading と error は `FetchFrame` で全画面ほぼ揃う。ただし error は理由を出さず、before の取得系画面はすべて同じ「取得に失敗しました。」（例: `_tasks-1440.png`）で、未接続・権限・存在しないの区別がつかない。
- empty は各画面の文で、次に何をするか（作る・絞り込みを外す）を示す画面は少ない。
- stale は「再取得失敗で古い値を残す」ことまでで、いつの値かは出さない。SSE 切断中に古い値を見ていることは画面からは分からない。
- disconnected は停止の帯 1 本。スマホ幅では接続状態の表示自体が隠れている。
- permission-denied は files・artifacts・run log の 3 か所だけで、管理系の画面で 403 が返った場合は一般の error と同じになる。
- 破壊的操作の確認は `<dialog>`・`window.confirm`・checkbox・確認なしの 4 通りが混在する。却下・中止・削除・昇格のうち確認が無いのは `/inbox`・`/approvals`・`/knowledge`・`/knowledge/skills`・`/releases`・`/tasks/T1`。

## cosmetic と IA/component 層の切り分け

判定の基準は quality gate の Rescue 手順（cosmetic fix / page-level refactor / component-layer refactor / restart）に合わせる。情報の並びと操作の置き場を変えずに直せるもの（余白・文字の階層・色・境界・折返し・文言）は **cosmetic**。主操作の発見性、情報の優先順、一覧と詳細の行き来、危険度の違う操作の混在、状態の誤認は、画面の構成（**IA**）か共通部品（**component**）を作ってから直す。cosmetic だけで直すと、各画面が直書きの class を個別に直すことになり、上の「component hierarchy」の局所的なずれが増える。

### 共通部品（component 層から直す）

| component | cosmetic で足りる所 | IA/component 層から直す所 |
|---|---|---|
| shell（`web/components/shell/shell.tsx`, `nav-items.ts`） | nav の現在位置の強調、badge の `?` の見た目、header の高さ | 17 項目の平らな nav を「日々の仕事（受信箱・タスク・案件…）」と「管理（daemon・プロバイダ・リリース…）」に分ける。360〜412 で接続状態と受信箱・承認の badge が隠れる問題は header に常時出す置き場を作る |
| page header（`screen-frame.tsx`） | h1 の大きさ・余白 | パンくず・状態・主操作の置き場を持つ page header を作る。`/tasks/T1` の tab、`/projects/P1` の「文書」「ボード」、`/tasks/T1/files` の戻り先をここに寄せる |
| 取得の枠（`fetch-frame.tsx` の `ErrorNotice`） | 灰色の棒・文言の調子 | error を理由別（未接続・権限・存在しない・server error）に分け、いつの値か（stale）と再試行の結果を出す。`/tasks/T1/runs/R1` の固有 error も同じ部品へ |
| list/table（各 feature の `<ul>` 直書き） | 行の余白・区切り線の統一 | 一覧の行の型（題名・状態 badge・補助情報・行の操作）を 1 つ作る。`/inbox`・`/approvals`・`/projects`・`/reports`・`/knowledge` の行をそれに載せる |
| form・button（`web/components/ui/button.tsx`） | label と入力の間隔、focus の輪郭 | button に主・副・危険の区別を足し、field（label・説明・検証の文）の部品を作る。`/board` の 9 欄・`/providers`・`/accounts` の縦長 form はこの部品と折りたたみで短くする |
| 確認（`project-ops.tsx` の `<dialog>`, `window.confirm`） | なし（見た目を揃えるだけでは解決しない） | 破壊的操作の確認部品を 1 つにし、影響範囲を本文に書く。確認なしの `/inbox` の却下・`/approvals` の常設許可・`/knowledge/skills` の削除・`/releases` の昇格・`/projects/P1/docs/maintenance` の適用に入れる |
| 状態の表示（生の status 文字列） | 色・badge の形 | status を人の言葉と badge に写す共通の表（`/projects/P1` の `Overview`・`PlanDag`、`/graph` のノード、`/reports` の level、`/clusters` の接続状態）を作る。色だけに頼らず文字も出す |
| `web/styles.css` | — | 色・余白・文字の token を定義する（`neutral-*` 直書きの置き換え）。cosmetic の直しはすべてこの token 経由にする |

### 画面ごと

| 画面（fixture） | cosmetic で足りる所 | IA/component 層から直す所 |
|---|---|---|
| `/` | 空の初期画面の余白を詰め、入力欄までの距離を縮める | なし（Console の構成は妥当。キーボード表示時の重なりは実機確認） |
| `/login` | 失敗理由の文の位置・強調 | なし（戻り先 `next` の表示は文言の追加で足りる） |
| `/help` | 目次と節の輪郭を変え、カードの連続をやめる | 状況から探す入口（「失敗した task を直す」など）と検索。関連画面へのリンクを節の頭へ |
| `/org/cos` | 宛先の表示（名前と役割を ID より前に） | なし |
| `/tasks` | 状態 8 ボタンの折返し、1440 の右の空白 | 絞り込みを折りたたみ、表を広い幅で列を増やす（list/table 部品） |
| `/tasks/new`・`/plans/new` | 受け入れ条件の欄の高さ | 計画の作成と通常の task 作成の違い・作成後の流れを form の前に示す（IA） |
| `/tasks/T1` | tab のラベルの言語を揃える | 現在状態と要判断を先頭に置き、`decision-panel.tsx` の多数の操作を危険度で分ける（IA + 確認部品） |
| `/tasks/T1/files` | 長い path の折返し | 空と取得失敗を分ける（取得の枠） |
| `/tasks/T1/changes` | 差分の行の色・行番号 | 破棄を取り込み方法の select から外し、危険の button と確認にする（IA + 確認部品） |
| `/tasks/T1/runs/R1` | 発話と tool の枠の色分け | 再試行の導線、現在位置（実行中の末尾へ）の置き場（取得の枠） |
| `/graph` | 状態の色分け、辺の矢印 | ノードを task 詳細へのリンクにし、キーボードで辿れるようにする。root の入力を task 選択にする（component） |
| `/inbox` | 0 件の区画を 1 行に縮める | 区画を判断の種類ごとの一覧の型に載せ、行の操作に危険度を付ける（list/table + 確認部品） |
| `/approvals` | 3 区画の取得失敗の重複表示 | 一回許可と常設許可の影響差を押す前に示す（IA + 確認部品） |
| `/reports` | 行の level・種別の表示 | 通知設定・試験ボタンを報告の一覧より後ろへ下げる（IA） |
| `/artifacts` | 行の余白 | 案件を選ばずに最近の成果物を出す初期状態（IA） |
| `/projects` | archive の checkbox の大きさ | 作成フォームを主操作から外し、一覧の行に未処理数・最終更新を足す（IA + list/table） |
| `/projects/P1` | 区画の見出し | 概要 → 計画 → 仕事の木を先にし、`ProjectOps` などの編集を後ろか別画面へ（IA）。「ボード」を案件で絞る |
| `/projects/P1/docs` | 一覧の行の密度 | 狭い幅で一覧を折りたたむ list-detail（component） |
| `/projects/P1/docs/maintenance` | `h2` の装飾 | JSON の `pre`・textarea を人が読める監査結果と整理案の表示に作り直し、適用に確認を付ける（restart 相当） |
| `/board` | カードの余白、列の見出し | 絞り込みの折りたたみと候補付きの入力、カードの編集を詳細へ移す（IA + form 部品） |
| `/knowledge` | 1440 の検索欄の幅、空の本文枠 | 結果の行に scope・更新日・抜粋を足し、狭い幅で選んだ本文へ移る（list/table + list-detail） |
| `/knowledge/inbox` | 題名の 2 重表示 | 結果を押した候補の近くに出し、却下に確認、既存ページとの差分を出す（IA + 確認部品） |
| `/knowledge/skills` | 空の右枠 | 削除に確認と影響範囲（mount している課）を出す（IA + 確認部品） |
| `/org` | 木の button の余白 | 追加フォームを木から出し、選んだ担当の詳細を先に（IA）。削除を共通の確認へ |
| `/daemon` | 取得時刻の位置、英語の文 | `last_tick_at` を「N 秒前」と stale の判定で出し、件数から該当 task へ移る（IA） |
| `/providers` | 1440 の 2 列の欄の伸び | 使えるかどうかを先頭の badge にし、編集を行の展開へ。reload の失敗を押したカードに出す（IA + form 部品） |
| `/accounts` | 節の見出し | account・secret・MCP クライアントを別の区画か tab に分ける（IA） |
| `/clusters` | 1 行の文を区切る | 接続状態を badge にし、作業ディレクトリの現在値を欄に入れる。disconnected と stale を分ける（component） |
| `/releases` | 稼働中・current・previous の文の強調 | 昇格を確認付きにし、昇格の状態を該当の行に出す。current の disabled の理由を示す（IA + 確認部品） |

まとめると、cosmetic だけで済むのは `/`・`/login`・`/org/cos` の 3 画面だけ。残りは共通部品（page header・一覧の行・button の階層・確認・状態の badge・token）を先に作ってから画面へ当てる方が手戻りが少ない。`/projects/P1/docs/maintenance` は画面の作り直しが要る。

## 4 群の割り当て

foundation は shell・login・home・help と、全画面が使う共通部品（Console を含む）の群。`/org/secretary`・`/org/$id` は Console（`features/console/console-view.tsx`）を home と共有するので foundation に入れる。台帳の 31 route（30 unique fixture）を 1 行 1 画面で割り当てる。

| 画面（台帳の route） | fixture | 群 | 理由 |
|---|---|---|---|
| / | `/` | foundation | home。Console と shell の既定の行き先 |
| /login | `/login` | foundation | 認証 gate（`routes/__root.tsx`）の行き先 |
| /help | `/help` | foundation | 全画面の説明と導線 |
| /org/secretary | `/org/cos` | foundation | home と同じ Console。旧 route からの転送 |
| /org/$id | `/org/cos` | foundation | 人ごとの Console |
| /tasks | `/tasks` | task・run 系 | task 一覧 |
| /tasks/new | `/tasks/new` | task・run 系 | task の作成 |
| /plans/new | `/plans/new` | task・run 系 | 計画（root task）の作成。`create-screen.tsx` を `/tasks/new` と共有 |
| /tasks/$id | `/tasks/T1` | task・run 系 | task 詳細と判断 |
| /tasks/$id/files | `/tasks/T1/files` | task・run 系 | task の作業ツリー |
| /tasks/$id/changes | `/tasks/T1/changes` | task・run 系 | task の差分と取り込み |
| /tasks/$id/runs/$runId | `/tasks/T1/runs/R1` | task・run 系 | run ログ |
| /graph | `/graph` | task・run 系 | task の依存関係 |
| /inbox | `/inbox` | inbox・project 系 | 人の判断の入口 |
| /approvals | `/approvals` | inbox・project 系 | 認可の判断（受信箱から続く） |
| /reports | `/reports` | inbox・project 系 | 上がってくる報告 |
| /artifacts | `/artifacts` | inbox・project 系 | 案件単位の成果物 |
| /projects | `/projects` | inbox・project 系 | 案件一覧 |
| /projects/$id | `/projects/P1` | inbox・project 系 | 案件詳細 |
| /projects/$id/docs | `/projects/P1/docs` | inbox・project 系 | 案件の文書 |
| /projects/$id/docs/maintenance | `/projects/P1/docs/maintenance` | inbox・project 系 | 案件の文書の保守 |
| /board | `/board` | inbox・project 系 | 案件の task を列で見る |
| /knowledge | `/knowledge` | inbox・project 系 | 案件をまたぐ知識の閲覧 |
| /knowledge/inbox | `/knowledge/inbox` | inbox・project 系 | 知識候補の判断 |
| /knowledge/skills | `/knowledge/skills` | inbox・project 系 | skill の閲覧と編集 |
| /org | `/org` | 管理系 | 組織の構成 |
| /daemon | `/daemon` | 管理系 | dispatcher の状態 |
| /providers | `/providers` | 管理系 | provider の設定 |
| /accounts | `/accounts` | 管理系 | account・secret・MCP |
| /clusters | `/clusters` | 管理系 | cluster の接続 |
| /releases | `/releases` | 管理系 | release の昇格 |

群ごとの数: foundation 5 route（4 fixture）、task・run 系 8、inbox・project 系 12、管理系 6。合計 31 route・30 fixture。

## quality gate critique

`.claude/skills/ui-ux-quality-gate/SKILL.md` の Surface type では、web/ は「ops workbench」と「admin」の混在で、既定の方針は「Admin / ops pages prioritize state, issues, and next action over decoration」。この観点で before を見ると、問題は装飾過多ではなく、**状態と次の操作が先に読めない**ことと、**部品の型が無いための局所的なずれ**（Anti-pattern の「Local consistency drift」）にある。以下、4 つの兆候ごとに具体の画面を挙げる。

### generic AI dashboard

- 統計ヒーローや飾りのグラフは無く、典型的な generic AI dashboard ではない。ただし「Default component-library look with no project-specific adaptation」に当たる。`web/styles.css` が `@import "tailwindcss";` だけで、全画面が `neutral-*` の灰色・`rounded border` の枠・同じ button で、Celeris 固有の状態（人の待ち・stale・昇格中）を表す視覚の語彙が無い。
- `/daemon` は dispatcher の状態を件数の行で並べるだけで、「最後の tick が古い」という結論より生の `last_tick_at` が先に出る（「Raw technical states as primary copy」）。`/projects/P1` の `Overview`・`PlanDag`、`/reports` の level、`/graph` のノードも生の status 名をそのまま主表示にしている。
- `/projects/P1/docs/maintenance` は JSON の `pre` と JSON を書く textarea が主操作で、「Raw JSON as the primary interface for non-developer tasks」に当たる。

### card wall

- `/help` は目次と各節が同じ輪郭の枠で 1800px（360）続き、task の階層が無い card wall。
- `/inbox` は 0 件の区画カードが縦に連続し（360）、今処理すべき判断が見えない。
- `/board` の `BoardCard` は全カードに編集フォームを常に出し、カードが高く、列の一覧性が落ちる（「Table stuffed with complex configuration」の card 版）。
- `/providers`・`/accounts`・`/clusters` は 1 件ずつのカードに状態・入力・操作を同じ重みで詰め、カードの違いが名前だけ。
- `/knowledge/skills` と `/knowledge` は一覧の枠と空の本文枠が同じ輪郭で並び、空の枠が主役に見える。

### 過剰余白

- 1440 の main に `max-width` も 2 列の配置も無く、`/tasks`・`/tasks/T1`・`/tasks/new`・`/graph` は入力と button が左上に集まり右 1000px 以上が空く（`_tasks-1440.png`, `_tasks_T1-1440.png`）。高密度の ops workbench として幅を使えていない（「empty leftover space is intentional, not unfinished layout residue」に反する）。
- `/knowledge` の 1440 は検索欄が全幅に伸びる一方、本文の枠は「知識を選択してください。」だけで右側が大きく空く。
- `/` と `/org/cos` の 360 の空の初期画面は、入力欄まで大きな空白が続く。
- 逆に 360〜412 では form の 1 項目が 76px 前後を使い、`/board`（952px）・`/providers`（1158px）の一覧が最初の画面に出ない（「Mechanical mobile stacking without task reordering」）。

### unstyled admin

- `/providers` の 1440 は concurrency・model の欄が全幅の 2 列に伸び、tier の checkbox と 3 つの button が同じ重みで並ぶ。使えるかどうかの状態は小さな文の行。
- `/accounts` は account・secret・MCP クライアントが `h2` だけを切れ目に縦に続き、「確認」「ログイン開始」「削除」が同じ見た目。
- `/clusters` は「pegasus.example / auth totp / 未接続」が 1 行の文で、接続状態の badge が無い。
- `/releases` は稼働中・current・previous が 1 行の小さな文で、本番を切り替える昇格の button が確認なしで他と同じ見た目（「Dangerous / destructive actions without confirmation or undo」）。
- `/org` の担当の削除は `window.confirm`、`/projects/P1` は `<dialog>`、`/knowledge/skills` は確認なし、と確認の作りが画面ごとに違う。

### その他の gate 項目

- 復帰（「No recovery path after error, permission denial」）: before の取得系画面はすべて同じ「取得に失敗しました。」で、未接続・権限・存在しないの区別が無い。`/tasks/T1/runs/R1` は再試行の導線が無い。
- スマホでの主タスク: 360〜412 で接続状態（disconnected）と受信箱・承認の badge が隠れ、`/knowledge`・`/org`・`/projects/P1/docs` は選んだ詳細が画面外に出る。
- アクセシビリティ: `/graph` のノードはリンクでもボタンでもなくキーボードで辿れない。状態の色分けを足すときは文字も併記する。
- これは before 画像とコードからの critique で、撮影画像は操作性・アクセシビリティ gate の合格証明ではない。直した後は同じ 4 幅で after を撮り、`pnpm mobile-audit` 相当のタップ領域と focus を確かめる。

## before screenshot

- 保存先: `artifacts/before`（WU artifacts 絶対 path: `/var/lib/celeris/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before`）。
- 撮影コマンド: `corepack pnpm@12.6.0 -C web screenshots -- --out /var/lib/celeris/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before`
- PNG: **120 枚**（31 route / 30 unique fixture × 360/390/412/1440）。幅は各 viewport の CSS px。
- 命名規則: fixture の英数字以外を `_` に置換し、末尾に `-<width>.png` を付ける。root `/` は `_` なので `_-360.png` 等。同一 fixture の route は同じファイル名になる。
