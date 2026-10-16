---
task: grade-case-gen (監査修正5)
status: 実装済み・mock検証済み（実 case 適用は k-cases WU）
completed: 2026-10-16
---
# grade-case/1 生成と CommandRunner（grade-case-gen）

## 実装済み（crates/campaign）
- `grade_case_gen::generate`（新）: 公開 `celeris.campaign-case/1` + grader 側 private の `hidden_command` から
  `celeris.grade-case/1` と `pristine/`・`hidden/`・`<id>.json` を書く。`definition_sha256`（grade-case 本文の hash）を返す。
  campaign-case-private/1 とは別型・別 schema（grade-case は command・テスト hash・実験条件だけ）。
  shell を要する command は拒否、`-- <名前>` を `must_pass` にする。模範解答は作らない（テスト file は呼び出し側が渡す）。
- `ImplementationSpec` に `hidden_dir` / `hidden_tests`（既定空）を追加。公開 `tests` は削除・改変で失格、
  隠しテストは grader 側の写しの hash だけ検査し、作業 dir へ挿入のみ。成果物に無くても削除判定にせず、在っても上書き。
  公開と隠しの path 重複は case 検証で拒否。
- `NoRunner`（新）: 評価 command を走らせず必ず不合格（fail-closed）。実走は既存 `grade_runner::ProcessRunner`（再利用、exit code を採点に反映）。

## 検証（mock のみ。ネットワーク・モデル無し）
`cargo test -p celeris-campaign --test grade_case_gen` → 10 passed。公開テスト改変/削除=失敗、隠し不在=合格、
成果物側の隠し名 file は上書きで合格、grader 側隠し写しの改変=失敗、exit 1=失敗、NoRunner=失敗。

## 未実施・未解決
- K1〜K4 実 case への適用（テスト file の用意、既知誤実装/修正版の確認）は k-cases WU。
- 修正1 の final.json grader 結合キーはこの branch に存在しない。`definition_sha256` を渡せる形で用意したが、結合は未実施。
- 実験工程の条件は `nodes` のみ。recompute/review は未生成（private に file/keyword が無いため）。

## 提案
- 結合キーは `definition_sha256` + hidden/pristine の各 sha256 を含める。
