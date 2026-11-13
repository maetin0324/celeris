---
title: 健全性v2 R5 measure-build — base と HEAD の test build/link と edit→verify を同じ run で測る
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 measure-build

完了日: 2026-11-11

WorkUnit `measure-build`。base は `git merge-base HEAD main`（R2〜R4 の入った main で、R5 の変更を含まない）、HEAD はこの WU の base の commit（段 final の統合の後）。試験は走らせていない（2 回の実行は measure-run が行う）。

## 結論
- 4 crate とも test binary の数が減った: task-api 114 → 65、celeris 57 → 46、celerisctl 19 → 15、task-worker 25 → 21（`-p <crate>` の nextest の binary 一覧）。
- crate の lib を作り直した時に link する test binary も同じ数だけ減った（下の表の「build link」）。task-api の編集で対象 7 crate が link し直す binary は 328 → 260。
- edit→verify の秒は task-api 102.87 → 98.39、celeris 65.26 → 30.98、celerisctl 11.48 → 8.82。task-worker の base は締め切りで測れなかった（HEAD は 190.58）。load average（1 分値）は 23〜75 の間で動き、側ごとに違う（表の右端）。秒の差を全部変更の効果とは読まない。数の差（binary・link・Compiling）は負荷に依らない。
- HEAD の edit→verify は HEAD の test-affected.sh の呼び方（`cargo nextest run --workspace -E 'package(=…) | …'`）で測った。base は base の呼び方（`-p` の組）。

## 計測の方法（`measure.sh compare`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh compare <out.json> <crate,...>` を足した。`--validate` は compare の出力（schema `health-v2-r5-build-compare/1`）も検証する。run の出力（baseline.json・measure-run の after-run.json）の検証は変えていない。
- base の tree は `git worktree add --detach $TMPDIR/r5-measure-base.* <merge-base>` で作り、終わりに `git worktree remove --force` で消した（`after-build-raw/worktree.log` の末尾 `worktree remove: exit 0`）。CARGO_TARGET_DIR は渡された値のまま両側で共有した。
- 順は crate ごとに入れ替えた: task-api（base → HEAD）、celeris（HEAD → base）、celerisctl（base → HEAD）、task-worker（HEAD → base）。
- crate ごと・側ごとの段:
  1. `targets`: `cargo metadata --no-deps` の kind ごとの target の数（build しない）。
  2. `build`: 両側とも `cargo nextest run -p <crate> --no-run` で settle した後、`src/lib.rs`（celerisctl は `src/main.rs`）の mtime を進め、同じ build を測る。その crate の lib と全 test target の作り直し・link の秒になる。baseline.json の build（target の状態を揃えない初回の build）とは方法が違う。
  3. `edit_verify`: baseline と同じく、編集で決まる対象 crate を `--list` で求め、その側の呼び方で settle してから、末尾に 1 行足す → `CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list` → その側の test-affected.sh の nextest の行に `--no-run` を足して実行。nextest の行と引数を作る 2 行は、その側の test-affected.sh から取り出して `eval` する（呼び方の写しを持たない）。edit→verify は `--list` と再 build の秒の和（B0・baseline と同じ定義）。続けてその側の doc test の行も 1 回走らせ、秒と件数を別の欄に書いた。
- 生の出力: `after-build-raw/`（各段の log・binary 一覧・`steps.jsonl`・取り出した呼び方 `inv-*`）。

## 環境
- host: 24 CPU、Linux 6.8.12-9-pve、cargo 1.98.1、cargo-nextest 0.9.146。target は WU の scratch（開始時 1.3G。依存の多くは未 build）。
- load average（1 分・5 分・15 分）: 開始 5.41 10.52 19.94、終了 38.98 48.94 50.09。全体 2133 秒。自分の build を含み、他の process の負荷と分けていない。並走の cargo は走らせていない。

## 結果（前後の表）
| crate | 側 | test target | test binary | build link | build --no-run 秒 | build Compiling | 対象 crate | 再 build で link / 対象 crate の binary | --list 秒 | 再 build 秒 | edit→verify 秒 | 再 build Compiling | load 1 分（build 前 → edit 後） |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| task-api | base | 113 | 114 | 114 | 46.60 | 1 | 7 | 328 / 333 | 0.11 | 102.76 | 102.87 | 5 | 23.43 → 42.15 |
| task-api | HEAD | 64 | 65 | 65 | 38.39 | 1 | 7 | 260 / 265 | 0.13 | 98.26 | 98.39 | 5 | 39.81 → 45.07 |
| celeris | base | 55 | 57 | 57 | 54.94 | 1 | 2 | 76 / 76 | 0.11 | 65.15 | 65.26 | 2 | 35.02 → 73.81 |
| celeris | HEAD | 44 | 46 | 46 | 26.49 | 1 | 2 | 61 / 61 | 0.13 | 30.85 | 30.98 | 2 | 27.39 → 31.89 |
| celerisctl | base | 18 | 19 | 1 | 9.83 | 1 | 1 | 1 / 19 | 1.29 | 10.20 | 11.48 | 1 | 67.50 → 71.45 |
| celerisctl | HEAD | 14 | 15 | 1 | 10.09 | 1 | 1 | 1 / 15 | 0.28 | 8.54 | 8.82 | 1 | 74.93 → 67.84 |
| task-worker | base | 21 | 25 | 25 | 41.34 | 1 | 7 | 未計測 | — | 未計測 | 未計測 | — | 53.59 → 38.98 |
| task-worker | HEAD | 17 | 21 | 21 | 52.86 | 1 | 7 | 265 / 265 | 1.42 | 189.16 | 190.58 | 7 | 67.40 → 57.21 |

