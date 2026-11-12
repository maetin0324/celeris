---
tasks: [01M76BYGKY7R8SBZ22AZGJ72SP]
---
# ADR 2026-11-11: repo health の検査器の欠け G1・G2 を直し、signal で終わる包んだ検査を error にする

状態: 実装済み

- 日付: 2026-11-11（G2 の script の判定を 2026-11-12 に改めた。下の「G2 の最初の判定を退けた理由」）
- 付記先: [ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md)、[ADR 2026-11-10 isolated Git fixture](2026-11-10-health-generic-fixtures.md)（(2) unsupported-tool、(3) 表 (d)）。G4 の人の決定 checker-gaps の回答 a（G4 では直さず、policy だけの別 task で直す）に従う。
- 実装: `scripts/dev/repo-health/repo_health.py`、`scripts/dev/repo-health/rules/existing_checks.py`、試験は `scripts/tests/test_repo_health_generic_bypass.py`（G1・G2・G3）と `scripts/tests/test_repo_health_wrapped_checks.py`（G2 の script の判定と Celeris の profile）。

## 決定
- G1: 検査器自身の module を import できない、または実行中に想定外の例外が出たときは、`repo_health.py` が理由を stderr に出して exit 2 にする（report は書かない）。契約では exit 1 は必須 rule の fail だけで、検査器の故障は error。traceback の exit 1 は fail に見えるので廃止する。
- G2: 包んだ検査は、起動の前に `_preflight` で 2 つを確かめ、外れたら `unsupported` にして finding を作らない。shell ごとの exit code（dash は 2、python3 も 2）に依らない。
  - argv[0] が PATH に無い: reason `tool_missing:<argv[0]>`（以前からの判定）。
  - command の script が head の tree に file として無い: reason `command_missing: script <path> が head の tree に無い`。
  - script とみなすのは、argv[0] の basename が interpreter（`sh`・`bash`・`dash`・`python`・`python3`・`python3.N`）のときの、option の後の最初の引数だけ。`-c`（文字列）・`-m`（module）・`-`（標準入力）を含む command には script が無い。`--` の次の引数は script。値を取る option（shell の `-o`、python の `-W`・`-X`・`--check-hash-based-pycs`）はその値を飛ばす。interpreter でない tool（例 `orchard-adr-lint`・`cargo`）の引数は script とみなさない。
  - script の無い command（module の実行など）は起動し、その exit で判定する。対象（試験の dir・module）が無ければ command 自身が非 0 で終わるので fail で、pass にはならない。
  - 起動した後の exit 126・127 は今までどおり `unsupported`（command_missing）。script は在るが、その中で起動できない物がある場合で、起動前の確かめとは別の事象。
  - fail の finding の evidence の代わりの path（出力に `path:line:` が無いとき）も同じ判定の script を使い、script が無ければ argv[0] にする。script の判定は `_script_of` の 1 か所だけにある。
- G3: 包んだ検査が signal で終わった（returncode < 0）ときは、finding にせず rule を `error`（reason `crashed:<rule>:CommandKilled`）にする。signal は検査対象の repo の欠陥を示さず、fail にすると repair へ「直す finding」として渡るため。分けても exit は 2 のままで、必須 rule は pass にならない。

## G2 の最初の判定を退けた理由
最初の版は「`scripts/` で始まる、または `.sh`・`.py` で終わる引数」を script とした。Celeris の profile の source-size の 2 つ目の command `python3 -m unittest discover -s scripts/tests -p test_source_size*.py` では `scripts/tests`（dir）を script とみなして `unsupported` にし、main に入れると Celeris のすべての task の健全性検査が exit 2 になる（operator の差分確認、承認 task 01M76CCPPK88BRZCXMZPVKW3R6 で不合格）。interpreter の最初の引数だけを見る今の判定では、module の実行・他の tool の path 風の引数を script にしない。`test_repo_health_wrapped_checks.py` が、判定の表、Celeris の profile の全 command が `_preflight` を通ること（script の候補が tree に在り unsupported にならない）、module の実行が起動して pass になることを確かめる。最初の版ではこの 3 つとも落ちる。

## fail-closed の確認
- G1・G2・G3 のどれも、必須 rule を pass・not_applicable に変えない。候補が script を消す・検査を殺しても、exit は 2（以前は G3 が 1）で pass にならない（`test_generic_unsupported_required_not_pass`、`test_generic_wrapped_check_killed_by_signal_is_error`、`test_missing_python_script_is_unsupported`）。
- G2 の script の判定は起動の前の分類だけを変える。判定が script を見落としても command は起動し、無い script は非 0（fail か exit 126・127 の unsupported）で終わる。判定が在る file を誤って無いとみなしても unsupported（exit 2）で、どちらも pass にならない。command は profile（policy の file、base 側で評価）にあり、候補は書き換えられない。
- 検査の判定を弱める policy の経路（baseline・exception・threshold）は変えない。

## 撤去
- 試験の `@unittest.expectedFailure` 2 件と「Remove expectedFailure when …」の文を外した。旧い traceback の経路（import 失敗時の exit 1）と、dash の exit 2 を fail に写す経路は残っていない。fail の evidence の path を決めていた別の判定（`scripts/` で始まる、`.sh`・`.py` で終わる最初の引数）は消し、`_script_of` にまとめた。
