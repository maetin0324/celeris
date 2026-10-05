---
title: record-r5 look-c — after-r4 の状態変種 8 状態（29 組）の 1440・360 を目視判定
tasks: [01M45TA2085GTJVFNFR8CQ1G43]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# record-r5 look-c — after-r4 の状態変種 8 状態（29 組）の 1440・360 を目視判定

対象: after-r4 = `/local/celeris/data/workspaces/01M45PRPACBEP4X1FWPNP4FRMG/wu/fix-r5/artifacts/after-r4/`、before = `/local/celeris/data/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before/`。各 png を Read で開いて見た（file 名や寸法だけでは判定していない）。縦に長い `many-*` 4 枚は全体表示では文字が読めないため、上端 800px と下端 800px を切り出して読んだ（切り出しは WU の artifacts に置き、repo には入れない）。`stale-_-360` も下部 260px を 2 倍で確認した。

観点は ui-ux-quality-gate の 7 点（階層・密度・折返し・切れ・状態表示・タップ領域・a11y の見え方）。判定は『可』『要修正』『保留』。『可』には見えた根拠を添える。

## before の対応

before（120 枚）には状態変種が 1 枚も無い。各行の before 列は対応する基本画面の before を書く。`/notifications` は before に基本画面も無いので『before なし』。before の `/graph`・`/tasks`・`/tasks/T1`・`/tasks/T1/runs/R1` はそれ自体が取得失敗（「取得に失敗しました。」＋枠なしの「再試行」、run ログは「run ログを取得できませんでした。」のみ）で撮られており、error 変種との比較に使える。

## 見た枚数

| 区分 | 枚数 |
| --- | --- |
| 見た after-r4（29 組 × 1440・360） | **58** |
| 見た before（基本画面 7 種 × 1440・360） | 14 |
| この葉の担当範囲で見ていない after-r4（29 組 × 390・412） | **58** |

見ていない 58 枚: `empty-_graph`・`empty-_inbox`・`empty-_notifications`・`empty-_tasks`・`error-_inbox`・`error-_providers`・`error-_tasks`・`error-_tasks_T1`・`forbidden-_inbox`・`forbidden-_providers`・`forbidden-_tasks`・`forbidden-_tasks_T1`・`loading-_inbox`・`loading-_providers`・`loading-_tasks`・`loading-_tasks_T1`・`long-id-_inbox`・`long-id-_tasks`・`long-id-_tasks_01M44C5GXAC1A8D95VMMRYYG0F01M44DCZWBN71Z80577YM0HTPR`・`long-text-_inbox`・`long-text-_tasks`・`long-text-_tasks_T1`・`many-_inbox`・`many-_notifications`・`many-_tasks`・`stale-_`・`stale-_providers`・`stale-_tasks`・`stale-_tasks_T1_runs_R1` の各 `-390.png` と `-412.png`（29 × 2）。

## (c) の直否: error が保留のまま撮られていたか

**直っている。** `error-_inbox-1440.png`・`error-_inbox-360.png`・`error-_providers-1440.png`・`error-_providers-360.png`・`error-_tasks-1440.png`・`error-_tasks-360.png`・`error-_tasks_T1-1440.png`・`error-_tasks_T1-360.png` の 8 枚すべてに、左赤罫の帯で取得失敗の文（受信箱「判断待ちを取得できません。表示を更新できません。再取得してください。」、プロバイダ「プロバイダを取得できません。…」、タスク・タスクの詳細「取得に失敗しました。表示を更新できません。再取得してください。」）と枠付きの「再試行」button が写っている。loading の skeleton（灰色の空枠）が残っている error 変種は 1 枚も無い。`role=alert` の属性そのものは画像から読めないので、属性は `web/e2e/support/states.ts` の `waitForStateCapture`（`[data-fetch-state="error"][role="alert"]` 内の「再試行」が見えるまで待つ）と fix-r5.md の記録に拠る。

## 8 状態の状態表示（文言・再試行・空の案内・権限の説明・stale の印）が読めるか

