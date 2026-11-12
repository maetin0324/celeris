---
title: 健全性v2 R5 meas-edit-worker — task-worker の base と HEAD の build と edit→verify を同じ run で測る
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# 健全性v2 R5 meas-edit-worker

## 結果

- task-worker の build と edit→verify の両方で、base と HEAD の組が load の比 2 未満で採用（included）になった（build 1.29、edit→verify 1.05）。受け入れ条件 0 を満たす。
- build は base 37.31 秒・HEAD 36.12 秒、doc test 込みの edit→verify は base 117.45 秒・HEAD 109.78 秒（nextest 部分は 108.77 秒・94.11 秒）。
- 前回の HEAD の 190.58 秒（measure-build、1 分値 23〜84）は、同じ run・同じ負荷の条件では再現しなかった。HEAD は base より 7.67 秒短い。

- コマンド: `MEASURE_MAX_LOAD=24 MEASURE_DEADLINE=<開始 + 3000 秒> bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json task-worker`
- 負荷の上限は決定 meas-load の答え a に従い 24 にした。
- 実行時刻は 2026-11-12T11:34:30Z〜11:45:33Z（663 秒、exit 0）。初回の試行で両方の組が採用になり、再試行は走らなかった。
- 記録の 1 分値（開始・終了と各段の前後、18 個）は 0.56〜23.65。各段の開始前の値はすべて 24 以下で、負荷待ちは最長 20 秒だった。23 前後の値は edit→verify の段の前後で、その段の直前の settle（7 crate の作り直し）が上げた値。両側とも同じ順なので、比は 1.05 にとどまった。
- base は `git merge-base HEAD main`、HEAD はこの branch の HEAD（どちらも記録の git_head・merge_base_main 欄にある）。

`bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json` の出力:

<!-- remeasure:begin -->
| crate | stage | attempt/pair | base s | HEAD s | base nextest s | HEAD nextest s | load ratio | result | reason |
|---|---|---|---:|---:|---:|---:|---:|---|---|
| task-worker | build | 1/1 | 37.31 | 36.12 | - | - | 1.28947 | included | - |
| task-worker | edit_verify | 1/1 | 117.45 | 109.78 | 108.77 | 94.11 | 1.04692 | included | - |
<!-- remeasure:end -->

build の秒は「task-worker の lib と全 test target の作り直しと link」、edit→verify の秒は doc test 込み、nextest s は doc test を除く edit→verify（`edit_to_verify_secs`）。

## 1 回目の計測で build の組が除外された理由と直し

この WU の 1 回目の run（2026-11-12T10:51:20Z〜11:24:41Z、3 試行）では、build の 3 組がすべて load の比 2 以上（2.93〜6.52）で除外され、計画の check（`--require-compare … task-worker`）が exit 1 になった。

- 原因は compare の段の順だった。1 つの crate について base の build → base の edit→verify → HEAD の build → HEAD の edit→verify の順で測っていた。
- HEAD の build の直前の settle は 1.2 秒（1 unit）で終わるので、HEAD の build は base の edit→verify（settle 100〜164 秒と作り直し）の直後に 1 分値 21〜22 で始まった。base の build は settle 77〜83 秒の後に 1 分値 2.67〜7.32 で始まった。
- 直し: commit「phase 1: meas-edit-worker: measure.sh の compare を crate ごとに両側の build → 両側の edit→verify の順にする（build の組が自分の計測の負荷で除外されないように）」で、`measure_one` を `measure_build` と `measure_ev` に分け、crate ごとに両側の build を先に、両側の edit→verify を後に測る順にした。側の順（order 欄）、除外の規則（両側の exit が 0、1 分値の最大の比が 2 未満、負荷待ちが ready、秒が有限で 0 以上）、MEASURE_MAX_LOAD・MEASURE_LOAD_WAIT・MEASURE_RETRIES は変えていない。
- 直しの後に、片側の負荷待ちが切れた時は、その側の build だけを skipped にし、edit→verify は自分の段の負荷待ちで判定する（直す前は、build の 2 回目の負荷待ちが切れると edit→verify を skipped に書かずに飛ばしていた）。
- meas-edit-a の記録（remeasure-edit-a.json）は直す前の順で測った。meas-edit-a の組は採用・除外の判定のとおりで、測り直していない。
- 1 回目の記録（3 試行の表と負荷の値）は commit「phase 1: meas-edit-worker: task-worker の base と HEAD の build と edit→verify を負荷上限 24 の同じ run で測る（build の組は負荷の比で全部除外）」の remeasure-edit-worker.json と、この file のその commit の版にある。1 回目の edit→verify の 3 組（採用）は base 109.12〜117.35 秒、HEAD 108.74〜117.35 秒で、今回の組と同じ範囲だった。

## test binary と link の数（同じ run）

| 段 | base | HEAD |
|---|---:|---:|
| task-worker の test target（cargo metadata） | 21 | 17 |
| build の test binary（link した数） | 25（25） | 21（21） |
| edit→verify の対象 7 crate の test binary（link した数） | 334（334） | 266（266） |

edit→verify の対象は両側とも test-affected.sh --list の 7 crate（celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）。HEAD の --workspace の一覧は全体で 312 binary で、うち対象 7 crate は 266。

## 読み方

- build: base 37.31 秒・HEAD 36.12 秒で差は 1.19 秒。link した test binary は 25 → 21。1 組なので、1 秒程度の差がばらつきを超えるかは未確認。
- edit→verify（doc test 込み）: base 117.45 秒・HEAD 109.78 秒で、HEAD が 7.67 秒短い。
- nextest 部分: base 108.77 秒・HEAD 94.11 秒で、HEAD が 14.66 秒短い。link した test binary は 334 → 266。
- doc test: base 8.68 秒（`cargo test --doc` を 7 crate の `-p` で、doc test 0 件）、HEAD 15.66 秒（`--workspace`、doc test 3 件）。HEAD の doc test の呼び方の費用が nextest 部分の短縮の半分を打ち消す。meas-edit-a と同じ傾向。
- B0 の task-worker の 98〜136 秒は B0 の計測（別の日・別の負荷・B0 の台本）の値で、今回の組と同じ条件ではない。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` → exit 0（v8 の build output 1 件）。
- `python3 -I agent-docs/progress/2026-11-10-health-v2-r5/test_measure_summary.py` → exit 0（17 件 OK）。
- 上記の compare → exit 0（663 秒、1 試行）。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json && bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require-compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json task-worker` → exit 0（計画の check と同じコマンド）。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary --check agent-docs/progress/2026-11-10-health-v2-r5/meas-edit-worker.md agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json` → 下の「検査」に結果を書く。

## 検査

（提出前に書く）

## 未解決

- 無し。

## 提案

1. meas-edit-dispatch・meas-edit-core は直した後の measure.sh で測る。meas-edit-a の task-api・celeris・celerisctl の組を直した順で測り直すかは、meas-report で判断する（この WU では測り直していない）。
