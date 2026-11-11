---
tasks: [01M76BYGKY7R8SBZ22AZGJ72SP]
---
# ADR 2026-11-11: repo health の検査器の欠け G1・G2 を直し、signal で終わる包んだ検査を error にする

状態: 実装済み

- 日付: 2026-11-11
- 付記先: [ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md)、[ADR 2026-11-10 isolated Git fixture](2026-11-10-health-generic-fixtures.md)（(2) unsupported-tool、(3) 表 (d)）。G4 の人の決定 checker-gaps の回答 a（G4 では直さず、policy だけの別 task で直す）に従う。
- 実装: `scripts/dev/repo-health/repo_health.py`、`scripts/dev/repo-health/rules/existing_checks.py`、試験は `scripts/tests/test_repo_health_generic_bypass.py`。

## 決定
- G1: 検査器自身の module を import できない、または実行中に想定外の例外が出たときは、`repo_health.py` が理由を stderr に出して exit 2 にする（report は書かない）。契約では exit 1 は必須 rule の fail だけで、検査器の故障は error。traceback の exit 1 は fail に見えるので廃止する。
- G2: 包んだ検査は、起動の前に command の中の script（`scripts/` で始まる、または `.sh`・`.py` で終わる引数）が head の tree に file として在るかを確かめ、無ければ `unsupported`（reason は `command_missing:` で始まる）にする。finding は作らない。shell ごとの exit code（dash は 2）に依らない。旧い exit 126・127 の判定は、script が在って起動できない場合のために残す（別の事象）。
- G3: 包んだ検査が signal で終わった（returncode < 0）ときは、finding にせず rule を `error`（reason `crashed:<rule>:CommandKilled`）にする。signal は検査対象の repo の欠陥を示さず、fail にすると repair へ「直す finding」として渡るため。分けても exit は 2 のままで、必須 rule は pass にならない。

## fail-closed の確認
- G1・G2・G3 のどれも、必須 rule を pass・not_applicable に変えない。候補が script を消す・検査を殺しても、exit は 2（以前は G3 が 1）で pass にならない（`test_generic_unsupported_required_not_pass`、`test_generic_wrapped_check_killed_by_signal_is_error`）。
- 検査の判定を弱める policy の経路（baseline・exception・threshold）は変えない。

## 撤去
- 試験の `@unittest.expectedFailure` 2 件と「Remove expectedFailure when …」の文を外した。旧い traceback の経路（import 失敗時の exit 1）と、dash の exit 2 を fail に写す経路は残っていない。
