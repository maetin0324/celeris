---
title: 戦略層 v1 子 1 prompt-audit — 修正を含む release の昇格後の本番 run の実 prompt に、目的（祖先を含む）と受入条件の全文が入ったか（本番 DB は mode=ro で読むだけ）
tasks: [01M7XQ9Q4P4ZQ38Y509J9XYAQ0]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# prompt-audit（WorkUnit。子 1 の受入条件 2 の再監査）

完了日: 2026-11-21。コードは変えていない（変えたのはこの md と `prompt-audit/` の script・tsv・meta.json だけ）。

## 結論

- planner の受入条件 0/126 は再発していない。修正後の planner の 3 run で、prompt に task の受入条件が 12/12 件入った。3 run とも見出し `## Task acceptance criteria (checked by the final review)` が 1 回ある。
- run 自身の objective（WU の run は WU の objective）の末尾 200 字と受入条件（WU の run は done_when）: prompt のある 16 run の全部で入った（cut 0）。H の監査と同じ結果。
- 祖先（親・root）の受入条件は、子 task の 7 run（prompt のある run）のどれにも入らない（0/26 件）。祖先の objective は worker・reviewer では 4/4 run で末尾が入ったが、builder が祖先の objective を出しているのではない。CoS が親の objective の文を子の objective に写したので、子の objective の一部として入っただけである。planner では祖先の objective の末尾が 5 件中 4 件で欠けた（先頭 300 字だけが入った 2 件と、祖先として出なかった 2 件）。残りの 1 件（子 1 の planner）も CoS の写しで入った。
- Project.request の末尾 200 字は、prompt のある run の全部（15 run。project の無い 1 run を除く）で入っていない。Project.request を prompt に出す builder は無い。
- 結論として、人の元の目的が CoS の写しに依らず届く経路は無い。経路と file:line は下の「欠けの一覧」に書いた。prompt-fix の WorkUnit に渡す。
- prompt.txt の無い run は 1 件（reviewer 01M7X7A83N1ACFKK3W2Z5TR4NP）。この run は起動前に review が終わって中止された（`interrupted: reviewer run not started (the review ended before launching it)`、usage null）。LLM は呼ばれていない。

## 基点（修正を最初に含んだ release の昇格時刻）

- 修正は ADR 2026-11-19-planner-reviewer-prompt-coverage の D1。commit は件名 `arch-v4 H R1 planner-prompt: planner の 4 builder の prompt に task の受入条件の節を出し、planner_prompt_acceptance_* と prompt_full_objective_planner の試験を足す` の 1 件（HEAD の祖先）。
- `/local/celeris/state/releases/*/promoted.json` の 4 件（operation promote・status succeeded）に `git merge-base --is-ancestor <修正の commit> <sha12>` を当てた。
  - 修正を含まない: `7014d24222c4`（昇格 2026-11-19T05:35:55Z）、`1680de5cd1b4`（昇格 2026-11-19T11:30:18Z）。
  - 修正を含む: `54e61984357f`（昇格 2026-11-20T15:44:15Z、promoted.json の from は `1680de5cd1b4`）、`7f69057907ad`（昇格 2026-11-20T17:09:27Z）。
  - 2 件とも release の `crates/task-worker/src/claude_code/prompt.rs` に上の見出しがある（`meta.json` の `has_heading`）。
- `54e61984357f` の from が修正を含まない `1680de5cd1b4` なので、`54e61984357f` が修正を最初に含んだ昇格である。基点は 2026-11-20T15:44:15Z。

## 対象と選び方

- 対象: `runs.role` が worker・planner・reviewer で、`started_at` が基点より後の run（WU の run を含む）。`started_at` の順に並べ、上限 300 件。DB を読んだ時刻（2026-11-21T01:01:09Z）で、条件に合う run は 17 件。上限で落とした run は無い。走っている run（この run を含む）も数えた。
- 依頼 celeris-strategy-dialogue-observability-v1-20261120 の run:
  - 親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE: run は 0 件（子を待つ親で、run が無い）。親の objective と受入条件は、子の run の「祖先」の行で比べた。
  - 子 0 01M7XQ9PZQ3DQZMYKNFNPTPBQJ: worker 1 件、reviewer 2 件（1 件は harness_error）。
  - 子 1 01M7XQ9Q4P4ZQ38Y509J9XYAQ0（この task）: planner 1 件、WU prompt-audit の worker 1 件（この run）。
  - 子 1 の WU から作られた子 task 2 件（01M7XSH46KVRV62ENAWWKWJKBQ userns-skip、01M7XSGW4FH258N1K3AMW76Y6F decision-failclosed）: planner 各 1 件（読んだ時刻に running）。
