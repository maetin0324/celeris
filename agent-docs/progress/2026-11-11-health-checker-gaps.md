---
title: repo health の検査器の欠け G1・G2 を直す（policy だけの branch）
tasks: [01M76BYGKY7R8SBZ22AZGJ72SP]
status: running
updated: 2026-11-12
---
# repo health の検査器の欠け G1・G2 を直す（policy だけの branch）

設計は [ADR 2026-11-11](../adr/2026-11-11-health-checker-gaps.md)。基準は `git merge-base HEAD main`。

## 変更
- `repo_health.py`: import・実行の失敗を exit 2（理由は stderr）にした（G1）。
- `rules/existing_checks.py`: 起動の前に `_preflight` で tool と script の有無を確かめ、無ければ unsupported（G2）。signal で終わる検査は error（G3）。
- `test_repo_health_generic_bypass.py`: expectedFailure 2 件を外し、signal の試験を足した。
- attempt 2: operator の差分確認（承認 task 01M76CCPPK88BRZCXMZPVKW3R6）で不合格。最初の版の script の判定（`scripts/` で始まる、`.sh`・`.py` で終わる引数）が Celeris の source-size の `python3 -m unittest discover -s scripts/tests ...` の `scripts/tests` を script とみなし、source-size を unsupported（exit 2）にした。script を interpreter（sh・bash・dash・python・python3）の option の後の最初の引数だけに改め、`-c`・`-m`・`-` は script 無しにした（ADR の G2）。
- attempt 2 の repair（reviewer の不合格）: `repo_health.py` の先頭の import が ImportError だけを捕まえていたので、`health/engine.py` の構文誤りと `health/snapshot.py` の import 時の例外が traceback の exit 1 になっていた。`except Exception` にして、どの import の失敗も `checker_import_failed` の exit 2 にした。`test_generic_checker_import_failure_is_error` に、遅れて import される scan.py の欠け・先頭の engine.py の欠け・engine.py の構文誤り・snapshot.py の import 時の例外の 4 つを置き、exit 2・report 無し・traceback 無しを確かめる（`except ImportError` に戻すと構文誤りと例外の 2 件が落ちることを手元で確かめた）。ADR の G1 に import の失敗の範囲を書き足した。試験の注釈の「checker gap G1・G2」と G4 の進捗の「未解決」を、直した ADR を指す文に直した。
- `test_repo_health_wrapped_checks.py`（新規）: 判定の表、Celeris の profile の全 command が `_preflight` を通ること、module の実行が起動して pass になること、無い python の script が unsupported になること。最初の版の判定では 4 試験とも落ちる（手元で旧い判定に戻して確かめた）。

## 証拠（提出時の HEAD で実行）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'`: exit 0、99 tests OK。expectedFailure は 0 件。
- `sh scripts/dev/tests/repo_health_gate.sh`: exit 0、(1)〜(7) OK。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
- 完了条件 5 の `repo_health.py --base $(git merge-base HEAD main) --head HEAD`: 必須 rule は policy.changed を除いて全部 pass（source-size も pass）。検査器の exit は 1（policy.changed だけ）。
- `bash scripts/dev/check-health.sh`: exit 1（policy.changed だけ fail。policy の file と試験・文書が混ざった変更なので policy review の exit 0 にはならない）。attempt 1 は source-size の unsupported で exit 2 だった。
- 直す前と後の exit code・report の抜粋と差分は成果物 checker-gaps-policy-diff.md に書いた。

## 未解決
- operator が policy の差分を確かめ、branch を main に no-ff で merge する（完了条件 6・7・8）。policy.changed の fail と check-health の exit 1（policy の file と試験・文書の混ざった変更）は想定どおり。
