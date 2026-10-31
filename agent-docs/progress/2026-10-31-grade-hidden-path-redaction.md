---
tasks: [01M67YEFQQHB054QWVS3GG33GX]
status: running
---
# 判定 JSON に隠しテストの path を出さない

## 実施
- grader: `hidden_inserted`（path 一覧）を廃止し `hidden_inserted_count` に。tail・失敗理由は `redact_hidden` で `<hidden[i]>` に置換。
- 試験: grade_case_gen（判定 JSON 全体に fixture の隠しテスト名が無い・compile error の stderr/stdout の置換）、k_cases_grade（件数欄・旧欄なし）。`cargo nextest run -p celeris-campaign --test grade_case_gen --test k_cases_grade` は 19 件合格。
- ADR 付記: `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md`「判定 JSON に隠しテストの path を出さない」。

## 未解決
- test-affected.sh・clippy・fmt の提出時 HEAD での結果は次の run で記録する（この run は壁時計で打ち切り）。
- wrapper 試験 (c)（漏洩検査を本物の binary の判定が通る）は未着手。

## 本番への移行
1. operator が release の celeris-campaign を `/var/lib/celeris-eval-bin` に入れ替える。
2. `agent-docs/progress/2026-10-30-k2-grade-eval-uid.md` の手順 4 で wrapper 経由の採点をやり直す。期待: wrapper exit 0、outbox に判定 JSON。
3. grade-case の形式は変えていないので、grade-case の作り直しは不要。
