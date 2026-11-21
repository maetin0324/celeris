---
title: 承認要求の needed_before の取りこぼしの再現と本番の再監査（decision-failclosed / repro）
tasks: [01M7XSGW4FH258N1K3AMW76Y6F]
status: done
updated: 2026-11-21
---
# 承認要求の needed_before の取りこぼしの再現と本番の再監査（decision-failclosed / repro）

完了日: 2026-11-21

## 結論

- アーキv4 E の子 task 01M7Q5ZABN9NFQCH6QBTTP3EZ8 で、WorkUnit `target-leaf` の run（01M7QDR149BTFN2FXBB9K6VNW4）が
  決定 `work-units-size-trim` を `needed_before: ["integrate-impl"]` で求めた。daemon は要求を捨てて進行の 1 行（seq 436、
  2026-11-18T14:46:28Z）だけを残した。その後 decisions の行・decision の event・受信箱の項目は 1 件も作られず、
  `integrate-impl` は答え無しに走って done になり、task は seq 568（2026-11-18T17:10:51Z）で done に達した。
  `reproduce.py verify` が fixture でこの流れを再現し、exit 0。
- 原因は修正前の commit（ADR を足した commit の first parent）の 2 か所にある（下の「原因」）。
- 本番の再監査: 修正を含む最初の昇格（promoted_at 2026-11-19T01:25:29Z）の後、進行の行は 0 行、key rejected の決定は 0 件、
  対応しない行は 0。worker の決定の要求そのものがこの期間に 0 件で、修正後の経路は本番ではまだ一度も使われていない（未確認の事項を参照）。

## 置いた file

| file | 中身 |
|---|---|
| `fixtures/e-target-leaf-events.json` | events seq 435〜568 の 134 件（type・seq・ts・状態の欄・run_id。進行の文は seq 436 の 1 行、worker の報告は決定を求めたと書いた 1 文だけ）と、seq 419・422 の文脈、task の decisions の全行、件数 |
| `reproduce.py` | stdlib だけ。`verify` で fixture の再現と、修正前の file:line の文字列の実在を確かめる。DB は読まない |
| `extract.py` | 本番 DB を `mode=ro` で読み、fixture（`fixture`）と再監査（`audit`）の JSON を標準出力に出す |
| `audit.json` | 再監査の結果 `{"since","progress_lines","rejected_decisions","matched","task_ids","unmatched"}` |

## 取りこぼしの events（fixture の要点）

| seq | ts (UTC) | type | 内容 |
|---|---|---|---|
| 422 | 2026-11-18T14:20:43 | worker_progress | worker の最後の文「so I need your OK on that before integrate-impl.」 |
| 435 | 2026-11-18T14:46:27 | work_unit_transitioned | `target-leaf` running → done（completed） |
| 436 | 2026-11-18T14:46:28 | worker_progress | `worker の決定の要求を記録できませんでした（ADR-0079 D7）: decision work-units-size-trim: needed_before names an unknown unit "integrate-impl" (use self, a unit key of the plan or stage:<key>)` |
| 437 | 2026-11-18T14:46:28 | worker_finished | 報告に「人の確認を decision work-units-size-trim で求めた（integrate-impl の前まで）」 |
| 440 | 2026-11-18T14:46:28 | work_unit_transitioned | `integrate-impl` pending → running（integrate） |
| 462 | 2026-11-18T15:35:01 | work_unit_transitioned | `integrate-impl` running → done（integrated） |
| 562 | 2026-11-18T16:46:12 | transitioned | task running → reviewing（worker_done） |
| 568 | 2026-11-18T17:10:51 | transitioned | task reviewing → done（review_pass） |

- task の decisions の行は全部で 1 行（key `record-run-write-sets-home`、unit `write-set`、2026-11-18T12:47:51Z に作られ
  12:48:21Z に回答済み）。これは別の正しく受け付けられた要求で、受信箱の項目もある。seq 436 の後に作られた行は **0 行**、
  key `work-units-size-trim` の行は 0 行、decision_* の event は 0 件、この task を指す受信箱の項目は 0 件。
  依頼文の「decisions の行数（0）」は、この「seq 436 の後の行数」に当たる。fixture では `decisions_rows_after_progress: 0`
  と全行の一覧 `decision_rows` を分けて持つ。

## 原因（修正前の commit の file:line）

修正前の commit = `git log --diff-filter=A -- agent-docs/adr/2026-11-18-rejected-worker-decisions-reach-human.md` が引く commit
（件名「adr: 受け付けられない worker の決定の要求を 1 件の決定にまとめて人に出す」）の first parent（`^1`）。

- `crates/task-dispatch/src/dispatcher/tree_units.rs:390` — `.filter(|u| u.kind != task_core::WorkUnitKind::Integrate)`。
  `needed_before` に書ける unit から Integrate の unit を除く（これ自体は正しい。ADR の「原因」節）。このため
  `integrate-impl` は unknown unit になる。
- `crates/task-core/src/decision.rs:628` — `bad = Some(format!("unknown unit {n:?}"));`、
  `:638`〜`:639` — `out.rejected.push(format!("decision {}: needed_before names an {why} (use self, …)"`。
  拒否した要求は理由の文字列として `rejected` に積むだけ（`:568`〜`:569` の doc「捨てた要素と理由（人に見える進行の 1 行にする）。」
  `pub rejected: Vec<String>,`）。
