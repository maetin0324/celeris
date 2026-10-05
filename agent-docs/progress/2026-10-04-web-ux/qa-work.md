---
title: Web work 画面群の visual QA（受信箱・通知・案件・board/home・報告・承認）
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-05
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

3 つの fix leaf の記録（[fix-inbox](qa-work/fix-inbox.md)・[fix-projects](qa-work/fix-projects.md)・[fix-reports](qa-work/fix-reports.md)）を指摘 ID ごとにまとめる。commit: fix-inbox `9abbfd92`、fix-projects `9a8527d3`・`87b5e3e3`・`f720b40d`、fix-reports `bb8ce583`、verify `bd393ea1`。扱いは次の 3 種類: 「修正」は直した、「一部」は直したうえで残りを残課題へ回した、「確認」は変更が不要だった。

| ID | 扱い | commit | 要点 |
|---|---|---|---|
| W-01 | 修正 | 9abbfd92 | 見出しの直下に問いを置き、回答・推奨・期限・案件の順に並べた。止めている範囲と関連は折りたたんだ |
| W-03 | 修正（一部は V-01） | 9abbfd92 | ホームの会話より上に、期限の近い判断待ち 3 件を置いた（題名・期限・案件。24 時間以内は「まもなく期限」）。通知は件数の入口だけにした |
| W-04 | 一部 | 9abbfd92 | 再接続中・取得失敗のときは、ホームに鮮度の注意と「未確認」を出す。shell の最終受信時刻は残課題 |
| W-05 | 修正 | 9a8527d3・f720b40d | 仕事の木の題名を折り返すようにした（`min-w-48 break-words`）。1440 で状態・判断待ちの列が見え、状態 badge は折り返さない |
| W-06 | 修正 | 9a8527d3 | 監査は指摘の一覧、整理案は操作の表、方針は日本語で表示する。JSON は details の中へ移した |
| W-07 | 修正 | 9a8527d3 | 承認・適用・採用を ConfirmDialog にした（対象数・影響・戻し方・結果 task を示す） |
| W-08 | 一部 | 9a8527d3 | 削除ボタンを destructive にした。ConfirmDialog 化は残課題 |
| W-09 | 修正 | 9a8527d3 | 3 画面の FetchFrame に subject を渡した。409 は「文書リポジトリがありません → 文書を用意する」と出す |
| W-10 | 一部 | bb8ce583 | 送信前に自動承認の影響を常に表示し、追加直後に「取り消す」を出す。確認ダイアログは残課題 |
| W-11 | 修正 | bb8ce583 | 一覧が読めない間（loading・error・切断・403）は追加を止め、Notice で理由を出す |
| W-12 | 修正 | bb8ce583 | 結果ごとに色を分けた（今回だけ success／今後も warning／認めなかった danger／取り下げ neutral）。常設には常設ルールへの参照を添えた |
| W-13 | 一部 | 9abbfd92 | 範囲の重複を消し、画面側で作る語を「作業単位」にした。API が返す文の「葉」「planner」は残課題 |
| W-14 | 修正 | 9abbfd92 | 11 件以上は期限順の一覧にし、回答欄は同時に 1 件だけ開く |
| W-15 | 修正 | 9abbfd92 | メタ情報を desktop で 2 列にし、長い関連リンクは補助の節へ移した |
| W-16 | 一部 | 9abbfd92 | 案件は名前で出し、名前が無いときは ShortId にした。task リンクの名前は残課題 |
| W-17 | 一部 | 9abbfd92 | 「理由（メモ）」「下書きの受け入れ」に改めた。parity が固定している語は残課題 |
| W-18 | 確認 | — | 未読がある間はボタンが有効で、枠が 1px あることを e2e で確かめた。pre の薄い見た目は取得途中の姿だった |
| W-19 | 一部 | 9abbfd92 | subject「判断待ち」を渡し、403 では権限の確認を案内する。戻り先の文言と `<a>` は共通部品側の残課題 |
| W-20 | 一部 | 9abbfd92 | 行の形の skeleton と subject を足した。「再取得」と「再試行」の語の不一致は W-52 へ |
| W-23 | 修正 | 9a8527d3 | board の tier・category・priority を日本語で出す。未知の値は「未確認」 |
| W-24 | 一部 | 9a8527d3 | 集計を日本語の状態名に、依存を相手の題名にした。repo.id のラベルは残課題 |
| W-25 | 修正 | 9a8527d3 | 「すべての案件」のときは、行に「案件: <名前>」を出す |
| W-26 | 修正 | 9a8527d3 | sm 未満では途中目標と最終更新を題名の下へ回し、360 で枠に収めた |
| W-28 | 一部 | 9a8527d3 | 一覧の判断待ちは `/inbox?project=` へ送り、ボードには `?project=` を渡す。詳細側は残課題 |
| W-30 | 修正 | 9a8527d3 | JSON が読めないときは送らず、`role="alert"` で理由を出す |
| W-31 | 修正 | bb8ce583 | 依頼元・元のタスク・決めた日時・回答を出し、新しい順に並べた |
| W-32 | 修正 | bb8ce583 | 種類を和名にし、悪い知らせ danger・質問 warning とした。段の語をフィルタと揃えた |
| W-33 | 修正 | bb8ce583 | 送り手・元のタスク・案件・「受信箱で答える」を出す |
| W-34 | 一部 | bb8ce583 | 失敗箱を subject で区別できるようにした。1 つにまとめる案は残課題 |
| W-35 | 修正 | bb8ce583 | 対象を組織から選ぶ Select にし、規則文に例と注意を添えた |
| W-36 | 修正 | bb8ce583 | 開閉ボタンの名前を「「見出し」を展開」にし、`aria-controls` を付けた |
| W-37 | 修正 | 9a8527d3 | nav リンクを 44px 四方にした |
| W-38 | 修正 | 9a8527d3 | 保守の 3 節を ProjectSection に揃え、「適用」を primary にした |
| W-40 | 修正 | 9a8527d3 | 途中目標を「なし」「取得中…」「取得不可」で区別する |
| W-41 | 一部 | 9a8527d3 | 誤った説明文を削った。狭い幅で案件 select を畳む件は残課題 |
| W-42 | 修正 | 9a8527d3 | inline style・任意値・裸の border を token に置き換えた |
| W-43 | 一部 | 9a8527d3 | h1 直下の案件名を `text-title` にした（h1 の文字列は parity のため変えていない） |
| W-44 | 修正 | 9abbfd92 | 判断待ちを warning、失敗を danger にし、24 時間以内は「まもなく期限」と出す |
| V-02 | 修正 | bd393ea1 | verify の全画面 mobile-audit で見つかった。兄弟 task の task 詳細のスマホ化（c67913d8）で `/tasks/T1` の run リンクが `min-w-0`（幅 17px）になっていたので、`min-w-11` にした。`web/features/tasks/` は本 task の画面群の外だが、task の範囲 check（`web/`）の中で、受け入れ条件の mobile-audit を通すための最小の変更 |