- 残りは、アーキv4 親 01M7MZT8KX547464YYS76QNP4G の closeout（WU の worker 6 件と reviewer 1 件）と、日次の 2 task（知識候補抽出の worker 1 件、日次整理の worker 1 件・reviewer 1 件）。

## 数え方

H の prompt-audit（`agent-docs/progress/2026-11-19-arch-v4-h/prompt-audit.md`）と同じ規則で数えた。

- 両側の空白の連なりを 1 つの空白にする。objective は、末尾 200 字が prompt に含まれれば found、含まれなければ cut、prompt.txt が無ければ missing。受入条件は text ごとに同じ判定をする。
- WU の run の objective と done_when は、run の開始時に有効だった計画の版（`execution_plans.created_at <= runs.started_at` の最大の版）の `units[]` から取る。
- prompt は `<task の workspace>/runs/<run id>/prompt.txt`（workspace は tasks.json の `workspace.path`）を読むだけ。
- tsv の 1 行は「run × 比べる相手」。`kind` は次のとおり。
  - `objective`・`acceptance`: run 自身の objective と受入条件（WU の run は WU の objective と done_when。reviewer は review する task）。
  - `task_objective`・`task_acceptance`: WU の run だけ。task の objective と受入条件（「このタスク全体の目的」と「最終レビューで確かめる Task の受け入れ条件（参考）」の節に出るはずの文）。
  - `ancestor_objective`・`ancestor_acceptance`: `parent_id` を root まで辿った祖先ごと（`up` は 1 が親、2 が祖父）。
  - `project_request`: 案件の Project.request の末尾 200 字。
- `head_only` は、cut の項目のうち先頭 80 字だけが prompt にあるものの数（途中で切れたのか、まったく無いのかを分ける）。
- tsv には id・role・数・判定だけを書き、text は書かない（秘密・人の名前を tsv に入れない）。

## 集計

<!-- prompt-audit:summary:begin (prompt_audit.py render が書く。手で直さない) -->

- DB を読んだ時刻: 2026-11-21T01:01:09Z。基点: release `54e61984357f` の昇格 2026-11-20T15:44:15Z（その前の release `1680de5cd1b4` は修正を含まない）。
- 対象の run: 17 件（条件に合う run 17 件、上限 300、上限で落とした run 0 件）。tsv の行: 82。
- role ごとの run の数（prompt のある run / ない run）: planner 3（3 / 0）、reviewer 4（3 / 1）、worker 10（10 / 0）。

| role | kind | 行 | 項目 | found | cut | missing | 先頭だけ | 判定 found / cut / missing / none |
|---|---|---:|---:|---:|---:|---:|---:|---|
| worker | objective | 10 | 10 | 10 | 0 | 0 | 0 | 10 / 0 / 0 / 0 |
| worker | acceptance | 10 | 34 | 34 | 0 | 0 | 0 | 9 / 0 / 0 / 1 |
| worker | task_objective | 7 | 7 | 7 | 0 | 0 | 0 | 7 / 0 / 0 / 0 |
| worker | task_acceptance | 7 | 24 | 24 | 0 | 0 | 0 | 7 / 0 / 0 / 0 |
| worker | ancestor_objective | 2 | 2 | 2 | 0 | 0 | 0 | 2 / 0 / 0 / 0 |
| worker | ancestor_acceptance | 2 | 4 | 0 | 4 | 0 | 0 | 0 / 2 / 0 / 0 |
| worker | project_request | 9 | 9 | 0 | 9 | 0 | 0 | 0 / 9 / 0 / 0 |
| planner | objective | 3 | 3 | 3 | 0 | 0 | 0 | 3 / 0 / 0 / 0 |
| planner | acceptance | 3 | 12 | 12 | 0 | 0 | 0 | 3 / 0 / 0 / 0 |
| planner | ancestor_objective | 5 | 5 | 1 | 4 | 0 | 2 | 1 / 4 / 0 / 0 |
| planner | ancestor_acceptance | 5 | 18 | 0 | 18 | 0 | 0 | 0 / 5 / 0 / 0 |
| planner | project_request | 3 | 3 | 0 | 3 | 0 | 0 | 0 / 3 / 0 / 0 |
| reviewer | objective | 4 | 4 | 3 | 0 | 1 | 0 | 3 / 0 / 1 / 0 |
| reviewer | acceptance | 4 | 21 | 17 | 0 | 4 | 0 | 3 / 0 / 1 / 0 |
| reviewer | ancestor_objective | 2 | 2 | 2 | 0 | 0 | 0 | 2 / 0 / 0 / 0 |
| reviewer | ancestor_acceptance | 2 | 4 | 0 | 4 | 0 | 0 | 0 / 2 / 0 / 0 |
| reviewer | project_request | 4 | 4 | 0 | 3 | 1 | 0 | 0 / 3 / 1 / 0 |

