---
title: 判定 JSON に隠しテストの path を出さない（K2 の wrapper 漏洩検査 exit 71 の修正）
tasks: [01M67YEFQQHB054QWVS3GG33GX]
status: done
updated: 2026-10-31
---
# 判定 JSON に隠しテストの path を出さない

## 実施
- grader: `hidden_inserted`（path 一覧）を廃止し `hidden_inserted_count` に。tail・失敗理由は `redact_hidden` で `<hidden[i]>` に置換。
- 試験: grade_case_gen（判定 JSON 全体に fixture の隠しテスト名が無い・compile error の stderr/stdout の置換）、k_cases_grade（件数欄・旧欄なし）。`cargo nextest run -p celeris-campaign --test grade_case_gen --test k_cases_grade` は 19 件合格。
- ADR 付記: `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md`「判定 JSON に隠しテストの path を出さない」。

## 検査（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `bash scripts/dev/test-affected.sh`: exit 0、nextest 2898 件合格・9 件 skip、文書検査も合格。
- `cargo clippy --workspace -- -D warnings`: exit 0（`sort_by` を `sort_by_key` に直した後）。
- `cargo fmt --all -- --check`: exit 0。

## commit（件名）
- grade: 判定 JSON に隠しテストの path を出さない（件数欄と tail の置換）
- 検査結果と clippy 指摘の修正

## 未解決
- wrapper 試験 (c)（漏洩検査の private に隠しテスト名を持たせ、本物の binary の判定が outbox に置かれる）は書いていない。本番での確認は下の移行手順 2 で行う。
- 漏洩検査の規則（12 字以上の private 文字列）は変えていない。

## 本番への移行
1. operator が release の celeris-campaign を `/var/lib/celeris-eval-bin` に入れ替える。
2. `agent-docs/progress/2026-10-30-k2-grade-eval-uid.md` の手順 4 で wrapper 経由の採点をやり直す。期待: wrapper exit 0、outbox に判定 JSON。
3. grade-case の形式は変えていないので、grade-case の作り直しは不要。
