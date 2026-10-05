---
title: Web work 画面群の visual QA（受信箱・通知・案件・board/home・報告・承認）
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: running
updated: 2026-10-04
---

# Web work 画面群の visual QA（受信箱・通知・案件・board/home・報告・承認）

対象: `/`・`/inbox`・`/notifications`・`/projects`・`/projects/P1`・`/projects/P1/docs`・`/projects/P1/docs/maintenance`・`/board`・`/reports`・`/approvals`。
基点 45a8fde5（qa-shared・qa-fixtures 統合後）。

## pre screenshot

- 置き場所: WU `critique` の artifacts `qa-qa-work-pre/`（`<task>/wu/critique/artifacts/qa-qa-work-pre`）。
- コマンド: `corepack pnpm@12.6.0 -C web build` → `corepack pnpm@12.6.0 -C web screenshots --out <dir>`（128 枚 / 32 画面）→ 同 `--states --out <dir>`（116 枚 / 8 状態）。幅は 360/390/412/1440。
- work 画面の名前: `_-<w>`（ホーム）・`_inbox-<w>`・`_notifications-<w>`・`_projects-<w>`・`_projects_P1-<w>`・`_projects_P1_docs-<w>`・`_projects_P1_docs_maintenance-<w>`・`_board-<w>`・`_reports-<w>`・`_approvals-<w>`。状態は `long-text-_inbox`・`long-id-_inbox`・`empty-_inbox`・`empty-_notifications`・`many-_inbox`・`many-_notifications`・`loading-_inbox`・`error-_inbox`・`forbidden-_inbox`・`stale-_`。

### 撮影の制約（critique を読むときの注意）

- 全 PNG が高さ 800px で止まる。`fullPage: true` だが shell の構成で viewport 分しか写らない。360 では受信箱の選択肢・案件詳細の操作節・成果物の節が写っておらず、それらはコードを読んで判定した。
- `error-_inbox-360.png` は `loading-_inbox-360.png` と md5 が同じ（screenshots.mjs は goto 直後に撮り、query-client の再試行 2 回を待たない）。error 状態の見た目は未確認。
- `long-id-_inbox-360.png` は `_inbox-360.png` と md5 が同じ（長い ID の項目は fold の下）。
- 偽 daemon の既定 fixture に `/api/v1/reports`・`/api/v1/approvals`・standing-rules・`/projects/P1/docs` 系が無く、`/reports`・`/approvals`・文書 2 画面は取得失敗の姿しか撮れていない。`/projects`・`/projects/P1`・`/board` はデータ入り。

## critique

ui-ux-quality-gate の Frontend Thinking Gate（要約）:

- Project stage: half-built（shell・primitive は qa-shared で整備済み、画面は旧構成が残る）。Surface type: ops workbench（受信箱・ホーム・案件）＋ admin（承認の常設ルール・文書の保守）。
- Primary user task: 受信箱で判断を返す / ホームで「いま自分に要ること」を知る / 案件 → 途中目標 → 根の仕事 → 子の仕事 の関係を読む / 報告を読む / 認可の記録と常設ルールを確かめる。
- What must not appear: 内部語（葉・run・node ID・enum 値・JSON）、確認のない破壊的操作、意味の無い取得失敗文。
- Key anti-pattern risks: Raw technical states as primary copy / Dangerous actions without confirmation / Mechanical mobile stacking / Default component-library look。
- 判定: generic AI dashboard 化はしていない（hero・stat card・card wall は無い）。弱点は「状態と次の操作が先に読めない」「内部値がそのまま主表示に出る」「確認の無い危険な操作」の 3 つに集中する。受信箱の判断構造は選択肢と結果の対は読めるが、問いが行の最後にあり、案件 → task の関係は案件詳細の仕事の木が折返し禁止で列が隠れるため読めない。

担当 leaf: fix-inbox＝ホーム/受信箱/通知、fix-projects＝案件/文書/board、fix-reports＝報告/承認。「—」は共通部品（`web/components/`・`web/scripts/`）側で、この task の範囲外（残課題へ）。