## 残課題

指摘は、上の「修正」（「一部」を含む）かこの節のどちらか、または両方に入っている。
大半は parity e2e（`web/e2e/parity/`）の期待と衝突する。この task は parity の期待を書き換えないので、parity と画面を一緒に直す task に回す。

| ID | 重さ | 残る理由 | 次の手 |
|---|---|---|---|
| W-02 | 高 | 推奨でない非破壊の選択は即送信のまま。parity `inbox.spec.ts` が click の直後に API 応答を期待している | parity の更新と合わせて、確認か数秒の取り消しを入れる |
| W-03（V-01） | 高 | verify で見つかった。ホームの会話は、開いた直後に末尾まで scroll する（parity `console.spec.ts:321`「開いた直後は末尾にいる」）。判断待ちの区画を足したので頁が viewport より高くなり、360 では h1 と最も期限の近い判断待ち 1 件が初回表示で画面外に出る（実測 scrollY 253px。1440 では 121px で、3 件は見える）。fullPage 撮影では上部が空白帯に写る（`_-360`・`_-1440`・`stale-_-*`） | 会話を独立の scroll 領域にするか、初回の追従を会話が viewport を超えたときだけにする（`web/features/console/` と parity の協調変更） |
| W-04 | 高 | shell の最終受信時刻がスマホ幅で隠れる（`web/components/shell/`） | 共通部品の task |
| W-08 | 高 | `window.confirm` が残っている。parity が `page.once("dialog")` を期待している | parity の更新と合わせて ConfirmDialog にする |
| W-10 | 高 | 常設ルールの追加に確認ダイアログが無い。parity の /approvals 試験が「追加」1 click で POST を期待している | parity の更新と合わせて ConfirmDialog にする |
| W-13・W-16・W-17・W-24 | 中 | API が返す文（葉・planner）、parity が固定している accessible name（「タスク」「理由・note」「task の完了」「run の作業」「Console への入力」）、repo.id のラベル | API の文言と parity の協調変更 |
| W-19・W-20 | 中 | FetchFrame の戻り先と語（共通部品） | W-52 と一緒に直す |
| W-21・W-22・W-27・W-29 | 中 | 操作節の畳み込み、操作の出し分け、作成フォームの開閉、作業場所の削除と状態変更の確認。いずれも parity `projects.spec.ts` が開いた form と click 直後の送信を期待している | parity と一緒に直す |
| W-28・W-41・W-43 | 中〜低 | 詳細の判断待ちリンク（parity が `href="/inbox"` を期待）、board の案件 select（parity が使う）、h1 の文字列 | parity と一緒に直す |
| W-34 | 中 | 両方失敗したときに失敗箱を 1 つにまとめる機能が FetchFrame に無い | 共通部品の task |
| W-39 | 低 | checkbox を 20px にすると、mobile-audit が input 自体を 44px 未満と判定する | mobile-audit を label の当たり判定込みにしてから直す |
| W-45・W-46 | 低 | 通知の既読ボタンの位置、自動既読の失敗の黙殺 | 遷移をまたいで操作結果を持ち越す共通の仕組みと一緒に |
| W-47 | 低 | Console の入力欄の語と focus（`web/features/console/`、parity が名前を固定） | console の task |
| W-51・W-52 | 低 | h1 に当たる script focus の枠、失敗文の二重否定と語の不一致（`screen-frame.tsx`・`fetch-frame.tsx`） | 共通部品の task |