### run ごとの判定

| run | task | role | WU | 状態 | prompt | objective | acceptance | task_objective | task_acceptance | 祖先 objective | 祖先 acceptance | project_request |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 01M7WTCQ2N1FP7KM5HWHYRC2YH | 01M7MZT8KX547464YYS76QNP4G | worker | deps | completed | present | 1/1 | 3/3 | 1/1 | 3/3 | - | - | 0/1 |
| 01M7WTDR7XYHTPD56AN7PX9NKX | 01M7MZT8KX547464YYS76QNP4G | worker | gate | failed | present | 1/1 | 3/3 | 1/1 | 3/3 | - | - | 0/1 |
| 01M7WTE2XHNN5BNH3Z4QG82D23 | 01M7MZT8KX547464YYS76QNP4G | worker | loc | completed | present | 1/1 | 3/3 | 1/1 | 3/3 | - | - | 0/1 |
| 01M7WTYFMSR1NVJFA278TRJJQB | 01M7MZT8KX547464YYS76QNP4G | worker | store | completed | present | 1/1 | 3/3 | 1/1 | 3/3 | - | - | 0/1 |
| 01M7WVG1S8K6RZVQC6GRZAXZQV | 01M7MZT8KX547464YYS76QNP4G | worker | gate | completed | present | 1/1 | 3/3 | 1/1 | 3/3 | - | - | 0/1 |
| 01M7WWGTH8NDFWSBBJBE1R3QHR | 01M7MZT8KX547464YYS76QNP4G | worker | close | completed | present | 1/1 | 4/4 | 1/1 | 3/3 | - | - | 0/1 |
| 01M7WX28NMF6C5FVFPPC74YK5G | 01M7MZT8KX547464YYS76QNP4G | reviewer | - | completed | present | 1/1 | 3/3 | - | - | - | - | 0/1 |
| 01M7X6ZZT682C9NBZX1A5SQWTH | 01M7X6ZYMEA4SRFZQ9EDGJZA3T | worker | - | completed | present | 1/1 | 0/0 | - | - | - | - | - |
| 01M7X795BJJYNW0ZEFQW7PM2DT | 01M7X6ZVVC1ETP9AJZPRC6WD8P | worker | - | completed | present | 1/1 | 4/4 | - | - | - | - | 0/1 |
| 01M7X7A83N1ACFKK3W2Z5TR4NP | 01M7X6ZVVC1ETP9AJZPRC6WD8P | reviewer | - | cancelled | missing | 0/1 | 0/4 | - | - | - | - | 0/1 |
| 01M7XQA949D4JGG2RXHAP1SSW2 | 01M7XQ9PZQ3DQZMYKNFNPTPBQJ | worker | - | completed | present | 1/1 | 7/7 | - | - | 1/1(up1) | 0/2(up1) | 0/1 |
| 01M7XRHHFBRQ55AT7DVACE6ME9 | 01M7XQ9PZQ3DQZMYKNFNPTPBQJ | reviewer | - | harness_error | present | 1/1 | 7/7 | - | - | 1/1(up1) | 0/2(up1) | 0/1 |
| 01M7XRKHCRCQMNF3RZJ4CF2GHN | 01M7XQ9PZQ3DQZMYKNFNPTPBQJ | reviewer | - | completed | present | 1/1 | 7/7 | - | - | 1/1(up1) | 0/2(up1) | 0/1 |
| 01M7XRYPGB64N1KC7F0R8NY6D2 | 01M7XQ9Q4P4ZQ38Y509J9XYAQ0 | planner | - | completed | present | 1/1 | 6/6 | - | - | 1/1(up1) | 0/2(up1) | 0/1 |
| 01M7XSHPDX013XB9PS0P2GZA9V | 01M7XQ9Q4P4ZQ38Y509J9XYAQ0 | worker | prompt-audit | running | present | 1/1 | 4/4 | 1/1 | 6/6 | 1/1(up1) | 0/2(up1) | 0/1 |
| 01M7XSHSAYVFP46A6V6Q5VS9B2 | 01M7XSH46KVRV62ENAWWKWJKBQ | planner | - | running | present | 1/1 | 3/3 | - | - | 0/1(up1) 0/1(up2) | 0/6(up1) 0/2(up2) | 0/1 |
| 01M7XSHSC732HJ5RGQPFHDBBD4 | 01M7XSGW4FH258N1K3AMW76Y6F | planner | - | running | present | 1/1 | 3/3 | - | - | 0/1(up1) 0/1(up2) | 0/6(up1) 0/2(up2) | 0/1 |

