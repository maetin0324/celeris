---
title: 戦略層 v1 子 5 adr-scope — ADR・skill・進捗の「結果を見てから選ぶ」の保証を gate の実装の範囲に直し、穴を未確認に書く
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
wu: adr-scope
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# adr-scope（WorkUnit。子 5 の final review の不合格（条件 4）の修正）

完了日: 2026-11-22。設計の正本は [ADR 2026-11-22-strategy-review-critic](../../adr/2026-11-22-strategy-review-critic.md)。
基準はこの WU の `$CELERIS_WU_BASE`。検査はすべて提出時の HEAD で実行した。コード・schema・試験は変えていない。

## 結論

- 子 5 の final review（reviewer 01M826CFDQH1YGY0DZG7GHDZ1J）は条件 4 を不合格にした。理由は、ADR の D2.4 の 6・D4.2・守る不変条件の 1 項目めが、
  実装の保証しない性質（選ぶ版を実験の結果を読んでから書いたこと）を gate が保証すると書いていたこと。
- 実装の事実（読んで確かめた）: `crates/task-core/src/strategy/review.rs` の `check_execution` は、前の版の実験 task が 1 件以上あり、`task.create` の時点ですべて終端かを見る。
  `check_failures_read` は、終端の task が版の証拠の `task`・`artifact` の参照に現れるかだけを見る。`crates/task-api/src/cos/strategy_gate.rs` の `Reader::lookup` は
  `Reference::Task` を task の status によらず実在すれば `Found` にする。したがって、走っている実験 task を先に参照して `selected` を書いた版は、実験が走る間は
  422 `strategy_experiment_phase` で、実験が終われば D2.4 の 6 も D4.2 も通る。選ぶ版を実験の結果より先に書くことは gate では止めない。
- この WU が直したこと:
  - ADR: D2.4 の 6 の 2 文目、D4.2 の最後の文、守る不変条件の 1 項目めを、上の範囲だけを保証する文に書き直した。3 か所とも「gate では止めない」を含む。
    「未確認と残る穴」に、この穴・確かめる者（人の承認 D3.7 と Critic の C4）・決定的に閉じる案（版の `ArtifactProduced` の時刻と前の版の実験 task の終端の event の時刻の比較）と、
    採否を決める者（子 6 の設計か親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE の closeout で人）を足した。末尾に節「付記（2026-11-22、gate が保証する範囲の訂正）」を足した。
  - skill `config/skills/cos-operator/strategy-review.md`: 17 行目の compound が通る条件の文を同じ範囲に直し、「gate では止めない」と、結果を読んでから次の版を書くのは
    CoS の責任であることを書いた。手順の意味は変わらないので `SKILL.md` の `metadata.version`（`"8"`）は変えていない。`SOURCE.md` の履歴に 1 行足した。
  - 進捗: 親の進捗 [2026-11-22-strategy-v1-5.md](../2026-11-22-strategy-v1-5.md) の結論の言い過ぎを訂正し、条件 1 に限界、WorkUnit の表に adr-scope の行、
    commit の列挙、「未確認の事項」にこの穴、「提案」に決定的に閉じる案と判断者、「検査」に提出時の HEAD の結果を書いた。
    [experiment-gate.md](experiment-gate.md) の brief の不変条件（39 行目）と試験の表の 1 行、[shadow-case.md](shadow-case.md) の試験の表の 1 行の同じ言い過ぎを直した。

## 変更（commit の件名）

- `strategy-v1 5 adr-scope: ADR・skill・進捗の gate の保証を実装の範囲に直し、選ぶ版を結果より先に書ける穴を未確認に書く`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

| 受け入れ条件 | 実行したこと | 出力の要点 |
|---|---|---|
| 0. D2.4 の 6・D4.2・守る不変条件が gate の保証を「`task.create` の時点で前の版の実験が全部終端で、全部版の証拠に参照されている」までと書き、消した 3 文が無い | `grep -c 'gate では止めない' agent-docs/adr/2026-11-22-strategy-review-critic.md`、消した 3 文の語句の否定 grep | `4`（D2.4 の 6・D4.2・不変条件・未確認と残る穴）。消した語句は 0 件 |
| 1. 未確認と残る穴に、穴・確かめる者・決定的に閉じる案と判断者 | ADR の「未確認と残る穴」を読む | 3 つ目の項目に、穴（走っている実験 task を先に参照した版も実験が終われば通る）、確かめる者（人の承認 D3.7 と Critic の C4）、閉じる案（`ArtifactProduced` の時刻と終端の event の時刻の比較）、判断者（子 6 の設計か親の closeout で人）がある |
| 2. 末尾の付記 | `grep -n '付記（2026-11-22、gate が保証する範囲の訂正）' agent-docs/adr/2026-11-22-strategy-review-critic.md` | 末尾の節として 1 件。直した節の番号と理由を書いた |
| 3. skill の compound の条件の文と SOURCE.md、version を変えない | `grep -c 'gate では止めない' config/skills/cos-operator/strategy-review.md`、`git diff --stat $CELERIS_WU_BASE -- config/skills/cos-operator/SKILL.md` | `1`。SOURCE.md の末尾に履歴の 1 行。SKILL.md の差分 0（`metadata.version` は `"8"` のまま） |
| 4. 親の進捗と WU の進捗に訂正・未確認・提案・check の結果 | 親の進捗の結論・条件 1・WorkUnit の表・commit・未確認の事項・提案・検査の節、この file | 書いた |
| 5. crates/・docs/・web/・schema を変えていない | `sh "$CELERIS_WU_SCOPE_PATHS"` | 変えた file は ADR・skill 2 file（`strategy-review.md`・`SOURCE.md`）・進捗 4 file（親・adr-scope・experiment-gate・shadow-case）の 7 つだけ。`crates/`・`docs/`・`web/`・schema は 0 件 |

検査（提出時の HEAD で実行）:

- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (478 files)`）。
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- `bash scripts/dev/test-affected.sh` → exit 0（`repo-health: verdict=pass exit=0 … fail_grade=0 new=1 worsened=0`、`repo_health_gate: all checks passed`、`test-affected: 試験対象の変更なし（docs のみ等）`）。
- `cargo clippy --workspace -- -D warnings` → exit 0（vendor の `deno_core` の既存の deprecated の警告 1 件だけ）。
- `cargo fmt --all -- --check` → exit 0。

## 未解決

- 選ぶ版を実験の結果より先に書くことは gate では止めない（この WU は文書を実装に合わせただけで、穴は閉じていない）。確かめるのは人の承認と Critic の C4 で、
  どちらも決定的ではない。
- gate の違反の文（`crates/task-core/src/strategy/review.rs` の `strategy_experiment_phase` の message の括弧書き）はコードの中の文で、この WU の範囲（コードを変えない）の外なので変えていない。

## 提案

- 決定的に閉じる案: gate が版 v<sub>n</sub> の `ArtifactProduced` の時刻と前の版の実験 task の終端の event の時刻を比べ、v<sub>n</sub> がすべての終端より後に作られていなければ拒否する。
  採否は子 6 の設計か、親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE の closeout で人が決める。
