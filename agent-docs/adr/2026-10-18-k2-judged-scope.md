# ADR 2026-10-18: K2 の判定範囲を事前に明示する（analysis / review は今回判定しない）

- 日付: 2026-10-18
- 状態: 採用・実装済み
- 関連: 2026-10-16-mlsys-independent-grader、2026-10-17-campaign-lock、2026-10-18 本測定移行 P1-3（grader-real）

## 背景
K2 は stages に analysis・review を持つが、grade-case の recompute / review は null で、採点器は実装テストと実験証拠しか
判定しない。この状態で報告が「分析・レビューまで検証済み」と書けてしまうと、判定していない工程を判定済みと主張する。

## 選択肢
1. K2 に最小の分析再計算・レビュー判定を追加する。→ 既知の誤り list と模範解答を新設することになる。結果を見た後に grader を
   変えるのを避けたい今回の方針（巨大な問題集を作らない）に反する。
2. 判定範囲を事前に明示し、判定しない工程を主張させない。**採用**。

## 決定
- D1 `case.json` に `judged_scope`（stages の部分集合、省略時は stages 全部）を置く。K2 は `["implementation","experiment"]`。
  stages との差が `not_judged`（K2 は analysis, review）。
- D2 `celeris.grade-case/1` に任意の `judged_scope` を置く。持つ場合、analysis ⇔ recompute、review ⇔ review が一致しない定義は
  不正（not_judged を判定する定義も、judged を判定しない定義も作れない）。recompute / review が null でよいのは not_judged のときだけ。
  `GradeReport` は `judged_scope` と `not_judged` を出す。
- D3 報告は `judged_scope stages=<..> not_judged=<..>` 行を持たねばならず（not_judged がある case を含む campaign）、
  `verified stage=<s>` で not_judged の工程を主張する行、および「分析・レビューまで検証済み」の定型句は `check-report` が違反にする。
  `check-report --cases-root <dir>` が case.json から範囲を引く。`report::scope_line` が正しい行を生成する。
- D4 case.json / grade-case を変えたので grade_case_ref.sha256 と fixtures を更新し、lock（grade_case_sha256）は新しい hash で
  作り直す。旧 lock の試行とは混ぜない（lock の successor 検査どおり）。

## 結果
K2 の成功は「実装と実験の判定を通った」ことだけを意味する。分析・レビューを判定するには別 ADR で case を追加する。
