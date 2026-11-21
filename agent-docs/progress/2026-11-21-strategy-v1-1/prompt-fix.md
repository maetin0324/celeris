---
title: 戦略層 v1 子 1 prompt-fix — 子 task の worker・planner・WU・reviewer の実 prompt に祖先の目的と受入条件の全文を出す
tasks: [01M7XQ9Q4P4ZQ38Y509J9XYAQ0]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# prompt-fix（WorkUnit。子 1 の受入条件 2 の修正）

完了日: 2026-11-21（範囲の外に残った 1 行は WorkUnit e2e-protocol が直した。下の「範囲の外に残った 1 行」）。WU の check 0〜4 は提出時の HEAD で全て exit 0。設計の記録は [ADR 2026-11-21-ancestor-objective-in-prompts](../../adr/2026-11-21-ancestor-objective-in-prompts.md)。

## 結論

- prompt-audit の欠け 1〜4 の経路を直した。子 task（`parent_id` か木の親を持つ task）の worker・planner・WU・reviewer（final review と
  設計 review）の run の prompt に、祖先（root から親まで）の objective と受入条件の全文が「祖先の task の目的と受け入れ条件」節として
  1 回ずつ載る。CoS が親の文を子の objective に写したかによらない。
- 欠け 5（Project.request）は直していない。理由は ADR の「欠け 5 を扱わない理由」と下の提案に書いた。
- 計画の無い root の task（祖先の無い task）の prompt は 1 byte も変わらない（試験で確かめた）。
- この修正は、release が本番に昇格されるまで本番の run には効かない。昇格は人だけが行う。

## 前回の run の check の不合格と、この run の直し

前回の run（設計 v1。commit「strategy-v1 1 prompt-fix: 子 task の worker・planner・WU・reviewer の prompt に祖先の目的と受入条件の全文を出す」）
は、WU の check `bash scripts/dev/test-affected.sh && …` で落ちた。test-affected が最初に走らせる repo health の
`size.responsibility-growth` が fail 級 4 件を出した:

| 指摘 | base | v1 | 原因 |
|---|---|---|---|
| `RunContext` の欄の数（閾値 40） | 40 | 41（new） | `ancestors` を 41 番目の欄として足した |
| dispatcher module の本体の行（閾値 8000） | 32316 | 32328（worsened） | `run_worker`・`pick_reviewer` に祖先を組む行を足した |
| `run_worker` の関数の行（閾値 200） | 803 | 808（worsened） | 同上 |
| `Dispatcher::pick_reviewer` の関数の行（閾値 200） | 210 | 221（worsened） | 同上 |

この run の設計（v2。commit「strategy-v1 1 prompt-fix: 祖先と子を RunContext.lineage にまとめ、祖先の走査を 1 関数にして repo health の上限を守る」）:

- `RunContext.children` と祖先を `RunContext.lineage: TaskLineageContext { ancestors, children }` にまとめた（欄の数は 40 のまま。
  どちらも store の task の木の上下）。
- 祖先の走査は `crates/task-dispatch/src/prompt_ancestors.rs` の `chain_to_root` 1 か所にした。ADR-0117 D1 の人の決定・回答の収集
  （`Dispatcher::review_human_inputs`）が持っていた走査（木の親、無ければ `parent_id`。32 hop）をそのまま移し、両方が使う。
  これで dispatcher module の行が減り、祖先を足した分を相殺した。
- worker・planner・WU は `run_extras` の 1 行（`lineage: prompt_ancestors::lineage(...)?`）で組む。`run_worker` は欄の名前を変えただけ。
- final review は `review_human_inputs` が人の決定・回答と祖先を `ReviewHumanInputs` で返し、`ReviewExtras.human_inputs` →
  `ReviewHumanContext.inputs` → `run_reviewer_inner` の `RunContext.lineage` に写す。`pick_reviewer`・`spawn_review` は伸ばしていない。
- 設計 review は `start_design_review`（`health_review/design_gate.rs`。`#[path]` で dispatcher module に入る）が 1 行で組む。

v2 の repo health（size の規則だけ、提出時の作業ツリー）: `verdict=pass exit=0 … fail_grade=0 new=0 worsened=0`。

## 設計 review の指摘への対応（この run）

commit「strategy-v1 1 prompt-fix: 設計 review の祖先の読み取りを WorkerStarted の前に移し、wire の版を 5 に上げる」と
commit「strategy-v1 1 prompt-fix: wire の版の試験を 5 に合わせる」。

