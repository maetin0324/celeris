# 行数の削減の目標を planner・worker・reviewer に届け、純減でない変更を合格にしない

- 日付: 2026-11-16
- 状態: Accepted（実装済み）
- 関連: [ADR-0072](0072-task-execution-decomposition.md) D9（WU の prompt の task 全体の目的）、[ADR 2026-11-07-health-review-integration](2026-11-07-health-review-integration.md) D1（汎用規約の配送）、[ADR-0007](0007-phase5-planner-and-reviewer.md) D5（決定的な check が落ちたら reviewer を起動しない）
- task: 01M7J3NKV5R4XFVSDEWDQ4MF22（LoC 削減 v3 の子 B。親 01M7J3N68JRNTP5Q4G934QWF36）

## 背景

健全性v2 の子 12 件は done になったが、本体（production）の行数は 366,177 から 386,112 に増えた。削減の目標が worker と reviewer に届いていたか、届いたとして合否に使われたかを、配送経路で確かめた。

- 汎用規約（`crates/task-worker/src/change_rules/generic.md`）は全 run kind の prompt に 1 回入る。しかし行数の純減には触れていなかった。
- 計画のある task の WorkUnit の run は、task 全体の目的を先頭 1,500 文字しか受けていなかった（`work_unit_prompt_context`。ADR-0072 D9）。削減の依頼の目的は長く、削除の候補・基点の commit・禁止事項・不変条件は 1,500 文字より後ろにあるので、WU の worker に届かない。
- final review の reviewer は task の目的の全文・受け入れ条件・celeris が実行した決定的な check の結果・health の材料（target_sha・reviewed_sha・diff の統計）を受ける。受け入れ条件の command check が 1 つでも落ちると、reviewer は起動されず review は不合格になる（`review_decision::REVIEWER_NOT_EVALUATED_REASON`）。

## 決定

D1. WU の run には task 全体の目的の全文を渡す。`WorkUnitPromptContext.task_objective_excerpt`（先頭 1,500 文字）を `task_objective`（全文）に置き換える。prompt の見出しは「このタスク全体の目的（全文。目標・禁止事項・不変条件はこの WorkUnit にも掛かる）」にする。ADR-0072 D9 の「先頭 1,500 文字だけ載せる」はこの ADR で廃止する。

D2. 汎用規約に規約 11 を足す。行数の純減は task の目的か受け入れ条件が求めるときだけの目標にし、求める task では、計画は候補・基点・目標・禁止事項・不変条件と測定 command を子 task と WorkUnit に写し、実装は消した対象と before/after の行数を報告し、review は本体の純増・不変・移動だけの削減と測定の欠損を合格にしない。

D3. 純減の合否の決定的な判定は、受け入れ条件の command check（size check）が持つ。測定は `scripts/dev/source-size-report.py --exclusive-classes` を base の commit の script と toml で base と HEAD に当てる（分類規則を base に固定）。repo 固有の書き方は `CLAUDE.md` の「作業の進め方」に 1 行で書き、generic.md には repo の名前を書かない。

D4. 新しい Rust の型・gate・config 欄は足さない。size check は既存の command check、reviewer への材料は既存の `ReviewRequest`（checks・health）で足りる。

## 結果

- 削減の目的は planner（task の Objective）、WU の worker（全文の task 全体の目的）、reviewer（task の Objective・受け入れ条件・check の結果・diff）のどれにも欠けずに届く。
- 純増・移動だけ・測定の欠損は size check が落ちて review が不合格になり、reviewer の判定と health の verdict では覆らない。通常の task は規約 11 の対象外で、純減を求められない。
- 試験は `crates/task-dispatch/src/health_review/reduction_flow_tests.rs`（本物の dispatcher・本物の `build_prompt`・本物の `source-size-report.py` を一時 git repo で回す 6 例）。
- WU の prompt は長い目的の task で 1,500 文字を超えて長くなる。task の Execute の prompt は前から全文を載せており、同じ長さになるだけである。
