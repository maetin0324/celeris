---
title: 健全性v2 R5 meas-edit-a — task-api・celeris・celerisctl の build と edit→verify を同じ負荷の条件で測り直す
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
---
# 健全性v2 R5 meas-edit-a

## 結果

base（`git merge-base HEAD main`）と HEAD を同じ run で測り、`remeasure-edit-a.json` と `remeasure-edit-a-raw/` に出力した。task-api・celeris の build と edit→verify、celerisctl の build のどれにも、load の比が 2 未満で採用（included）の組がある。

- コマンド: `MEASURE_MAX_LOAD=24 MEASURE_DEADLINE=<開始 + 3000 秒> bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-a.json task-api,celeris,celerisctl`
- 負荷の上限は決定 meas-load の答え a に従い 24 にした。operator は重い task を一時停止していた。
- 実行時刻は 2026-11-12T10:20:39Z〜10:44:12Z（1413 秒）。exit 1 は celerisctl の edit→verify が 3 回とも除外されたためで、予期したとおり（下記）。
- 記録の 1 分値（全試行の各段の前後と負荷待ち、146 個）は 1.72〜26.81。26.81 は task-api の HEAD の build の直後の値で、段の開始前の値はすべて 24 以下だった（負荷待ちは最長 35 秒）。

`bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-a.json` の出力:

<!-- remeasure:begin -->
| crate | stage | attempt/pair | base s | HEAD s | base nextest s | HEAD nextest s | load ratio | result | reason |
|---|---|---|---:|---:|---:|---:|---:|---|---|
| task-api | build | 1/1 | 43.9 | 35.48 | - | - | 1.36856 | included | - |
| task-api | edit_verify | 1/1 | 99.25 | 99.16 | 90.87 | 83.67 | 1.11435 | included | - |
| celeris | build | 1/1 | 23.77 | 23.29 | - | - | 1.24246 | included | - |
| celeris | edit_verify | 1/1 | 29.21 | 40.09 | 27.7 | 24.6 | 1.05231 | included | - |
| celerisctl | build | 1/1 | 5.11 | 4.47 | - | - | 1.16431 | included | - |
| celerisctl | edit_verify | 1/1 | 4.68 | 19.52 | 4.41 | 5.36 | 1.37093 | excluded | base doc_test_exit 101 |
| celerisctl | build | 2/1 | 5.39 | 4.22 | - | - | 1.15385 | included | - |
| celerisctl | edit_verify | 2/1 | 4.57 | 19.87 | 4.3 | 5.71 | 1.17635 | excluded | base doc_test_exit 101 |
| celerisctl | build | 3/1 | 5.37 | 4.09 | - | - | 1.09044 | included | - |
| celerisctl | edit_verify | 3/1 | 4.6 | 19.33 | 4.32 | 4.87 | 1.10109 | excluded | base doc_test_exit 101 |
<!-- remeasure:end -->

build の秒は「その crate の lib と全 test target の作り直しと link」、edit→verify の秒は doc test 込み、nextest s は doc test を除く edit→verify（`edit_to_verify_secs`）。

## test binary と link の数（同じ run）

| crate | 段 | base | HEAD |
|---|---|---:|---:|
| task-api | build の test binary（link した数） | 114（114） | 65（65） |
| task-api | edit→verify の対象全体の test binary（link した数） | 334（329） | 266（261） |
| celeris | build の test binary（link した数） | 58（58） | 47（47） |
| celeris | edit→verify の対象全体の test binary（link した数） | 77（77） | 62（62） |
| celerisctl | build・edit→verify の test binary（link した数） | 19（1） | 15（1） |

## celerisctl の edit→verify（受け入れ条件 1）