| 指摘 | 対応 | 確かめ方 |
|---|---|---|
| (1) `start_design_review` の祖先の読み取りが `WorkerStarted`・runs の行・`quota_begin` の後にある | `prompt_ancestors::lineage(...)?` を `run_watch(...)?` の直後（`pick_reviewer` と `Event::WorkerStarted` の append の前）に移した。読めなければ run を起こさずに返る | WU の check 2: `run_watch=283 lineage=286 started=298` |
| (1) ADR D3 の位置の記述 | D3 を書き直し、付記に 3 経路の位置を書いた: final review は `review_human_inputs(&task)?` が WorkerStarted より前（base と同じ）、設計 review は上のとおり前、worker・planner・WU は `run_extras(...)?` の中で WorkerStarted の後（base でも同じ所で `?` で store を読む） | ADR の D3 と付記 |
| (2) `dispatcher.rs` の `has_child_failed_transition` の doc comment が `context.children` | `context.lineage.children` に直した | check 2: `grep -n 'context\.children' crates/task-dispatch/src/dispatcher.rs` が 0 行 |
| (2) `PROTOCOL_VERSION` が 4 で、doc が「全て追加のみ」 | 5 に上げ、doc に「v5 は `context.children` と `objective_excerpt` を消した」と書いた。`docs/protocol/worker-protocol.md` の版の行・`run.protocol` の行・例 2 か所を 5 にし、「4 のまま」の記述を「上げない」にした。schema を `UPDATE_SCHEMA=1` で作り直した。ADR に D5 を足した | check 2: `pub const PROTOCOL_VERSION: u32 = 5;`。`committed_schema_matches_generated` が pass |
| (2) 版を数値で比べる所の確認 | `git grep -n PROTOCOL_VERSION` で production のコードは代入だけ（比べる所は無い）。試験の 3 か所が 4 を固定していた: `crates/task-worker/src/protocol/tests.rs` の `v["protocol"] == 4`（5 に直した）、`crates/task-worker/src/subprocess/tests.rs` の comment 行の試験の末尾の `assert_eq!(PROTOCOL_VERSION, 4)`（消した。comment 行の契約は残りの assert が守り、版の値は前者が守る）、`tests/e2e/tests/scenarios.rs:437` の `"protocol":4`（範囲の外。WorkUnit e2e-protocol が 5 に直した。下の節） | `git grep -nE 'v\["protocol"\]|PROTOCOL_VERSION, ?4|"protocol":4' -- crates tests` |
| (3) `review_spawn.rs` の先頭の use 行 | `use crate::review::RepoHealth;` を base に戻し、`ReviewHumanInputs` は `crate::review::ReviewHumanInputs` と full path で書いた。root-artifacts の branch は merge していない | `git diff <WU の base> HEAD -- crates/task-dispatch/src/dispatcher/review_spawn.rs` に use 行の差が無い。WU の check 3: `git merge-tree --write-tree --name-only celeris-wu/01M7XQ9Q4P4ZQ38Y509J9XYAQ0/root-artifacts HEAD` が exit 0 |

`crates/task-worker/src/subprocess/tests.rs` は高 risk の path class `sandbox` に入る。変えたのは試験の版の固定の 1 行と doc comment の 1 文だけで、
sandbox の不変条件（`worker-db-read-only`・`worker-no-subagents`）に関わるコードは変えていない。

## 範囲の外に残った 1 行（WorkUnit e2e-protocol で解消）

- `tests/e2e/tests/scenarios.rs:437` の `assert!(run1.starts_with(r#"{"type":"run","protocol":4"#));` は版 5 では落ちる。e2e の crate は
  task-worker に依存しないので test-affected は走らせず、release gate の `cargo nextest run --workspace` だけが走らせる。
- WU の範囲 check（check 4）は `tests/e2e/` を許さない。範囲の外に回避策を置かないため、この run では直していない。
- 求めた直し: 範囲 check の正規表現に `tests/e2e/tests/scenarios.rs` を足す。直しは 437 行の `4` を `5` にし、直前の comment（435〜436 行）を
  「v5 は `context.children` を `context.lineage.children` に移した」に合わせる。`tests/e2e/tests/delegation_scenarios.rs` の 269・618 行の
  doc comment の `context.children` も古い（fake worker の `grep -q '"children":\[{'` は `"lineage":{"children":[{` にも当たるので動きは変わらない）。
- 解消（2026-11-21、WorkUnit e2e-protocol。commit「strategy-v1 1 e2e-protocol: e2e の worker protocol の literal を 5 に直す」）:
  計画が e2e-protocol の unit を足し、その unit が `scenarios.rs:437` の `4` を `5` にし、435〜436 行の comment を v5 の内容に直した。
  `delegation_scenarios.rs` の 269・618 行の doc comment は `context.lineage.children` に直した（fake worker の script は変えていない）。
  証拠（詳細は [e2e-protocol.md](e2e-protocol.md)）:
  - 直す前: `cargo nextest run --workspace --no-tests=fail -E 'binary(scenarios)'` → exit 100、4 件中 1 件 FAIL
    （`command_check_fails_once_then_passes_after_retry` が `tests/e2e/tests/scenarios.rs:437:5` の
    `assertion failed: run1.starts_with(r#"{"type":"run","protocol":4"#)` で panic）。
  - 直した後: `cargo nextest run -p e2e --no-tests=fail -E 'binary(scenarios)'` → exit 0、4 passed。
    `-E 'binary(delegation_scenarios)'` → exit 0、5 passed。

