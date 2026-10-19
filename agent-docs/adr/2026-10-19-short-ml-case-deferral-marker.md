# 短い ML case の延期記録 dir を CaseSet::load が case として読まない

- 日付: 2026-10-19 / 状態: 採用

## 背景
短い ML 研究 case は、既存資産・既存 grader では無 LLM 正負例が成立しないため延期した（R1〜R3、`crates/campaign/cases/dev/short-ml-campaign/status.json`）。受け入れ条件は延期記録を同 path に置くことを求める。だが `CaseSet::load` は `cases/<split>/` 下の全 dir に `case.json` を要求し、status.json だけの dir で 8 試験が落ちた。

## 決定
`case.json` が無く `status.json` がある dir は読み飛ばす（1 箇所）。`case.json` も `status.json` も無い dir は従来どおりエラー。ダミーの case.json は作らない（偽の case を測定集合に入れるため）。

## 影響
grader 本体・既存 case・manifest は不変。試験 `deferral_marker_dir_is_not_a_case` で固定。