撮影の制約: fullPage が 800px で止まる問題は残る（/approvals のように shell の外へ伸びる頁だけ全高で写る）。error 状態の撮影は改善し、`error-_inbox-360` は loading と別の姿になった。偽 daemon に reports・approvals・文書の既定 fixture が無いため、それらは取得失敗の姿で撮れている。データ入りの姿は fix-reports の parity fixture screenshot と、非 parity e2e（`e2e/work/`）で確かめた。

## pre/post

- pre: `<task>/wu/critique/artifacts/qa-qa-work-pre/`（240 枚）。post: `<task>/wu/verify/artifacts/qa-qa-work-post/`（240 枚。build 後に `corepack pnpm@12.6.0 -C web screenshots --out <dir>` で 128 枚、`--states --out <dir>` で 116 枚）。
- 名前は pre と post で同じで、`comm -3` の差は 0 件。同じ名前の画像どうしが対応する。work 画面群（`_-*`・`_inbox*`・`_notifications*`・`_projects*`・`_board*`・`_reports*`・`_approvals*` と、各状態の work 画面）80 枚のうち、変化したのは 72 枚。同一の 8 枚は `empty-_inbox-*`・`empty-_notifications-*` で、0 件の表示は変えていない。

### 再評価（ui-ux-quality-gate、post を pre と並べて）

| 対応（pre → post） | 再評価 |
|---|---|
| `_inbox-360` | pre は推奨・期限・止めている範囲・案件・待ち・関連が並んだ後に、やっと問いが出ていた。post は見出し → 問い → 理由欄 → 推奨の選択肢の順で、最初の選択肢が fold の中にある。W-01 解消 |
| `many-_inbox-360` | 40 件の回答欄の常設が無くなった。各行は問い・「回答を開く」・推奨・期限・案件の順。W-14 解消 |
| `_-1440`・`_-360` | 期限の近い判断待ち 3 件と「まもなく期限」が会話より上に出る（W-03）。ただし初回は会話の末尾まで scroll するので、360 では最上位の判断待ちが隠れる（残課題 V-01） |
| `_projects_P1-1440` | 長い題名が折り返り、状態「実行待ち」と「判断待ち 1 件」の列が見える。案件名は `text-title` になった。W-05・W-43 |
| `_projects-360` | 横溢れが無く、途中目標と更新は題名の下の 1 行に出る。作成フォームは常に開いたまま（W-27 残課題） |
| `_board-1440` | 優先度・レベル・種類が日本語（通常・標準・機能）になり、各行に「案件: 画面品質の確認」が出る。W-23・W-25 |
| `_approvals-360` | 失敗箱が「決めた認可」「常設ルール」の subject で区別できる。一覧が読めない間は追加が止まり、Notice が理由を示す。W-11・W-34・W-50 |
| `_reports-*`・`_projects_P1_docs*` | 取得失敗の姿だが、何が取れなかったかを subject で示す。W-09・W-34 |

判定: generic AI dashboard 化はしていない（pre と同じ）。critique で挙げた 3 つの弱点のうち、「状態と次の操作が先に読めない」と「内部値の主表示」は、画面側で直せる範囲を直した。「確認の無い危険な操作」は、保守画面（W-07）と常設ルールの誤追加（W-11）を直した。残る確認（W-02・W-08・W-10・W-29）はどれも parity が click 直後の送信を固定していて、parity と一緒に直す必要がある。新しく見つけた V-01（ホームの初回 scroll）も、parity の console 試験との協調が要る。

### 検査（verify、HEAD `bd393ea1`）

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` exit 0、`build` exit 0
- `typecheck` exit 0、`lint` exit 0（既存の warning 5 件）、`test` exit 0（vitest 57 files / 347 tests、node 42 pass）、`check:boundaries`・`check:parity`・`check:secrets` exit 0
- `mobile-audit`（全画面）: 1 回目は exit 1（`/tasks/T1` の `a#- 17x44`、V-02）。`bd393ea1` の後は exit 0（`31 path(s) x 4 widths ok`）
- `e2e`（functional 全体と release）: exit 0、174 passed / 8 skipped
- 生の色・任意値の grep（FRONTEND_CONTRACT の式）: `web/components web/features web/routes web/lib` で 0 件（fixtures と test を除く）。基点 45a8fde5 からの追加行でも 0 件

## 提案

- `screenshots.mjs` に「`[data-fetch-state]` が loading でなくなるまで待つ（loading 状態は除く）」と、main の scroll 容器を含めた全高の撮影を足す。
- 偽 daemon の既定 fixture に報告 1 件・決めた認可 2 件（once/standing）・常設ルール 1 件・案件文書 2 件を足す。