## 欠けの一覧ごとの対処（prompt-audit の「欠けの一覧」）

| 欠け | 経路 | 対処 | 試験 |
|---|---|---|---|
| 1 祖先の受入条件がどの role にも入らない | `prompt_header`（`prompt.rs`）が run 自身の task しか出さない | `ancestors_section` を `prompt_header` に足し、受入条件は `acceptance_lines` で出す | `prompt_ancestor_objective_every_builder_carries_full_ancestor_text_once`、dispatcher の 2 件、設計 review の 1 件 |
| 2 planner で祖先の objective が 300 字で切れる | `tree_planner_context` の `objective_excerpt` | `objective_excerpt` を消し、全文は祖先の節で出す（木の祖先の行は題名と段階だけ） | `prompt_ancestor_objective_child_planner_and_work_unit_runs_carry_the_parent`、`planner_prompt_carries_depth_and_leaf_criteria` |
| 3 木の外の祖先（CoS の `parent_id` の親）が出ない | `ancestors_with_self` が木の親だけを辿る | 祖先の節は木の親、無ければ `parent_id` で root まで辿る | 同上（親は `parent_id` だけで繋いだ task） |
| 4 worker・reviewer に祖先の節が無い | `RunContext` に欄が無い | `RunContext.lineage.ancestors` を worker・WU・reviewer・設計 review の run に載せる | `prompt_ancestor_objective_child_worker_and_reviewer_runs_carry_the_parent`、`prompt_ancestor_objective_design_review_carries_the_parent` |
| 5 Project.request がどの prompt にも無い | — | 直していない（提案節） | — |

## 修正前に落ちた出力

dispatcher の 2 件（`crates/task-dispatch/src/dispatcher/tests/prompt_ancestor_objective.rs`）は、修正前の production の tree（この WU の base = commit
「strategy-v1 1 prompt-audit: 修正後の本番 run の実 prompt を再監査する」の production に試験 file だけを足した tree）で 4 経路とも落ちた。この 2 件は `RunRequest` の prompt を見る端から端の試験で、
v1・v2 で試験の file を変えていないので、記録は v2 にも当てはまる。出力の要点（全文は run の artifacts の `before-fix-dispatch.log`）:

```
FAIL … prompt_ancestor_objective_child_worker_and_reviewer_runs_carry_the_parent
    "child worker: parent objective tail x0 (want x1)",
    "child worker: parent acceptance tail x0 (want x1)",
    "child reviewer: parent objective tail x0 (want x1)",
    "child reviewer: parent acceptance tail x0 (want x1)",
FAIL … prompt_ancestor_objective_child_planner_and_work_unit_runs_carry_the_parent
    "child planner: parent objective tail x0 (want x1)",
    "child planner: parent acceptance tail x0 (want x1)",
    "child work unit: parent objective tail x0 (want x1)",
    "child work unit: parent acceptance tail x0 (want x1)",
```

task-worker の 2 件と設計 review の 1 件は新しい欄（`RunContext.lineage`）を組むので、修正前の tree では compile できない（落ちる出力は無い）。

## 試験が守る不変条件と、既存の試験で検出できない理由