- base の doc test は 3 回とも exit 101。base の `scripts/dev/test-affected.sh` は doc test を `cargo test --doc -p celerisctl` で走らせ、lib target の無い celerisctl では `error: no library targets found in package celerisctl` で失敗する（`remeasure-edit-a-raw/attempt-1-raw/base-celerisctl-doc.log`）。base で celerisctl だけを変えると、test-affected は doc test で落ちる。
- HEAD の doc test（`cargo test --doc --workspace`）は 3 回とも exit 0。
- HEAD の秒: doc test 込みの edit→verify は 19.52・19.87・19.33 秒、nextest 部分は 5.36・5.71・4.87 秒（doc test は 14.16・14.16・14.45 秒）。
- base の秒は doc test が失敗するので比べない。参考として、base の nextest 部分は 4.41・4.30・4.32 秒（doc test は 0.27〜0.28 秒で失敗）。
- この組は `measure_summary.py` の除外の規則（両側の exit 0）により、load の比（1.10〜1.37）に関係なく除外される。check は celerisctl の build だけを求める（計画 v9）。

## 読み方

- task-api の build は 43.9 → 35.48 秒、test binary は 114 → 65。edit→verify の doc test 込みの秒は 99.25 → 99.16 秒でほぼ同じ。nextest 部分は 90.87 → 83.67 秒で、HEAD の doc test（`--workspace`、15.49 秒）が base の doc test（`-p` の 7 crate、8.38 秒）より長い分が差を打ち消した。
- celeris の build は 23.77 → 23.29 秒でほぼ同じ。edit→verify の nextest 部分は 27.7 → 24.6 秒、doc test 込みは 29.21 → 40.09 秒で、HEAD の方が 10.88 秒長い。HEAD の doc test は `--workspace` で 15.48 秒、base は `-p` の 2 crate で 1.51 秒。
- HEAD の doc test の秒（14〜15.5 秒）は対象 crate に関係なくほぼ一定。affected-features が doc test を `--workspace` にした（lib の無い crate の exit 101 を直し、build の features を揃える）ことの費用で、小さい crate の edit→verify ほど比率が大きい。
- 1 回ずつの組なので、数秒の差はばらつきの範囲を超えるかどうか未確認。

## baseline（2026-11-10）との関係

baseline.md の edit→verify（task-api 155.78 秒・celeris 37.10 秒・celerisctl 5.53 秒）は負荷 41〜69 の下の HEAD 前の値で、今回の値（負荷 24 以下で開始）と条件が違う。前後比較には今回の同じ run の組を使う。

## 前の WU の試行

同じ WU の前の試行（2026-11-12 の 08〜10 時 UTC、1 分値 29〜70）は、負荷待ちの期限切れで task-api の組が 1 つも取れなかった。その出力（`remeasure-edit-a-run4.json` など）はこの branch には無く、今回の `remeasure-edit-a.json` はこの run の compare の出力だけで作った。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` → exit 0（v8 の build output 1 件）。
- 上記の compare → exit 1（1413 秒。celerisctl の edit→verify の除外による）。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-a.json` → exit 0。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require-compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-a.json task-api,celeris,celerisctl:build` → exit 0。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary --check agent-docs/progress/2026-11-10-health-v2-r5/meas-edit-a.md agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-a.json` → exit 0。

## 検査（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh` → exit 0（168 秒。docs のみの変更として試験対象なし、repo_health_gate は合格）。
- `cargo clippy --workspace -- -D warnings` → exit 0（vendored `deno_core` の deprecated 警告 1 件）。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0（変更 129 path はすべて `agent-docs/progress/2026-11-10-health-v2-r5/` の下）。

## 未解決

- 各 crate の組は 1 回ずつ。秒の差がばらつきを超えるかは未確認。
- HEAD の doc test（`--workspace`）が小さい crate の edit→verify を 14〜15 秒延ばす。affected-features の判断（lib の無い crate の exit 101 を直す・features を揃える）の費用で、meas-report が ADR D7 と付記に書く材料になる。

## 提案

- meas-report は、この表の celeris の edit→verify の doc test 込みの秒（29.21 → 40.09）と nextest 部分の秒（27.7 → 24.6）を両方載せ、差が doc test の呼び方から来ることを書く。