| 状態 | 1 行まとめ |
| --- | --- |
| empty | 受信箱「いま決めることはありません。」、通知「通知はありません。」＋「未読はありません」、タスク「0 件」＋「条件に一致するタスクがありません」は 1440・360 とも読める。依存グラフだけは「0 ノード / 0 辺」の 1 行と空の灰色枠で、案内文が無い |
| error | 4 画面 × 2 幅すべてで赤帯の文と「再試行」が読める。360 では文が 2 行に折れ button が下段に落ちるが切れない |
| forbidden | 4 画面 × 2 幅で「…権限がありません。管理者に権限を確認してください。」と「一覧へ戻る」が読める。受信箱・プロバイダは対象名入り、タスク一覧・詳細は「この内容を」の汎用文 |
| loading | 4 画面 × 2 幅とも灰色の skeleton 枠だけで、文言は無い（受信箱 2 枠、他 1 枠）。読み上げ用の文があるかは画像から判定できない。側欄の件数は「?」になる |
| long-id | 一覧は題名と id が 1 行省略で収まり、詳細は見出しに 26 字 ×2 の id が 1440 では 1 行、360 では 3 行に折れて読める |
| long-text | 区切りの無い長い英数字が受信箱・詳細で単語内改行して収まり、一覧では題名 1 行省略。切れは無い |
| many | 受信箱 40 件・通知 50 件（＋「古い通知を見る」）・タスク 150 件が崩れずに並ぶ。件数は見出し・側欄 badge・「未読 60 件」で読める |
| stale | header の「接続状態: 再接続中」（黄）は 4 画面 × 2 幅で読める。ホームは加えて黄帯の説明文と「未確認」badge、プロバイダは「未確認」badge と「まだ接続を確認していません」。タスク一覧と run ログには header 以外の stale の印が無い |

## 判定表（58 枚）