- `crates/task-dispatch/src/dispatcher/tree_units.rs:419`〜`:422` — `for reason in &batch.rejected {` →
  `events.push(Event::worker_progress(` → `format!("worker の決定の要求を記録できませんでした（ADR-0079 D7）: {reason}")`。
  `rejected` の使い道はこの進行の 1 行だけ（この file で `batch.rejected` を読むのは 1 か所）。
- `crates/task-dispatch/src/dispatcher/tree_units.rs:434`〜`:438` — `if batch.accepted.is_empty() {` のとき
  `held_rows: Vec::new(),`・`self_hold: false,` で返す。決定の行も止める unit も作らず、run は成功のまま進む。

`reproduce.py verify` は、この 12 か所の行の文字列を `git show <ADR の commit>^1:<path>` で確かめ、fixture の seq 436 の文が
`tree_units.rs:422` と `decision.rs:639` の format の文字列から組み立てた文と 1 文字も違わないことを確かめる。

## 本番の再監査（(d)）

- 修正の commit: 件名「worker の決定: 受け付けられない要求を 1 件の決定にまとめて人に出し、その仕事を止める」（main にある）。
- 最初の昇格の決め方: `/local/celeris/state/releases/*/promoted.json` の release のうち、修正の commit を含み
  （`git merge-base --is-ancestor <修正> <release>` が 0）、`from` の release が修正を含まないものの中で promoted_at が最も早いもの。
  - 残っている promoted.json は 4 件（promoted_at 2026-11-19T05:35:55Z・2026-11-19T11:30:18Z・2026-11-20T15:44:15Z・
    2026-11-20T17:09:27Z）。全件が修正を含むが、最も早い 05:35:55Z の `from` の release も修正を含む。
  - その `from` の release の directory は release.sh の保持数（current・previous・新しい 3 件）で消えていた。
    `/local/celeris/state/logs/promote-*.log` の `promoted.json: … (mode=live from=…)` の行で補った。
    この release は 2026-11-19T01:25:29Z に昇格し、`from` は ADR を足した commit の first parent（修正を含まない）。
    残っている promoted.json と log の時刻は 4 件とも秒まで一致する。
  - よって `since = 2026-11-19T01:25:29Z`。
- 数え方: `since` 以後の events のうち type `worker_progress` で msg が `worker の決定の要求を記録できませんでした` で始まる行と、
  `since` 以後に作られた decisions のうち key が `rejected` / `rejected-N`、`raised_by.origin` が worker の行を、
  `run_id`（進行の行の `run_id` と決定の `raised_by.run_id`）で対応させる。
- 結果（2026-11-21T01:10Z 時点の DB。events の最新は 2026-11-21T01:08:23Z）:

| 欄 | 値 |
|---|---|
| since | 2026-11-19T01:25:29Z |
| progress_lines | 0 |
| rejected_decisions | 0 |
| matched | 0 |
| task_ids | なし |
| unmatched | なし |

- 文脈（audit.json には入れていない数）: `since` 以後に作られた決定は planner 5 件・daemon 1 件・worker 0 件、
  `worker_finished` は 159 件。修正前の進行の行は DB 全体で 14 行・11 task
  （2026-10-01〜2026-11-18。最後が本件の seq 436）で、全部が key rejected の決定を伴わない。

## 手順（再実行）

repo の root で:

```sh
D=agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed
python3 -I $D/extract.py fixture > $D/fixtures/e-target-leaf-events.json   # 本番 DB を mode=ro で読む
python3 -I $D/extract.py audit > $D/audit.json                             # 同上 + releases・promote の log・git
python3 -I $D/reproduce.py verify                                          # DB を読まない
```

fixture の events は追記専用の表から切り出しているので、再実行しても同じ内容になる。audit.json は実行時点の DB で数える。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/reproduce.py verify` → exit 0
  （進行の行 seq 436、done の seq 568、修正前の file:line 12 か所が一致）
- fixture を一時的に壊した 5 通り（seq 436 の後に decisions の行を足す・受信箱の件数を 1 にする・decision の event を置く・
  進行の行を消す・hash を入れる）→ どれも理由を出して exit 1。fixture は戻して exit 0 を確かめた
- 引数なしの `reproduce.py` → usage を出して exit 2
- 本番 DB は `sqlite3 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro'` と `extract.py` の `sqlite3.connect(…mode=ro, uri=True)`
  だけで読んだ。書き込みはしていない
- `sh "$CELERIS_WU_SCOPE_PATHS"` → この dir の 5 file だけ
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未確認の事項

- 修正後の経路（`rejected` の決定・leaf の `blocked(decision)`）は、本番ではまだ一度も起きていない（worker の決定の要求が
  `since` 以後 0 件）。本番で効くことは試験（revert の unit）でしか確かめていない。
- 消えた release の昇格時刻は promote の log から取った。log が消えると `extract.py audit` はその release を数えられない。

## 提案

- release の directory を消すときも promoted.json（または昇格の 1 行）を残すと、再監査が log に頼らずに済む。