### planner の受入条件

- planner の run 3 件（01M7XRYPGB64N1KC7F0R8NY6D2, 01M7XSHSAYVFP46A6V6Q5VS9B2, 01M7XSHSC732HJ5RGQPFHDBBD4）。prompt のある run の受入条件: 12/12。

<!-- prompt-audit:summary:end -->

## planner の受入条件（0/126 の再発の有無）

- 対象の planner の run: 01M7XRYPGB64N1KC7F0R8NY6D2（子 1、6 件）、01M7XSHSAYVFP46A6V6Q5VS9B2（userns-skip の子 task、3 件）、01M7XSHSC732HJ5RGQPFHDBBD4（decision-failclosed の子 task、3 件）。計 3 run。
- 受入条件は 12/12 件が入った。H の監査の 0/126（修正前の 24 run）は再発していない。
- 3 run とも prompt に見出し `## Task acceptance criteria (checked by the final review)` が 1 回ある（ADR 2026-11-19-planner-reviewer-prompt-coverage D4 の回帰条件）。
- ただし、planner の prompt に入るのはその task 自身の受入条件だけで、祖先の受入条件は入らない（下の欠け 1）。

## 欠けの一覧（prompt-fix に渡す）

行番号はこの WorkUnit の base の tree のもの。

1. 祖先の受入条件が、どの role の prompt にも入らない（worker 0/4・reviewer 0/4・planner 0/18 件）。
   - worker・reviewer: `prompt_header`（`crates/task-worker/src/claude_code/prompt.rs:152`）は task の objective（:213）か、WU の objective と task の objective（:178〜:181）だけを出す。受入条件の節は `build_execute_prompt`（:634）と `build_review_prompt`（:1702）が `acceptance_lines`（:744）で出すが、対象は `task.acceptance` だけである。`RunContext`（`crates/task-worker/src/protocol.rs:436`）に祖先の受入条件を運ぶ欄が無い。
   - planner: `build_execution_plan_prompt`（`prompt.rs:964`）の祖先の節（:1353〜:1367）は `TreeAncestorContext`（`protocol.rs:850`）の title・stage・`objective_excerpt` だけを出す。受入条件の欄は無い。
2. planner の prompt で、祖先の objective が先頭 300 字で切れる（2 件。`head_only` 2）。
   - `tree_planner_context`（`crates/task-dispatch/src/dispatcher/planner_flow.rs:644`）が `objective_excerpt: t.objective.chars().take(300)`（:678）で先頭 300 字だけを渡す。`protocol.rs:856` の doc comment も「目的の先頭 300 文字」。
3. planner の prompt で、tree の外の祖先（CoS が `parent_id` で作った親）が祖先として出ない（2 件。userns-skip と decision-failclosed の子 task から見た親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE。子 1 の planner 01M7XRYPGB64N1KC7F0R8NY6D2 にも祖先の節が無い）。
   - `ancestors_with_self`（`crates/task-ops/src/tree.rs:106`）は `tree.parent_unit`（:110〜:114）だけを辿る。子 1 は tree の root なので、その上の `parent_id` の親で止まる。
4. worker・reviewer の prompt に、祖先の objective を出す節が無い（found の 4 件は CoS の写しによる）。
   - 子 0 と子 1 の objective は、親の objective の末尾 200 字を文のまま含む（`norm(親の objective)[-200:] in norm(子の objective)` が両方で真）。そのため found に数えられたが、経路は欠け 1 と同じで、`prompt_header`（`prompt.rs:152`）は祖先の objective を出さない。子 1 の planner 01M7XRYPGB64N1KC7F0R8NY6D2 の found の 1 件も同じで、prompt に祖先の節は無い（欠け 3）。CoS が写さない子では届かない。
