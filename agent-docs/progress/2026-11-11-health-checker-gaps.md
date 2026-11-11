---
title: repo health の検査器の欠け G1・G2 を直す（policy だけの branch）
tasks: [01M76BYGKY7R8SBZ22AZGJ72SP]
status: running
updated: 2026-11-11
---
# repo health の検査器の欠け G1・G2 を直す（policy だけの branch）

設計は [ADR 2026-11-11](../adr/2026-11-11-health-checker-gaps.md)。基準は `git merge-base HEAD main`。

## 変更
- `repo_health.py`: import・実行の失敗を exit 2（理由は stderr）にした（G1）。
- `rules/existing_checks.py`: 起動の前に script の有無を確かめ、無ければ unsupported（G2）。signal で終わる検査は error（G3）。
- `test_repo_health_generic_bypass.py`: expectedFailure 2 件を外し、signal の試験を足した。

## 証拠（提出時の HEAD で実行）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` と `sh scripts/dev/tests/repo_health_gate.sh` の結果は成果物 checker-gaps-policy-diff.md に書いた。

## 未解決
- operator が policy の差分を確かめ、branch を main に no-ff で merge する（完了条件 6・7）。policy.changed は想定どおり。
