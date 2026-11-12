---
title: 健全性v2 R5 meas-edit-core — task-core の編集（全 14 crate）の build と edit→verify を base と HEAD の同じ run で測る
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# 健全性v2 R5 meas-edit-core

## 結果

- task-core の build と edit→verify の両方で、base と HEAD の組が load の比 2 未満で採用（included）になった（build 1.08、edit→verify 1.06）。受け入れ条件 0 を満たす。
- build は base 39.21 秒・HEAD 38.69 秒。doc test 込みの edit→verify は base 154.62 秒・HEAD 146.34 秒、nextest 部分は base 138.90 秒・HEAD 130.93 秒（差 7.97 秒、5.7%）。
- base の nextest 部分 138.90 秒は、B0 の主要 edit→verify の task-core（139〜144 秒）と同じ範囲に入った。
- 編集の後に link した test binary は base 378・HEAD 310（差 68）。差は R5 の束ねで減った integration test target の数と一致する（下の「作り直しと link」）。

- コマンド: `MEASURE_MAX_LOAD=24 MEASURE_DEADLINE=1794488390 bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-core.json task-core`
- 負荷の上限は決定 meas-load の答え a に従い 24 にした。
- MEASURE_DEADLINE は run の開始（2026-11-12T12:09:50Z）から 3000 秒（2026-11-12T12:59:50Z）にした。計測は期限に当たらなかった。
- 実行時刻は 2026-11-12T12:10:49Z〜12:23:17Z（記録の elapsed 745.8 秒、exit 0）。初回の試行で両方の組が採用になり、再試行は走らなかった。
- 記録の 1 分値（全体の前後、各段の前後、settle と各段の負荷待ちの値、18 個）は 1.52〜24.43。各段の開始前の値はすべて 24 以下で、負荷待ちは最長 25 秒（HEAD の edit→verify の前）だった。build の組の 1 分値の最大は base 3.57・HEAD 3.84、edit→verify の組は base 24.43・HEAD 22.95。24.43 は base の edit→verify の段の終わりの値で、段の前ではない。edit→verify の段の前後の値が高いのは、直前の settle（15〜16 crate の作り直し、138〜162 秒）と段の中の 14 crate の作り直しが上げた値。
- 段の順は base の build → HEAD の build → base の edit→verify → HEAD の edit→verify（Objective のとおり base を先にした。meas-edit-worker で直した順）。
- base は `git merge-base HEAD main`、HEAD はこの branch の HEAD（どちらも記録の git_head・merge_base_main 欄にある）。

`bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-core.json` の出力:

<!-- remeasure:begin -->
| crate | stage | attempt/pair | base s | HEAD s | base nextest s | HEAD nextest s | load ratio | result | reason |
|---|---|---|---:|---:|---:|---:|---:|---|---|
| task-core | build | 1/1 | 39.21 | 38.69 | - | - | 1.07563 | included | - |
| task-core | edit_verify | 1/1 | 154.62 | 146.34 | 138.9 | 130.93 | 1.06449 | included | - |
<!-- remeasure:end -->

build の秒は「task-core の lib と全 test target の作り直しと link」、edit→verify の秒は doc test 込み、nextest s は doc test を除く edit→verify（`edit_to_verify_secs`）。

## 作り直しと link（同じ run）

| 段 | base | HEAD |
|---|---:|---:|
| task-core の test target（cargo metadata） | 2（project_retention_scale・script_fixtures） | 2（同じ） |
| build の Compiling | 1（task-core） | 1（task-core） |
| build の test binary（link した数） | 3（3） | 3（3） |
| edit→verify の対象 crate（test-affected.sh --list） | 14 | 14 |
| edit→verify の再 build の Compiling | 14 | 14 |
| edit→verify の対象 14 crate の test binary（link した数） | 378（378） | 310（310） |
| edit→verify の再 build で link した binary（全 crate） | 378 | 310 |
| nextest が一覧にした binary | 378 | 312 |

- 対象 14 crate は両側とも celeris・celeris-campaign・celeris-credentiald・celeris-mcp・celeris-script・celerisctl・e2e・llm-proxy・routing-eval・task-api・task-core・task-dispatch・task-ops・task-worker。再 build の Compiling も両側とも同じ 14 crate。追加の検査（web の typecheck+unit、ts-verify）も両側で同じだった。
- 378 → 310 の差 68 は、R5 の束ねで integration test target が task-api 113 → 64、celeris 56 → 45、celerisctl 18 → 14、task-worker 21 → 17 に減った分（計 68）と一致する（`git ls-tree` で数えた `tests/*.rs` と `tests/*/main.rs`。どの crate の Cargo.toml にも `[[test]]` は無い）。
- HEAD の `--workspace` は全体の 312 binary を一覧にするが、編集の後に link したのは対象 14 crate の 310 binary だけだった（`linked_binaries_all` 310）。対象外の 2 binary は task-core に依存しないので作り直されない。F1（test-affected.sh が `--workspace -E <filter>` で build する呼び方）は、task-core の編集の経路で Compiling も link も増やさなかった。
- doc test: base 15.72 秒（`cargo test --doc` を 14 crate の `-p` で）、HEAD 15.41 秒（`--workspace`）。両側とも 3 件合格・2 件 ignored・Compiling 0。対象が 14 crate なので、meas-edit-a・meas-edit-worker・meas-edit-dispatch で見た「HEAD の doc test の呼び方が約 10 秒多くかかる」差はこの経路では出なかった。
- settle（編集の前に同じ組で --no-run を 1 回）の Compiling は base 16 crate（deno_core を含む）・HEAD 15 crate、秒は base 137.88・HEAD 161.93。settle の秒は edit→verify の秒に入らない。この違いの原因は調べていない。

## 読み方

- build: base 39.21 秒・HEAD 38.69 秒で差は 0.52 秒。Compiling と link した test binary の数は両側で同じ。R5 は task-core の test target を変えていないので、差が無いのは予想どおり。
- nextest 部分の edit→verify: base 138.90 秒・HEAD 130.93 秒で、HEAD が 7.97 秒短い。link した test binary は 378 → 310。1 組なので、8 秒の差がばらつきを超えるかは未確認。
- doc test 込みの edit→verify: base 154.62 秒・HEAD 146.34 秒で、HEAD が 8.28 秒短い。
- B0 の task-core の edit→verify（139〜144 秒）は B0 の計測（別の日・別の負荷・別の settle）の値で、今回の組と同じ条件ではない。今回の base の 138.90 秒がその範囲に入ったことだけを書く。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` → exit 0（v8 の build output 1 件）。計測の前に実行した。
- 上記の compare → exit 0（746 秒、1 試行）。commit は「phase 1: meas-edit-core: task-core の base と HEAD の build と edit→verify を負荷上限 24 の同じ run で測る（両方の組を採用）」。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-core.json && bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require-compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-core.json task-core` → exit 0。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary --check agent-docs/progress/2026-11-10-health-v2-r5/meas-edit-core.md agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-core.json` → exit 0。

## 検査（提出時の HEAD で実行。WU の基準は `$CELERIS_WU_BASE`）

- 下の「検査の結果」に書く。

## 未解決

- 無し。

## 提案

1. 無し（measure.sh・検査器・baseline は変えていない）。