| ID | 画面 | screenshot 名 | 重さ | 担当 leaf | 指摘 | 直し方の案 |
|---|---|---|---|---|---|---|
| W-01 | /inbox | `_inbox-360`・`_inbox-1440`・`many-_inbox-360` | 高 | fix-inbox | 何を決めるのか（`item.detail`）が推奨・期限・止めている範囲・案件・待ち・関連の後、行の最後に出る（`inbox-screen.tsx:341-345`）。360 では問いが fold 近くまで下がり、選択肢は画面外。 | 見出し直下に問い。並びは 問い → 選択肢と結果 → 推奨 → 期限 → 対象/案件。止めている範囲・関連・待ちは折りたたむ。 |
| W-02 | /inbox | `_inbox-1440` | 高 | fix-inbox | 破壊的でないと判定された選択肢は 1 クリックで送信され行が消える（`inbox-screen.tsx:247-254`）。判定は option key の固定リスト（`inbox-model.ts:48-58`）で、「計画をやり直す」や推奨でない選択も確認なし。確認文自身が「受信箱からは取り消せません」と書く。 | 推奨でない選択・差し戻し・取り下げは ConfirmDialog か数秒の取り消し。破壊判定を key 名の推測に頼らない。 |
| W-03 | / | `_-360`・`_-1440` | 高 | fix-inbox | 自分の対応が要るものは「受信箱 判断待ち 5 件」「通知 未読 3 件」の同じ見た目の枠 2 つだけ（`routes/index.tsx:17-46`）。期限 9 時間後の判断があっても知らせと同じ重み。 | 判断待ちの上位数件（題名・期限・案件）を会話の上に置き、期限の近いものを warning。通知は件数の入口に落とす。 |
| W-04 | / | `stale-_-360`・`stale-_-1440` | 高 | fix-inbox（shell 部分は —） | SSE 切断中に変わるのは shell の「再接続中」badge だけ。件数・会話は鮮度を示さず、360 では最終受信も隠れる（shell の最終受信は `md:block`）。 | 画面側で再接続中は件数の横に「未確認」を添え、会話の上に全幅の注意を出す。shell の最終受信の 360 表示は残課題。 |
| W-05 | /projects/P1 | `_projects_P1-360`・`_projects_P1-1440` | 高 | fix-projects | 仕事の木の題名セルが `whitespace-nowrap`（`project-detail-view.tsx:207`）。長い題名が枠を占め、1440 でも「状態」「判断待ち」列が見えず、横 scroll の手掛かりも無い。案件 → task の関係が読めない。 | 題名を折り返す（`break-words`＋最小幅）。狭い幅では状態を題名の下へ（board と同じ形）。 |
| W-06 | /projects/P1/docs/maintenance | `_projects_P1_docs_maintenance-360`（コード） | 高 | fix-projects | 監査結果・保存済み報告・結果が `JSON.stringify` の `pre`、整理案とポリシーは JSON を書く textarea（`project-docs-maintenance-screen.tsx:47,57-59,72-77,91-96`）。人が読んで判断する画面になっていない。 | 監査を指摘の一覧、整理案を移動・削除・統合の表に。JSON は details の中へ。 |
| W-07 | /projects/P1/docs/maintenance | （コード） | 高 | fix-projects | 「承認済み案を適用」「整理案を承認」「ポリシーを採用」が確認なしで即送信（同 `:80-85,98`）。FRONTEND_CONTRACT 規則 6 に反する。 | ConfirmDialog。対象 file 数・影響・戻し方・結果 task を示し、確定ラベルは「整理案を適用する」。 |
| W-08 | /projects/P1/docs | （コード） | 高 | fix-projects | 文書の削除が `window.confirm`（`project-docs-screen.tsx:101`）。削除ボタンも既定の見た目（`:192`）。 | 共通 ConfirmDialog と destructive variant。初期 focus は「戻る」。 |
| W-09 | /projects/P1/docs・/maintenance | `_projects_P1_docs-360/1440`・`_projects_P1_docs_maintenance-360/1440` | 高 | fix-projects | 失敗表示が「取得に失敗しました。表示を更新できません。」だけで何が取れなかったか不明（FetchFrame に `subject` なし。docs `:128`・maintenance `:49`）。保守画面は 409（文書リポジトリなし）を区別せず、文書画面の「文書を用意する」導線と食い違う。 | `subject="案件の文書"` 等を渡す。保守画面も 409 を「文書リポジトリがありません → 文書を用意する」に。 |
| W-10 | /approvals | `_approvals-360`・`_approvals-1440` | 高 | fix-reports | 「常設ルールを追加」が確認なしで即追加（`approvals-screen.tsx:221-226`）。以後一致する認可が自動で通る常設許可なのに、削除側（`:192-210`）にだけ確認がある。 | 追加にも ConfirmDialog。対象 node・自動で認める範囲・取り消し方を示し、確定ラベルは「常設ルールを追加して自動承認する」。 |
| W-11 | /approvals | `_approvals-360`・`_approvals-1440` | 高 | fix-reports | 常設ルール一覧の取得に失敗しても追加フォームは使えるまま（FetchFrame `:179` の外に `:218`）。既存を確かめられない状態で重複・広すぎる規則を足せる。 | 一覧が error・permission-denied の間は追加を止めるか「既存のルールを確認できません」を添える。 |
| W-12 | /approvals | （コード） | 高 | fix-reports | 決めたものの Badge が `denied` 以外すべて success（`:304`）。「取り下げ」が緑、「今後も認めた」と「今回だけ認めた」が同じ見た目。 | standing は warning、withdrawn は neutral。常設には常設ルールへの参照を添える。 |
| W-13 | /inbox | `_inbox-360`・`_inbox-1440` | 中 | fix-inbox | 止めている範囲が「葉 auth を止めている　web の認証　葉 auth」と重複し、内部語「葉」がそのまま出る（`inbox-screen.tsx:124-137`）。選択肢の説明も「planner に差し戻す」「葉を走らせる」。 | summary があれば units を出さない。「葉」→「作業単位」など。effect 文の語彙を揃える。 |
| W-14 | /inbox | `many-_inbox-360`・`many-_inbox-1440` | 中 | fix-inbox | 40 件すべてに textarea と全選択肢が常設され、form の壁になる（`inbox-screen.tsx:296-347`）。期限順・まとめが無く Tab 移動が非常に長い。 | 一覧は 種類・問い・期限・推奨 の 1 行。回答欄は開いた 1 件（または Drawer）だけ。期限の近い順。 |
| W-15 | /inbox | `_inbox-1440` | 中 | fix-inbox | 行内リンクに `min-h-11 inline-flex`（`inbox-screen.tsx:32`）。リンクを含むメタ行だけ 44px 高く、推奨・期限との間隔が不揃いで間延びする。 | メタは dl の 2 列 grid。44px の操作は行末の「開く」に集め、本文内は参照名だけ。 |
| W-16 | /notifications | `_notifications-360`・`_notifications-1440`・`many-_notifications-360` | 中 | fix-inbox | 元 task へのリンク名が「タスク」だけ、代替も `task ${id}`（`notice-view.ts:39-41`）。案件は `案件 ${project_id}` の生 ID（`notifications-screen.tsx:97`）で、取得済みの案件名 query（`:133`）を使っていない。 | リンク名を対象の題名に。案件は title を出し ID は補助へ。 |
| W-17 | /・/inbox・/notifications | `_-360`・`_inbox-*`・`_notifications-*` | 中 | fix-inbox | 英語の内部語: 「理由・note」（`inbox-screen.tsx:207`）、「draft の受け入れ」（`inbox-model.ts:32`）、「task の完了」（`notice-view.ts:10`）、ホームの「run の作業 standard」「tool 2 回」、入力欄の名前「Console への入力」。 | 「理由（メモ）」「下書きの受け入れ」「タスクの完了」「作業の実行」に統一。用語表を共有。 |
| W-18 | /notifications | `_notifications-360`・`_notifications-1440` | 中 | fix-inbox | 未読 3 件なのに「すべて既読にする」が無効状態の見た目（枠なし・灰色）。行の「既読にする」（白地・枠）と違う。`disabled={!unreadTotal}`（`:157`）と件数は同じ query なので、variant の見た目か描画の瞬間の問題。 | 有効時は secondary の枠付きにし、無効なら理由を添える。実機で class を確かめる。 |
| W-19 | /inbox | `forbidden-_inbox-360` | 中 | fix-inbox（文言は —） | 403 は一般の error と区別できている。ただ「一覧へ戻る」の行き先が `/`（ホーム）で文言と合わず、`<a href>` で全画面を読み直す（`fetch-frame.tsx:179-182`）。必要な権限・誰に頼むかも無い。 | 画面側で「受信箱を見るには ○○ の権限が要ります」と対象・依頼先を示す。戻り先の文言と Link 化は共通部品側の残課題。 |
| W-20 | /inbox | `loading-_inbox-360`（＝`error-_inbox-360`） | 中 | fix-inbox（語の統一は —） | 読み込み中は 64px の灰色の帯だけ（skeleton 未指定）。error 文も `subject` 未指定（`inbox-screen.tsx:487`）で「取得に失敗しました」と一般的。ボタン「再試行」と文中「再取得」が不一致（`fetch-frame.tsx:107-111`）。 | 項目行の形の skeleton。`subject="判断待ち"`。語の統一は fetch-frame 側の残課題。 |
| W-21 | /projects/P1 | （コード。800px の外） | 中 | fix-projects | DAG と成果物の間に常に開いた操作節 3 つ（`routes/projects.$id.index.tsx`・`project-detail-view.tsx:310`）。form 約 10 個、primary 5 つ（`project-ops.tsx:116,240,302,327,536`）。読む内容が操作の壁に埋もれる。 | 操作は「編集」「計画を立てる」の details か Drawer に畳む。primary は節ごと 1 つ。 |
| W-22 | /projects/P1 | （コード） | 中 | fix-projects | 根の仕事ごとに「計画を承認」「段階を通す」「止める」「再開」を状態に関係なく全部出す（`project-ops.tsx:353-397`）。 | `task.actions` か状態で、そのとき押せる操作だけ出す。 |
| W-23 | /board | `_board-1440`・`_board-360` | 中 | fix-projects | 優先度・レベル・種類の列に内部の英語値（normal / standard / feature）がそのまま（`board-screen.tsx:124,130-133`）。編集の select も cheap/standard/frontier（`:83`）。 | tier・category の日本語ラベル対応表を共通化。未知の値は「未確認」。 |
| W-24 | /projects/P1 | `_projects_P1-1440`（コード） | 中 | fix-projects | 根の task の集計が英語 status 名（`/ ready 1`、`project-detail-view.tsx:100`）、DAG の「依存:」が内部 key（`:143`）、リポジトリ欄のラベルに repo.id（`project-ops.tsx:448,452`）。 | 状態はラベル対応表。依存は相手の題名。ID は details へ。 |
| W-25 | /board | `_board-1440` | 中 | fix-projects | 「すべての案件」のとき行に案件名が無く（`board-screen.tsx:113-133`）、どの案件・途中目標・親に属するか読めない。 | 案件名（と途中目標）の列か、題名の下に 1 行。 |
| W-26 | /projects | `_projects-360` | 中 | fix-projects | 360 で表が横にはみ出し「判断待ち」が「判…」「2 ·」に切れ、最終更新が見えない。scroll の手掛かりも無い。 | 狭い幅は 2 段組（題名 / 状態・判断待ち・更新）。 |
| W-27 | /projects | `_projects-1440` | 中 | fix-projects | 作成フォームが常に開き、一覧（1 行）より大きい（`project-list-screen.tsx:216`）。1440 は右半分が空く。UX_AUDIT の friction のまま。 | 見出し横に「案件を作る」ボタン、フォームは開閉か Drawer。 |
| W-28 | /projects・/projects/P1 | （コード） | 中 | fix-projects | 「判断待ち N 件」が絞り込みなしで `/inbox` へ（`project-list-screen.tsx:168`・`project-detail-view.tsx:229`）。受信箱は `?project=` を受ける（`routes/inbox.tsx:9`）。 | `search={{ project: id }}` を渡す。 |
| W-29 | /projects/P1 | （コード） | 中 | fix-projects | 「作業場所を消す」が即送信（`project-ops.tsx:210-218`）。状態 select で「完了」へ確認なしに変えられる（`:123-135`）。 | 両方 ConfirmDialog。 |
| W-30 | /projects/P1/docs/maintenance | （コード） | 中 | fix-projects | JSON.parse に失敗すると黙って return（`:24-25`）。押しても反応が無い。 | 欄に結び付けた `role="alert"` のエラー。 |
| W-31 | /approvals | （コード） | 中 | fix-reports | 履歴の行は decision と question だけ（`:303-310`）。型にある `node_id`・`task_id`・`project_id`・`decided_at`・`answer` を出さず、誰の・どの task の・いつの認可か読めず元 task へ戻れない。 | 依頼した課・task リンク・日時・回答を補足行に。 |
| W-32 | /reports | （コード） | 中 | fix-reports | Badge が `{kind} / 段 {level}`（`reports-screen.tsx:38-40`）で `bad_news`・`result` の英語値と数字の段がそのまま。フィルタは同じ段を CoS/部/課 と表示（`:115-117`）し食い違う。全部 neutral で悪い知らせが目立たない。 | kind を和名（悪い知らせ・結果・提案・質問・進捗）。bad_news は danger、question は warning。段の語をフィルタと揃える。 |
| W-33 | /reports | （コード） | 中 | fix-reports | 行に送り手（`node_id`）も元 task・案件へのリンクも無い（`:35-49`）。 | 見出し下に「送り手・タスク名（リンク）」。question・proposal には受信箱への導線。 |
| W-34 | /approvals | `_approvals-360`〜`-1440` | 中 | fix-reports | 同文の失敗箱が 2 つ並ぶ。両 FetchFrame とも `subject` 無し（`:179,:295`）。 | `subject="決めた認可"`・`"常設ルール"`。両方失敗なら上部に 1 つにまとめる。 |
| W-35 | /approvals | `_approvals-360`・`_approvals-1440` | 中 | fix-reports | 追加フォームが「対象の node ID」の自由入力と例の無い「規則文」だけ（`:231-252`）。登録済み行も「対象: {node_id}」（`:189`）。 | node を組織から選ぶ。規則文に例と一致範囲の説明。行に課の名前。 |
| W-36 | /reports | （コード） | 中 | fix-reports | 各行の開閉ボタンが全部「展開」で `aria-controls` 無し（`:46-48`）。読み上げで区別できない。 | `aria-label="「{headline}」を展開"`＋`aria-controls`。見出しを button にしてもよい。 |
| W-37 | /projects/P1/docs・/maintenance | `_projects_P1_docs-1440` | 低 | fix-projects | nav リンクが `min-h-11 underline` のインライン要素で min-height が効かず 44px 未満（docs `:120,123,185`・maintenance `:36,42`）。 | `inline-flex items-center min-h-11 min-w-11`（詳細画面の nav と揃える）。 |
| W-38 | /projects/P1/docs/maintenance | （コード） | 低 | fix-projects | h2 に class が無く本文と同じ見た目（`:53,70,89`）。ボタンもすべて既定。 | ProjectSection の見出し・枠に揃える。 |
| W-39 | /projects・/board | `_projects-360`・`_projects-1440` | 低 | fix-projects | checkbox が `h-11 w-11` / `size-11` で空の大きな四角に見え、入力欄と紛れる（list `:197`・board `:290`）。 | 絵は 20px、44px は label の当たり判定で確保。 |
| W-40 | /projects | `_projects-1440` | 低 | fix-projects | 途中目標列の「—」が読み込み中・0 件・取得失敗で同じ（`project-list-screen.tsx:162`・`project-structure.ts:127`）。 | 0 件「なし」、取得中 Skeleton、失敗「取得不可」。 |
| W-41 | /board | `_board-360` | 低 | fix-projects | 説明文「行の『編集』から変えます」だが edit 操作の無い task にはボタンが出ない（`:135,243`）。360 では絞り込みと状態 chip 7 個で約 600px を使い最初の行が下。 | 説明文を削る。狭い幅は検索だけ出し案件 select は details へ。 |
| W-42 | /projects/P1・/docs | （コード） | 低 | fix-projects | token 外の値: inline style `maxHeight`（`project-detail-view.tsx:116`）、任意値 `lg:grid-cols-[…]`（docs `:138`）、`text-lg`・裸の `rounded border`（docs `:141,181`）。 | DESIGN.md の token へ。 |
| W-43 | /projects 3 画面 | `_projects_P1-*` ほか | 低 | fix-projects（制約） | h1 が「案件の詳細 P1」と ID で、案件名は h1 下の `<p>`（`project-detail-view.tsx:279,283`）。parity が h1 を見るので h1 は変えられない。 | h1 直下の案件名を見出し相当の大きさにし ID は補助の見た目に落とす（h1 文字列は不変）。 |
| W-44 | /inbox | `_inbox-1440`・`many-_inbox-1440` | 低 | fix-inbox | 種類 badge が failed 以外すべて info（`inbox-screen.tsx:300`）。DESIGN では人の判断待ちは warning。期限は過ぎて初めて強調（`:97-107`）で 9 時間後と期限なしが同じ。 | 判断待ちは warning 系。24 時間以内は「まもなく期限」で warning。 |
| W-45 | /notifications | `_notifications-1440`・`many-_notifications-1440` | 低 | fix-inbox | 1440 で「既読にする」が題名から約 1000px 離れた右端（`notifications-screen.tsx:103-116`）。60 件に同じボタンが並ぶ。 | メタ行の末尾に置くか、リンクを開いた自動既読を主にし補助操作へ。 |
| W-46 | /notifications | （コード） | 低 | fix-inbox | リンクを開いたときの自動既読が失敗しても `() => undefined` で黙殺（`:59-62`）。 | 行内 ActionResult に出すか未読数を再取得。 |
| W-47 | / | `_-1440` | 低 | fix-inbox | 1440 で会話が上 520px で終わり、固定入力欄まで約 200px 空く。入力欄に見える名前も placeholder も無く（`console-view.tsx:153-156`）focus 枠に offset が無い。 | 空きに判断待ちの要約。入力欄に「CoS に頼む・聞く」の見える名前、focus を共通の輪郭に。 |
| W-48 | /reports | （コード） | 低 | fix-reports | 段の `<select>`（`:25`）と通知へのリンク（`:97`）に focus-visible の指定が無く、approvals の fieldClass（`:158`）と揃っていない。 | 共通の field / link の variant に寄せる。 |
| W-49 | /reports | `_reports-360`〜`-412` | 低 | fix-reports | 狭い幅で「通知で報告の知らせを見る」が見出し下の独立行になり一覧より上を占める。フィルタ label「段」だけで意味が伝わらない。 | リンクは見出し行の補助へ。label は「報告元の階層」。 |
| W-50 | /reports・/approvals | 全 8 枚 | 低 | fix-reports | 「〜を読む画面です」「この画面は〜の記録です」と画面が自分を説明する文（anti-patterns 1A）。 | 利用者の行動を書く（「判断待ちの認可は受信箱で答えます」）。 |
| W-51 | 全画面 | `_-1440`・`_reports-1440` ほか | 低 | — | 直接開くと h1 に script focus が当たり focus 枠が出たまま撮れる（`screen-frame.tsx:25-27`）。h1 が全幅なので入力欄のような横長の箱に見える。 | 初回 script focus には枠を出さない、または h1 を `w-fit`。共通部品の残課題。 |
| W-52 | 取得失敗の全画面 | `_reports-1440`・`_approvals-360` ほか | 低 | — | 失敗文「〜できません。表示を更新できません。」が二重否定で冗長、文中「再取得」とボタン「再試行」が不一致（`fetch-frame.tsx:107-111`）。 | 「報告の一覧を読み込めませんでした。」と言い切り、ボタンを「再取得」に統一。共通部品の残課題。 |

