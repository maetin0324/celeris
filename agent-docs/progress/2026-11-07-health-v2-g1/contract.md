---
title: 健全性v2 G1 contract — repo health 契約の ADR・report schema・検証器
tasks: [01M6TYVJSRM3SFQ3VHWQE4MZ9M]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G1 contract — repo health 契約の ADR・report schema・検証器

完了日: 2026-11-07

WorkUnit `contract`（G1 の工程「契約の決定」）。基準は `$CELERIS_WU_BASE`（件名「integrate wu/g1-v0-design (phase design)」）。

## 結論
- 契約は [ADR 2026-11-07 汎用 repo health の契約](../../adr/2026-11-07-repo-health-contract.md) に D1〜D8 で決めた（入力と CLI、profile 解決と base 側 policy・policy.changed・bootstrap、6 状態と集約・exit code、finding の欄、rule ID、既存負債との差分、doc-only・tool 未導入・複数 repo・dirty・shallow・non-Git、report の形と決定性）。採らなかった案は各 D の末尾。
- report の形は `scripts/dev/repo-health/schema/report-v1.schema.json`、検証器は `scripts/dev/repo-health/validate_report.py`（標準ライブラリだけ。形 + 意味規則 S1〜S12）。
- examples 12 件は全件 exit 0、bad-examples 21 件は各 1 件ずつ exit 1。試験 `scripts/tests/test_repo_health_contract.py` が固定する。
- crates/・web/ は変えていない。repo_health.py 本体は core WorkUnit が書く。

## 変更（commit の件名）
- `phase G1: repo health 契約の ADR・report v1 schema・検証器・examples`

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `python3 -I scripts/dev/repo-health/validate_report.py scripts/dev/repo-health/schema/examples/*.json` → exit 0（12 valid, 0 invalid）
- `for f in scripts/dev/repo-health/schema/bad-examples/*.json; do python3 -I scripts/dev/repo-health/validate_report.py "$f"; done` → 21 件すべて exit 1、違反の場所（JSON pointer）付き
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → exit 0（4 tests）
- `bash scripts/dev/check-health.sh` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の crate 変更なし。scripts の unittest 60 件 OK）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- `git diff --name-only $CELERIS_WU_BASE -- crates web` → 空
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set G1 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0

## 未解決
- profile（TOML）の項目の schema は core WorkUnit が置く。この ADR は項目名（version・extends・required・例外・policy_paths・doc_only.paths・arch_map.path）だけを決めた。
- `policy.changed` の別 review を誰が行うか（人か、G2 の reviewer prompt か）は G2 で決める。

## 提案
- 既存の `check-health.sh` は celeris-profile WorkUnit で `repo_health.py` の呼び出しに寄せる。それまで `validate_report.py` の試験は `test_repo_health*.py` の unittest で走る（slices.json の G1 checks）。
