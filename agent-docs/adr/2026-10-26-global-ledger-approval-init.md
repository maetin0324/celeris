# 共有台帳の承認を CLI で記録する（ledger-init）

- 状態: 採用（2026-10-26）
- 関連: `agent-docs/adr/2026-10-19-experiment-sets.md`（D5/D7 共有台帳）、`docs/ops/mlsys-main-4-pilot-runbook.md`（C5）

## 背景
共有台帳（`experiment_sets.<set>.fixed.shared_ledger`）は runner の `admit_start`・予約・精算だけが行を書き、
人の承認（誰が・いつ・範囲・期限・試行数）を記録する入口が無かった。runbook C5 は「台帳を初期化し caps が承認値と一致」を
求めるが、`measure-launcher.sh prepare` は共有台帳に触れない。

## 決定
- D1: `celeris-campaign ledger-init --manifest <m> --global-ledger <dir> --approval <json>` を置く。`--global-ledger` は
  manifest の宣言 path だけを受ける（`check_shared_ledger`）。
- D2: 承認の出典・範囲・期限・補足は台帳 dir の `approval.json`（schema `celeris.campaign-ledger-approval/1`）に置く。
  台帳の行（`deny_unknown_fields`）には入れない。既存の読み手（固定済みの campaign binary を含む）が同じ台帳を読める。
- D3: 承認の試行数（任意で tokens）は測定 ID の `measurement` 行の `caps` に入れる。上限は全行の最小なので増えない。
  違う承認の再記録は拒否する（引き上げは新しい台帳 path と manifest で行う）。同じ承認の再実行は冪等。
- D4: 消費は書かない（runner の予約・精算だけが消費を書く）。出力は台帳・manifest・実効の上限と、確定消費・残り試行数
  （承認の残りと、manifest との最小で効く残り）。
- D5: 入力の欠け（空の source/scope/expiry、`YYYY-MM-DD` でない期限、試行数 0）は何も書かずに失敗する。
