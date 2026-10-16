---
title: "MLSys 本測定: 試行の入力を workspace 内の kb/ に接続し、クラスタ実験は評価側の取得記録と照合する"
tasks: [01M530Y2NWW4ATM6N5WMQCCS9F]
status: accepted
date: 2026-10-16
---
# 試行の入力隔離と独立な実験証拠

## 背景
- runner は試行ごとに KB を複製していたが、複製先（`trials/.../kb`）は workspace の外で、A（`claude -p`）も B/C（Celeris task）も参照できなかった。参照したのは mock 実行器だけ。
- grader の実験証拠検査は成果物内の job 記録（job id・scheduler 記録・ログ・生データ hash）の整合しか見ず、エージェントが書いた整合的な自己申告で通り得た。

## 決定
- D1: 初期 KB は `materials/` と同じく試行の初期木（`workspace-seed`）に `kb/` として入れる。`TrialRequest.kb` は `<workspace>/kb`。全 arm・全反復が同じ初期 snapshot から始まり、通常運用 KB の path は試行に渡さない。repo 自身に `kb/` があれば衝突として拒否する。`rec.kb` は `…/workspace/kb`。
- D2: 実験 case（`experiment` を持つ非 control case）は、評価側が取得した job 記録 `celeris.observed-jobs/1`（成果物の外。`jobs[]` に `job_id`・`exit_status`・`conditions`・`raw[{path,sha256}]`）を要する。記録が無ければ不合格。成果物の各 job は、id が記録にあり、exit_status 0、case の条件と一致し、生データ sha256 が取得値と一致しなければ不合格。
- D3: `grade --observed <file>`、`grade-out` は `<out>/observed/<problem>/<Config>-rep<N>.json` を読む。記録の取得（qstat/sacct の実行）は評価側（人または評価 command）が行い、汎用の来歴基盤は作らない。control case は記録なしで通る（最終合格にはならない）。

## 帰結
- 旧い「成果物内の記録だけで実験完了」の経路は、control 以外では無くなった（互換分岐なし）。