### 重さ別の件数と leaf

| 担当 leaf | 高 | 中 | 低 |
|---|---:|---:|---:|
| fix-inbox | 4（W-01〜04） | 8（W-13〜20） | 4（W-44〜47） |
| fix-projects | 5（W-05〜09） | 10（W-21〜30） | 7（W-37〜43） |
| fix-reports | 3（W-10〜12） | 6（W-31〜36） | 3（W-48〜50） |
| —（共通部品・範囲外） | 0 | 0 | 2（W-51・52） |

### UX_AUDIT・screens 段の残課題との対応

- UX_AUDIT `/inbox`「0 件の区画が連続」: 新受信箱は単一の一覧になり解消。残るのは W-01・W-14（判断の型・密度）。
- UX_AUDIT `/projects` の作成フォームの比重・checkbox: W-27・W-39。`/projects/P1` の ops 先行・生の status・ボードが案件で絞られない: W-21・W-24（ボードリンクは `?project=` を渡すかを fix-projects で確認）。`/projects/P1/docs/maintenance` の JSON・確認なし・h2: W-06・W-07・W-38。`/board` の絞り込み 9 欄・横 scroll 列・常時編集 form: details への折りたたみと状態別の表に変わり解消。残るのは W-23・W-25・W-41。
- UX_AUDIT `/reports` の設定・試験ボタンの先行、`/approvals` の 3 ボタン並列: 通知画面と受信箱へ移って解消。残るのは W-32（生の kind/level）・W-10（常設の確認）。
- screens 段（`2026-10-04-web-inbox-notifications/`）の残課題: 受信箱の note 欄常設 → W-14。通知の案件名 → W-16。案件一覧の N 本取得は API 提案のまま（画面では W-40 の表示の区別のみ）。`/reports` の fixture 無し → 下の残課題。英語の内部語 → W-13・W-17・W-23・W-24・W-32。取得失敗の再試行導線は FetchFrame で全画面に揃い、残るのは subject 未指定（W-09・W-20・W-34）と語の不一致（W-52）。

## 修正

fix-inbox・fix-projects・fix-reports の各 leaf が、重い順に直した W-xx と commit を追記する。

## 残課題

- 撮影の制約（上記）: 800px で止まる fullPage、error 状態が loading と同じ、長 ID が fold の下。verify 葉の post 撮影では `data-fetch-state` の settle を待つか、少なくとも error・many の確認は e2e（`web/e2e/states/`）の結果で補う。`web/scripts/screenshots.mjs` の改修は本 task の範囲外なので提案に留める。
- `/reports`・`/approvals`・`/projects/P1/docs` 系は偽 daemon に既定 fixture が無く、データ入りの姿が未確認。fix-reports・fix-projects は画面固有の非 parity e2e で本文の状態を確かめる。
- W-51・W-52 は共通部品（`screen-frame.tsx`・`fetch-frame.tsx`）の変更が要り、この task では直さない。

## 提案

- `screenshots.mjs` に「`[data-fetch-state]` が loading でなくなるまで待つ（loading 状態は除く）」と、main の scroll 容器を含めた全高の撮影を足す。
- 偽 daemon の既定 fixture に報告 1 件・決めた認可 2 件（once/standing）・常設ルール 1 件・案件文書 2 件を足す。