5. Project.request の末尾が、どの prompt にも入らない（prompt のある 15 run で 0/15）。
   - `prompt_header`（`prompt.rs:152`）にも、どの builder にも Project.request を出す節が無い。`RunContext`（`protocol.rs:436`）に案件の request の欄が無い（`active_projects` は CoS の対話の run だけ）。
   - この案件（01M2WTS3DKNZBSZ2JMVB4CZMBW）の Project.request は 345 字の案件の説明で、依頼の原文ではない。依頼の原文は CoS が子の objective に写した文として届いている。

## prompt の無い run

| run | task | role | 開始（UTC） | 記録された終わり方 |
|---|---|---|---|---|
| 01M7X7A83N1ACFKK3W2Z5TR4NP | 01M7X6ZVVC1ETP9AJZPRC6WD8P（日次整理） | reviewer | 2026-11-20T19:35:42 | `interrupted: reviewer run not started (the review ended before launching it)`、usage null、`runs/<run id>/` の dir が無い |

この run には prompt が作られず、LLM も呼ばれていない。中身の欠けではないので、欠けの一覧には入れない。

## 再現

DB は `file:<path>?mode=ro` の URI でだけ開く。prompt.txt と `/local/celeris/state/releases/*/promoted.json` は読むだけ。

```sh
# repo の root で。tsv と meta.json を作り直す（DB を読んだ時刻は meta.json の read_at に入る）
python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/prompt-audit/prompt_audit.py \
  collect --db /local/celeris/data/db/celeris.sqlite3 --out agent-docs/progress/2026-11-21-strategy-v1-1/prompt-audit
# md の集計の節を tsv から組み直す
python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/prompt-audit/prompt_audit.py render --write
# commit した tsv・meta.json と md の集計が一致するかを確かめる（欠けでは落ちない）
python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/prompt-audit/prompt_audit.py check
```

- 読んだ時刻: 2026-11-21T01:01:09Z（`prompt-audit/meta.json` の `read_at`）。collect は 4.5 秒で終わった（exit 0、17 run・82 行）。
- 後で collect をやり直すと、基点より後の run が増えるので tsv は変わる。そのときは `render --write` で md の集計の節を組み直す。

## 検査（提出時の HEAD で実行）

基準は `$CELERIS_WU_BASE`（この WorkUnit の base）。commit は件名 `strategy-v1 1 prompt-audit: 修正後の本番 run の実 prompt を再監査する` の 1 件。

- WU の check 1 `python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/prompt-audit/prompt_audit.py check`: exit 0（`ok: 17 runs, 82 rows, 31 rows with gaps (cut/missing); md matches tsv`）。
- WU の check 2（範囲。変えた path が `agent-docs/progress/2026-11-21-strategy-v1-1/` の下だけ）: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0（`progress-index --check: ok`）。
- check の負の確かめ: md の集計の節の 1 つの数（planner の acceptance の found 12）を 11 に書き換えると check は exit 1（`md summary block differs …`）になり、元に戻すと exit 0 に戻った。
- `bash scripts/dev/test-affected.sh`・clippy・fmt は走らせていない（変えたのは agent-docs/progress の md・tsv・json・py だけで、Rust と web の変更は無い）。

## 未確認の事項

- 基点より前に始まり、基点の後も走っていた run は数えていない（どの release の worker が prompt を組んだかを run ごとに記録した欄が無いため、開始時刻で分けた）。
- release の dir は剪定される（cur・prev・新しい 3 件）。`7014d24222c4` より前の release は dir が無いが、修正の commit（2026-11-19T15:39Z）より前の昇格なので基点には影響しない。
- 読んだ時刻の後に始まった run（例: WU root-artifacts・prompt-fix の run）は数えていない。

## 提案（起票は人が決める）

1. prompt-fix の WorkUnit で、欠け 1〜5 の経路を直すかを決める。守る不変条件の候補は「子 task の worker・planner・reviewer の prompt に、`parent_id` を root まで辿った祖先の objective と受入条件の全文、と Project.request が入る」。今の試験（`prompt_full_objective_*`・`planner_prompt_acceptance_*`）は task 自身の objective と受入条件しか見ないので、この欠けを検出できない。
2. 回帰条件: 修正を含む release の昇格後に、この script の collect と render を走らせ、`ancestor_objective`・`ancestor_acceptance`・`project_request` の cut が 0 になることを確かめる。