| after-r4 | before | 判定 | 根拠 |
| --- | --- | --- | --- |
| `empty-_graph-1440.png` | `_graph-1440.png`（before は取得失敗表示） | 要修正（軽） | 「0 ノード / 0 辺」は読めるが、その下の灰色の空枠（高さ約 150px）に案内文も次の操作も無く、空なのか読み込み中なのか枠だけでは区別できない。入力 2 つと「絞り込み」の並び・タップ領域は問題なし |
| `empty-_graph-360.png` | `_graph-360.png`（同上） | 要修正（軽） | 同じ空枠の問題。入力は縦積みで切れず、「絞り込み」は 44px 級で押せる |
| `empty-_inbox-1440.png` | `_inbox-1440.png` | 可 | 「判断待ち（0）」の見出しの直下に「いま決めることはありません。」が読め、説明文・2 つの select・見出しの階層が揃う。before の 4 枠「ありません。」より 1 文で意図が伝わる |
| `empty-_inbox-360.png` | `_inbox-360.png` | 可 | select が全幅に縦積みし、空の案内が切れずに読める |
| `empty-_notifications-1440.png` | before なし | 可 | 「未読はありません」と「通知はありません。」が読め、右上の「受信箱を開く」「すべて既読にする」と下の「通知の届け方」まで一画面に収まる |
| `empty-_notifications-360.png` | before なし | 可 | 操作 2 つが説明文の下に落ち、select 3 つは 2 段に折れて切れない。「通知を試す」は独立行で押せる |
| `empty-_tasks-1440.png` | `_tasks-1440.png`（before は取得失敗表示） | 可 | 「0 件」と表の見出し行の下に中央寄せで「条件に一致するタスクがありません」が読め、絞り込みの chip は和名で 1 行 |
| `empty-_tasks-360.png` | `_tasks-360.png`（同上） | 可 | chip が 3 段に折れて切れず、空の案内が読める |
| `error-_inbox-1440.png` | `_inbox-1440.png` | 可 | 赤帯に文と「再試行」。帯は select の下に置かれ見出し階層を壊さない。(c) 直った |
| `error-_inbox-360.png` | `_inbox-360.png` | 可 | 文が 2 行、「再試行」が下段に落ちて帯内に収まる。(c) 直った |
| `error-_providers-1440.png` | `_providers-1440.png` | 可 | 見出しの直下に赤帯と「再試行」。(c) 直った |
| `error-_providers-360.png` | `_providers-360.png` | 可 | 文 2 行＋「再試行」、切れなし。(c) 直った |
| `error-_tasks-1440.png` | `_tasks-1440.png`（before は枠なしの「取得に失敗しました。」） | 可 | 絞り込みを残したまま赤帯と「再試行」。before より何が失敗したかと次の操作が明確。(c) 直った |
| `error-_tasks-360.png` | `_tasks-360.png`（同上） | 可 | chip 3 段の下に赤帯、文 2 行＋「再試行」。(c) 直った |
| `error-_tasks_T1-1440.png` | `_tasks_T1-1440.png`（同上） | 可 | tab 帯の下に赤帯と「再試行」、tab 5 つは 1 行で読める。(c) 直った |
| `error-_tasks_T1-360.png` | `_tasks_T1-360.png`（before は tab の文字が縦に割れる） | 保留 | 赤帯と「再試行」は読める（(c) 直った）。tab 帯の右端で「成果物」が「成果」で切れ、横 scroll の手掛かりが無い。before の縦割れよりは良いが、これは基本画面 `/tasks/T1` 360 と共通の課題なので判定は look-b に合わせる |
| `forbidden-_inbox-1440.png` | `_inbox-1440.png` | 可 | 「判断待ちを表示する権限がありません。管理者に権限を確認してください。」と「一覧へ戻る」が赤帯で読める |
| `forbidden-_inbox-360.png` | `_inbox-360.png` | 可 | 文 2 行、button 下段、切れなし |
| `forbidden-_providers-1440.png` | `_providers-1440.png` | 可 | 対象名入りの権限の説明と「一覧へ戻る」 |
| `forbidden-_providers-360.png` | `_providers-360.png` | 可 | 文が「ん。」で折り返すが読める。button は 44px 級 |
| `forbidden-_tasks-1440.png` | `_tasks-1440.png` | 要修正（軽） | 権限の説明は読めるが、一覧画面そのものに「一覧へ戻る」が出ており、戻り先が無い。文も「この内容を」の汎用文で、受信箱・プロバイダと違い対象名が無い |
| `forbidden-_tasks-360.png` | `_tasks-360.png` | 要修正（軽） | 同上（折返し・切れは無い） |
| `forbidden-_tasks_T1-1440.png` | `_tasks_T1-1440.png` | 可 | tab 帯の下に権限の説明と「一覧へ戻る」。詳細から一覧へ戻る導線として意味が通る |
| `forbidden-_tasks_T1-360.png` | `_tasks_T1-360.png` | 保留 | 帯は読める。tab 帯の「成果物」切れは `error-_tasks_T1-360.png` と同じで look-b に合わせる |
| `loading-_inbox-1440.png` | `_inbox-1440.png` | 可 | select の下に角丸の skeleton 2 枠、見出し・説明は出たまま。側欄の件数が「?」で未取得と分かる |
| `loading-_inbox-360.png` | `_inbox-360.png` | 可 | skeleton 2 枠が全幅、切れなし |
| `loading-_providers-1440.png` | `_providers-1440.png` | 可 | 見出し＋skeleton 1 枠。header が「接続状態: 接続を確認中」 |
| `loading-_providers-360.png` | `_providers-360.png` | 要修正（軽） | 「接続状態: 接続を確認中」が「接続済み」より長く、header が 2 段（1 段目 Celeris＋接続状態、2 段目にメニュー button）に折れて高さが倍になる。他の 360 の shot では 1 段なので、接続文言で layout が跳ねる |
| `loading-_tasks-1440.png` | `_tasks-1440.png` | 可 | 絞り込みを残して skeleton 1 枠 |
| `loading-_tasks-360.png` | `_tasks-360.png` | 可 | chip 3 段の下に skeleton 1 枠、切れなし |
| `loading-_tasks_T1-1440.png` | `_tasks_T1-1440.png` | 可 | tab 帯の下に skeleton 1 枠 |
| `loading-_tasks_T1-360.png` | `_tasks_T1-360.png` | 保留 | skeleton は問題なし。tab 帯の「成果物」切れは上と同じ |
| `long-id-_inbox-1440.png` | `_inbox-1440.png` | 可 | 判断待ち 5 件の card（決定・計画の承認・失敗・認可・知識の候補）が 1 列で読め、「先に答える項目: decision…」は省略記号で切れない。推奨 button は塗り、危険操作は赤で階層がある |
| `long-id-_inbox-360.png` | `_inbox-360.png` | 可 | button が縦積み、期限・推奨の行が折れて読める |
| `long-id-_tasks-1440.png` | `_tasks-1440.png` | 可 | 先頭行の長い題名と 52 字の id がそれぞれ 1 行省略で列幅に収まり、他の列（状態 badge・担当・run・更新）は揃う |
| `long-id-_tasks-360.png` | `_tasks-360.png` | 可 | 題名・id とも 1 行省略、状態 badge と担当・run・更新を 2 行に積む |
| `long-id-_tasks_01M44C5GXAC1A8D95VMMRYYG0F01M44DCZWBN71Z80577YM0HTPR-1440.png` | `_tasks_T1-1440.png` | 保留 | 見出し「タスクの詳細 01M44…HTPR」は 1 行で読め、題名 4 回繰り返しも 2 行で折れる。ただし「実行と routing」の中に「取得に失敗しました。…再試行」の赤帯が 2 本出ている。これは既定 fixture にこの id の execution・routing が無く 404 になるためで、UI の折返しとは別の fixture 側の欠け。帯の見た目自体は error 変種と同じで読める |
| `long-id-_tasks_01M44C5GXAC1A8D95VMMRYYG0F01M44DCZWBN71Z80577YM0HTPR-360.png` | `_tasks_T1-360.png` | 可 | id が 3 行に単語内改行して読め、題名は 3 行で省略。節の chip（概要・判断・実行・木）が横に並び、「依存元」「run」まで切れない。tab 帯の「成果物」切れは共通課題として別記 |
| `long-text-_inbox-1440.png` | `_inbox-1440.png` | 可 | 区切りの無い英数字 `averyvery…_0123456789` が題名・本文・「先に答える項目」の link で単語内改行し、横溢れが無い |
| `long-text-_inbox-360.png` | `_inbox-360.png` | 可 | 同じ長語が 360 でも折れて切れない |
| `long-text-_tasks-1440.png` | `_tasks-1440.png` | 可 | 題名 1 行省略、id T1、列は揃う |
| `long-text-_tasks-360.png` | `_tasks-360.png` | 可 | 題名 1 行省略、2 行積み |
| `long-text-_tasks_T1-1440.png` | `_tasks_T1-1440.png` | 可 | 見出し card の題名が 3 行で折れ、目的の長文も単語内改行。木の節の長い題名は 2 行で省略。実行と routing は値が出ている |
| `long-text-_tasks_T1-360.png` | `_tasks_T1-360.png` | 保留 | 題名 3 行 clamp、目的の長語は折れて読める。tab 帯の「成果物」切れは共通課題 |
| `many-_inbox-1440.png` | `_inbox-1440.png` | 可 | 40 件が「決定／多数の判断 n／1 文／回答を開く／推奨・期限・期限切れ／案件」の同じ骨格で並び、各 card は折り畳みで密度が保たれる。側欄の badge 40・60 と見出し（40）が一致 |
| `many-_inbox-360.png` | `_inbox-360.png` | 可 | 上端・下端とも card の骨格が崩れず、「期限切れ」badge は期限の行の右で切れない |
| `many-_notifications-1440.png` | before なし | 可 | 50 件が「未読／task の完了／3 件の束／題名／要約／タスク link・案件」で並び、末尾に「古い通知を見る」と「通知の届け方」。右端「既読にする」は列として揃う |
| `many-_notifications-360.png` | before なし | 可 | 「既読にする」が各 card の下段に落ち 44px 級、末尾の「古い通知を見る」「通知を試す」も読める |
| `many-_tasks-1440.png` | `_tasks-1440.png` | 可 | 「150 件」と 7 行強が枠内 scroll で見え、1 行 52px で密度が揃う。枠の下端で 8 行目の id が半分切れるのは scroll 枠の端で、列は切れない |
| `many-_tasks-360.png` | `_tasks-360.png` | 可 | 3 件強が枠内に見え、題名・id・状態・担当・更新が 2 行積みで読める |
| `stale-_-1440.png` | `_-1440.png` | 可 | header 黄「接続状態: 再接続中」、黄帯「接続を確認しています。判断待ちと会話の最新の状態は未確認です。…」、判断待ち card の「未確認」badge の 3 段で stale が読める。会話枠の「宛先: CoS」「新しい会話」行は枠の上端に留まり、(d) の浮いた枠は無い |
| `stale-_-360.png` | `_-360.png` | 要修正（軽） | 黄帯と「未確認」は読める。しかし黄帯と判断待ち card が縦を使い切り、「宛先: CoS／新しい会話」の下の会話枠が高さ約 40px しか残らず、末尾追従で「画面を検証する（tool 2 回）」の 1 行が上端で半分に切れて見える。(d) の 1440 の浮き枠とは別だが、同じ会話枠の高さ配分の問題 |
| `stale-_providers-1440.png` | `_providers-1440.png` | 可 | header 黄 badge に加え、表の状態「未確認」、card 見出しの「未確認」badge、「まだ接続を確認していません」で stale が data の近くで読める |
| `stale-_providers-360.png` | `_providers-360.png` | 可 | 表は「実行枠／状態／道具／受ける段」の 4 列に減らして切れず、「未確認」badge が読める。tier の checkbox と label は 44px 級 |
| `stale-_tasks-1440.png` | `_tasks-1440.png` | 保留 | header の黄 badge は読めるが、一覧側には「未確認」等の印が無く、通常の `/tasks` と見分けが付かない。ホーム・プロバイダが data の近くに印を出すのと一貫していない。意図した設計かは画像からは決められない |
| `stale-_tasks-360.png` | `_tasks-360.png` | 保留 | 同上（header badge は 360 でも 1 段に収まる） |
| `stale-_tasks_T1_runs_R1-1440.png` | `_tasks_T1_runs_R1-1440.png`（before は「run ログを取得できませんでした。」のみ） | 保留 | 状態 card「終了（結果の記録なし）」、「27 行」、末尾 10 行のログが読め、長い行は折り返される。stale の印は header だけで、ログが古い可能性を本文で示していない |
| `stale-_tasks_T1_runs_R1-360.png` | `_tasks_T1_runs_R1-360.png`（同上） | 保留 | 状態 card が 2 列に折れ、「長い行を折り返す」「原文で見る」は 44px 級で並ぶ。stale の印は上と同じ理由で保留 |