- test target は `cargo metadata` の kind `test` の数。test binary は nextest の一覧の数で、lib・bin の unit test を含む。
- build の Compiling は 4 crate・両側とも 1（その crate だけ）。link はその crate の全 test binary（celerisctl は lib が無く、mtime を進めた `main.rs` を使う bin の 1 つだけ）。
- 再 build の Compiling は両側で同じ crate だった: task-api の編集で task-api・celeris・celeris-campaign・task-worker・celerisctl、celeris の編集で celeris・celerisctl、celerisctl の編集で celerisctl、task-worker の編集（HEAD）で task-worker・task-dispatch・task-api・celeris-campaign・llm-proxy・celeris・celerisctl。
- HEAD の再 build は `--workspace` で build するので、nextest の一覧は workspace の 311 binary を持つ。編集で link し直したのは対象 crate の binary だけだった（全体の link の数も対象 crate の数と同じ: 260・61・1・265）。ADR D6-4 の「対象外の crate の test binary も build・link する費用」は、target に `--workspace` の版がある時には 0 だった。
- settle（編集の前の build）の Compiling: base task-api 15・celeris 2・celerisctl 1・task-worker 7、HEAD task-api 13・celeris 5・celerisctl 2・task-worker 7。HEAD は `-p <crate>` の build の後に `--workspace` の版へ移るので、build の段の mtime の分を作り直す。base の `-p` の組も同じ。
- task-worker の base の edit→verify は、`MEASURE_DEADLINE`（計測の開始から 2050 秒）までの残りが 30 秒未満になり、再 build を走らせなかった（`rebuild_exit` 125）。`after-build.json` の `skipped` に理由を書いた。settle（125 秒・Compiling 7）は済んでいた。台本は、この場合を skip として書くように直した。

## doc test（ADR D6-3、edit→verify の秒には入れない）
| crate | 側 | doc test の呼び方 | 秒 | passed / ignored | exit |
|---|---|---|---|---|---|
| task-api | base | `cargo test --doc -p …`（7 crate） | 8.85 | 0 / 1 | 0 |
| task-api | HEAD | `cargo test --doc --workspace` | 17.38 | 3 / 2 | 0 |
| celeris | base | `-p celeris -p celerisctl` | 4.43 | 0 / 0 | 0 |
| celeris | HEAD | `--workspace` | 20.45 | 3 / 2 | 0 |
| celerisctl | base | `-p celerisctl` | 2.13 | 0 / 0 | 101（`no library targets found in package celerisctl`） |
| celerisctl | HEAD | `--workspace` | 25.00 | 3 / 2 | 0 |
| task-worker | HEAD | `--workspace` | 21.40 | 3 / 2 | 0 |

- HEAD の doc test は対象 crate に依らず全 crate の 3 件と ignore 2 件を走らせ、17〜25 秒かかる。base は対象 crate の分だけで、celerisctl だけの組では lib が無いので exit 101 になる（base の test-affected.sh で celerisctl だけを変えると doc test で落ちる）。
- doc test の秒の計測の後に binary の一覧を取る順に直した。今回の計測は一覧の後に doc test を測ったので、秒には `cargo nextest list` の数秒が入る。

## 証拠コマンド（提出時の HEAD で実行）
- `MEASURE_DEADLINE=$(( $(date +%s) + 2050 )) bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh compare agent-docs/progress/2026-11-10-health-v2-r5/after-build.json task-api,celeris,celerisctl,task-worker` → exit 0、2133 秒、`ok: compare 4 crates (both sides built: 4), 0 skipped item(s)`。その後に task-worker の base の edit→verify を skip に置き換えた（上の節）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/after-build.json` → exit 0、`ok: compare 4 crates (both sides built: 4), 1 skipped item(s)`
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/baseline.json` → exit 0、`ok: 3 crates, 0 skipped item(s)`（run の検証は変えていない）
- `MEASURE_DEADLINE=$(date +%s) bash …/measure.sh compare $TMPDIR/dry/after-build.json celerisctl` → 全段が skip になり、`--validate` が `no crate has build values on both base and head` で exit 1（測れなかった記録を通さないこと）。worktree は消えた
- 範囲の check・slices-check・progress-index の結果は WU の報告に書く

## commit
- R5 measure-build: base と HEAD を交互に測る compare を measure.sh に足し、前後の build/link と edit→verify を記録する

## 未解決事項
- task-worker の base の edit→verify は未計測。base の test-affected と同じ組（7 crate）の再 build は、task-api の編集の base（7 crate、102.76 秒）に近いと見込むが、未確認。
- 秒は load average 23〜75 の間で測り、側ごとに負荷が違う。celeris の base（35 → 74）と HEAD（27 → 32）の差は負荷の差を含む。

## 提案
1. measure-run は after-run.json を `measure.sh run` で書く（`--validate` は run の形を今のとおり検証する）。
2. HEAD の doc test は対象 crate に依らず 17〜25 秒かかる。edit→verify の短縮を詰めるなら、lib の無い crate だけを除くのではなく、doc test を持つ crate（task-core・celeris-script・task-worker）の逆依存に入る時だけ走らせる案を ADR で検討する（unit `record` が提案に写す）。