各試験の doc comment に書いた。要点: 既存の `prompt_full_objective_*` と `planner_prompt_acceptance_*` は run 自身の task の objective と
受入条件の末尾だけを見る。木の planner の試験は 300 文字の抜粋の短い印しか見ない。どれも祖先を持つ task の prompt を組まないので、祖先の
目的が切れても、受入条件が無くても落ちない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`（この WU の base）と `git merge-base HEAD main`）

- WU の check 0（対象の試験）:
  `cargo nextest run -p task-worker -p task-dispatch --no-tests=fail -E 'test(/prompt_ancestor_objective_/) | test(/prompt_full_objective_/) | test(/planner_prompt_acceptance/) | test(/acceptance_lines_/) | test(/health_review::design/)'`
  に `a_comment_line_is_passed`・`committed_schema_matches_generated`・`review_human_inputs` を足して走らせた → exit 0、`20 tests run: 20 passed`。
- WU の check 1: `bash scripts/dev/test-affected.sh` → exit 0。repo health `verdict=pass exit=0 findings=695 fail_grade=0 new=0 worsened=0`、
  nextest `6163 tests run: 6163 passed (7 slow), 21 skipped`。`cargo clippy --workspace -- -D warnings` → exit 0。
  `cargo fmt --all -- --check` → exit 0。
  - 版を 5 に上げた直後の test-affected は `protocol::tests::run_request_serializes_with_type_tag` の 1 件で落ちた（`v["protocol"] == 4`）。
    この試験を 5 に直して再実行した結果が上。
- WU の check 2（読み取りの位置・版・古い説明）: `run_watch=283 lineage=286 started=298`、版 5、`context.children` 0 行 → exit 0。
- WU の check 3: `git merge-tree --write-tree --name-only celeris-wu/01M7XQ9Q4P4ZQ38Y509J9XYAQ0/root-artifacts HEAD` → exit 0（衝突なし）。
- WU の check 4（範囲）: `sh "$CELERIS_WU_SCOPE_PATHS"` の path のうち範囲の外は 0 件 → exit 0。

## LoC（この WU の base = commit「strategy-v1 1 prompt-audit: 修正後の本番 run の実 prompt を再監査する」と HEAD。分類規則は base の `scripts/dev/source-size-report.py` と `source-size-report.toml` に固定）

`python3 <base の source-size-report.py> --root . --config <base の toml> --exclusive-classes --rev <rev> --format json`:

| 区分 | base | HEAD | 差 |
|---|---|---|---|
| production（全言語） | 376018 | 376159 | +141 |
| production（Rust） | 331327 | 331468 | +141 |
| external_test | 463385 | 463935 | +550 |
| own_total_with_tests | 1034966 | 1035657 | +691 |

この run の差（前回の提出から）: production +3（`PROTOCOL_VERSION` の doc の行）、external_test −1（`subprocess/tests.rs` の版の固定の行）。

production の純増の理由: 祖先の節（`ancestors_section`）、`TaskLineageContext`・`AncestorContext` の型、`prompt_ancestors.rs`（走査を
`review_human_inputs` から移した分を含む）、`ReviewHumanInputs`。消したもの: `review_human_inputs` の走査（12 行）、`TreeAncestorContext.objective_excerpt`、
`RunContext.children`（`lineage.children` に移した）、`ReviewExtras.decisions`・`answers`（`human_inputs` に移した）。LoC の純減はこの WU の目的ではない。

## 昇格後に本番で確かめる手順（人が release を昇格した後）

1. この commit を含む release が昇格されたかを見る: `git merge-base --is-ancestor <この commit> <sha12>` が 0 の release の
   `/local/celeris/state/releases/<sha12>/promoted.json` の `promoted_at`。
2. その後に子 task の worker・planner・WU・reviewer の run が走ってから、本番 DB を読むだけで再監査する（出力は作業場所の一時 dir に置き、
   commit した prompt-audit の tsv は書き換えない）:
   ```sh
   python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/prompt-audit/prompt_audit.py \
     collect --db /local/celeris/data/db/celeris.sqlite3 --out "$TMPDIR/prompt-audit-after"
   ```
3. `$TMPDIR/prompt-audit-after/prompt-audit.tsv` のうち `started_at` が 1. の `promoted_at` より後の行で、`kind` が `ancestor_objective` と
   `ancestor_acceptance` の行の `cut` と `missing` が 0 で、`result` が `found` であることを確かめる。WU の run（`wu_key` が `-` でない行）と
   reviewer の行も含める。prompt_audit.py の基点（`FIX_SUBJECT`）は 11-19 の planner の修正なので、昇格時刻の絞り込みは手で行う。

## 未解決事項

- 本番での再監査は未確認（昇格前のため）。手順は上に書いた。
- `tests/e2e/tests/scenarios.rs:437` の版の literal は、この WU の範囲の外だったので WorkUnit e2e-protocol が直した（上の節）。
- 設計 review の祖先の読み取りが store の失敗で返る経路に、失敗を注入する試験は無い。位置は WU の check 2（行番号の比較）だけが守る。
- `celerisctl worker run`（手動の run）と eval の trial の run は、最小の context で起こす既存の方針のままで祖先を載せない。
- 修正前の tree での失敗は dispatcher の 2 件だけを記録した。task-worker の 2 件と設計 review の 1 件は新しい欄を使うので、修正前の tree では
  compile できない。

## 提案

- 欠け 5（Project.request）: 案件の root の task の prompt に Project.request を出すかを、別の判断として人に聞く。出すと案件に属する root の
  task の prompt がすべて変わる（この WU は root の prompt を変えないことを求めた）。今の案件の Project.request は 345 字の案件の説明で、依頼の
  原文は CoS が root の task の objective に写しており、この修正でその全文が子 task の全 prompt に届く。