集計: 可 40・要修正（軽）6・保留 12。要修正はいずれも軽微で、(a)〜(d) の再発では無い。

## 残った課題（この葉の範囲）

1. `empty-_graph-{1440,360}.png`: 空の依存グラフの枠に案内文（例: 起点の task id を入れて絞り込むと木が出る）が無い。
2. `forbidden-_tasks-{1440,360}.png`: 一覧画面の forbidden に「一覧へ戻る」が出る。戻り先はホームか、button を出さないかのどちらかに揃える。文も対象名入りにする。
3. `loading-_providers-360.png`: header の接続状態の文言長で 2 段に折れる。接続状態の pill に最大幅と省略、または 360 では文字を落として色だけにする。
4. `stale-_-360.png`: 黄帯が出ると会話枠が 40px 程度に潰れ、切れた 1 行だけが見える。帯が出るときは会話枠に最小高さを与えるか、判断待ち card を畳む。
5. 保留 5 枚（`*-_tasks_T1-360.png` と `long-id-_tasks_<長い id>-360.png`）の tab 帯の右端切れは基本画面 `/tasks/T1` 360 の課題として look-b の判定に合わせる。
6. 保留 4 枚（`stale-_tasks-*`・`stale-_tasks_T1_runs_R1-*`）: 一覧・ログに stale の印を出すかは設計判断。ホーム・プロバイダと揃えるなら「未確認」badge を件数見出しの横に出す。
7. 保留 1 枚（`long-id-_tasks_<長い id>-1440.png`）: 既定 fixture に長い id の execution・routing が無い（fake-daemon.mjs）。撮影上の欠けで、UI の修正ではない。

## 提案

- 状態変種の撮影に `/graph` の error と `/notifications` の error・forbidden・loading を足す（今は empty・many のみ）。
- `states.ts` の long-id は `/tasks/<id>/execution`・`/routing` の fixture も長い id で返す。

## 検査

| コマンド | 結果 |
| --- | --- |
| `sh scripts/dev/check-doc-links.sh` | exit 0（`check-doc-links: ok`） |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0（`check-adr-numbers: ok (138 files)`） |
| `sh scripts/dev/progress-index.sh --check` | exit 0（`progress-index --check: ok`） |
| 範囲 check（`git log --format= --name-only $CELERIS_WU_BASE..HEAD --not main` が `agent-docs/progress/` だけ、web/・crates/ の差分なし） | exit 0（変更は本 file 1 つ） |
