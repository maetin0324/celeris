---
title: 健全性v2 R5 meas-edit-dispatch — task-dispatch の base と HEAD の build と edit→verify を同じ run で測る
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# 健全性v2 R5 meas-edit-dispatch

## 結果

- task-dispatch の build と edit→verify の両方で、base と HEAD の組が load の比 2 未満で採用（included）になった（build 1.08、edit→verify 1.30）。受け入れ条件 0 を満たす。
- build は base 58.76 秒・HEAD 57.87 秒。doc test 込みの edit→verify は base 88.76 秒・HEAD 98.70 秒、nextest 部分は base 83.72 秒・HEAD 83.03 秒。
- F1（test-affected.sh が `--workspace -E <filter>` で build する呼び方）は、task-dispatch の編集の経路で Compiling も link も増やさなかった。再 build の Compiling は両側とも同じ 5 crate で、HEAD が link した test binary は対象 5 crate の 180 個だけだった（下の「F1 と作り直し・link」）。

- コマンド: `MEASURE_MAX_LOAD=24 MEASURE_DEADLINE=1794487190 bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-dispatch.json task-dispatch`
- 負荷の上限は決定 meas-load の答え a に従い 24 にした。
- MEASURE_DEADLINE は run の開始から約 3000 秒（2026-11-12T12:39:50Z。compare の開始から 2875 秒）にした。計測は期限に当たらなかった。
- 実行時刻は 2026-11-12T11:51:55Z〜12:02:40Z（記録の elapsed 643.6 秒、exit 0）。初回の試行で両方の組が採用になり、再試行は走らなかった。
- 記録の 1 分値（全体の前後、各段の前後、settle と各段の負荷待ちの値）は 1.59〜23.07。負荷待ちはすべて 0 秒で ready だった。build の組の 1 分値の最大は base 3.58・HEAD 3.33、edit→verify の組は base 17.77・HEAD 23.07。edit→verify の段の前の値が高いのは、直前の settle（15 crate の作り直し、106〜119 秒）が上げた値。
- 段の順は meas-edit-worker で直した順（base の build → HEAD の build → base の edit→verify → HEAD の edit→verify）。
- base は `git merge-base HEAD main`、HEAD はこの branch の HEAD（どちらも記録の git_head・merge_base_main 欄にある）。

`bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-dispatch.json` の出力:

<!-- remeasure:begin -->
| crate | stage | attempt/pair | base s | HEAD s | base nextest s | HEAD nextest s | load ratio | result | reason |
|---|---|---|---:|---:|---:|---:|---:|---|---|
| task-dispatch | build | 1/1 | 58.76 | 57.87 | - | - | 1.07508 | included | - |
| task-dispatch | edit_verify | 1/1 | 88.76 | 98.7 | 83.72 | 83.03 | 1.29826 | included | - |
<!-- remeasure:end -->

build の秒は「task-dispatch の lib と全 test target の作り直しと link」、edit→verify の秒は doc test 込み、nextest s は doc test を除く edit→verify（`edit_to_verify_secs`）。

## F1 と作り直し・link（同じ run）

| 段 | base | HEAD |
|---|---:|---:|
| task-dispatch の test target（cargo metadata） | 1（policy_close） | 1（policy_close） |
| build の Compiling | 2（task-dispatch・celeris-campaign） | 2（同じ） |
| build の test binary（link した数） | 2（2） | 2（2） |
| edit→verify の対象 crate（test-affected.sh --list） | 5 | 5 |
| edit→verify の再 build の Compiling | 5 | 5 |
| edit→verify の対象 5 crate の test binary（link した数） | 195（195） | 180（180） |
| edit→verify の再 build で link した binary（全 crate） | 195 | 180 |
| nextest が一覧にした binary | 195 | 312 |

- 対象 5 crate は両側とも celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。再 build の Compiling も両側とも同じ 5 crate（task-dispatch・celeris-campaign・llm-proxy・celeris・celerisctl）。
- HEAD の `--workspace` は全体の 312 binary を一覧にするが、編集の後に link したのは対象 5 crate の 180 binary だけだった（`linked_binaries_all` 180）。対象外の crate は task-dispatch に依存しないので、作り直しも link もされない。F1 はこの経路の link を増やしていない。
- 195 → 180 の差 15 は、R5 の束ねで celeris の integration test target が 56 → 45、celerisctl が 18 → 14 に減った分と一致する（`git ls-tree` で数えた `tests/*.rs` と `tests/*/main.rs`。どの crate の Cargo.toml にも `[[test]]` は無い）。
- settle（編集の前に同じ組で --no-run を 1 回）の Compiling は両側とも 15 crate（base は deno_core を含み e2e を含まない、HEAD は e2e を含み deno_core を含まない）。settle の秒（base 119.03・HEAD 106.33）は edit→verify の秒に入らない。この違いの原因は調べていない。

## 読み方

- build: base 58.76 秒・HEAD 57.87 秒で差は 0.89 秒。Compiling と link した test binary の数は両側で同じ。R5 は task-dispatch の test target を変えていないので、差が無いのは予想どおり。1 組なので、1 秒程度の差がばらつきを超えるかは未確認。
- nextest 部分の edit→verify: base 83.72 秒・HEAD 83.03 秒で差は 0.69 秒。link した test binary は 195 → 180。
- doc test: base 5.03 秒（`cargo test --doc` を 5 crate の `-p` で、doc test 0 件）、HEAD 15.67 秒（`--workspace`、3 件合格・2 件 ignored）。HEAD の doc test の呼び方が 10.64 秒多くかかり、doc test 込みの edit→verify は HEAD が 9.94 秒長い。meas-edit-a・meas-edit-worker と同じ傾向。
- B0 の task-dispatch の再 build（R1〜R3 で 79.7〜94.9 秒、R3 でも 8 crate を Compiling）は、B0 の計測（別の日・別の負荷・別の settle）の値で、今回の組と同じ条件ではない。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` → exit 0（v8 の build output 1 件）。計測の前に実行した。
- 上記の compare → exit 0（645 秒、1 試行）。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-dispatch.json && bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require-compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-dispatch.json task-dispatch` → exit 0（計画の check と同じコマンド）。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary --check agent-docs/progress/2026-11-10-health-v2-r5/meas-edit-dispatch.md agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-dispatch.json` → 下の検査に結果を書く。

## 検査（提出時の HEAD で実行。WU の基準は `$CELERIS_WU_BASE`）

- 検査の結果は commit「phase 1: meas-edit-dispatch: 検査の結果を進捗に書く」で書く。

## 未解決

- 無し。

## 提案

1. 無し（measure.sh・検査器・baseline は変えていない）。
