---
title: 健全性v2 V0 targets — test target 数と edit→verify の秒の B0 との比較
tasks: [01M6TYVJT97XH88TEEAVRD7JK6]
status: done
updated: 2026-11-16
completed: 2026-11-16
---

# 健全性v2 V0 targets

## 結論
- workspace の Rust の test target は B0 の 352 から HEAD で 287 になった（−65）。予測 test binary は 379 → 314（−65）。減ったのは R5 の束ね（task-api・celeris・celerisctl・task-worker）だけで、ほかの 11 crate は B0 と同じ数。
- 5 crate の test binary の実数（`cargo nextest list --list-type binaries-only`）は予測と全部一致した: task-core 3・task-dispatch 2・task-api 66・task-worker 21・celeris 47。
- edit→verify（lib.rs に 1 行 → `test-affected.sh --list` → 対象 crate の `nextest --no-run`）の settle 後の値（R3）は、B0 と比べて task-core・task-dispatch・task-worker・celeris が同じか長く、task-api だけ 5.7 秒短い。今回の load は B0 の R3 と同程度（1 分値 16〜36）で、他の run を分けていないため、10 秒程度の差は誤差の範囲として読む。
- B0 の measure script は `-p` で crate を選んで build する。R5 が test-affected.sh に入れた `--workspace` の features での build はこの手順に入っていない。このため、R5 の束ねは target 数と link する binary 数を減らしたが、この手順の秒には「組が変わると共有 crate を作り直す」交絡が B0 と同じく残る（celeris の R2・R3 は 12 crate をコンパイルした）。

## 基準と環境
- 計測の HEAD: WU の base と同じ tree（件名「integrate wu/close (phase close)」。SHA は targets.json の `head.sha`）。agent-docs の外は変えていない。
- B0: `agent-docs/progress/2026-11-07-health-v2-b0/test-targets.json`（件名「repair: main 取り込みの衝突解消の import を整形する」の commit）。
- host: home-dev、24 CPU、Linux 6.8.12-9-pve、cargo 1.98.1、cargo-nextest 0.9.146（B0 と同じ版）。CARGO_TARGET_DIR は celeris が渡した WU の scratch（開始時 1.3G。B0 と同じ大きさ）。
- 開始 2026-11-15T23:29:59Z、終了 2026-11-16T00:08Z。

## 手順
B0 の test-targets.md の「手順」をそのまま繰り返した。script は B0 の `agent-docs/progress/2026-11-07-health-v2-b0/test-targets-measure.sh`（変えていない）。
1. `python3 scripts/dev/test-target-inventory.py --json` → `targets-raw/inventory.json`
2. 5 crate を B0 と同じ順（task-core・task-dispatch・task-api・task-worker・celeris）で `test-targets-measure.sh build <crate> - targets-raw`
3. R1・R2 は celeris・task-worker・task-api・task-dispatch・task-core の順に `test-targets-measure.sh edit <crate> crates/<crate>/src/lib.rs <out>`（R1 は `targets-raw/`、R2 は `targets-raw/round2/`）
4. settle: R2 の task-core の対象 14 crate で `cargo nextest run -p … --no-run`（144 秒、exit 0、B0 も 144 秒）
5. R3 は celeris・task-dispatch・task-worker・task-api・task-core の順に edit（`targets-raw/round3/`）
6. `python3 agent-docs/progress/2026-11-15-health-v2-v0/targets-assemble.py` で原票を `targets.json` にまとめた。edit の 1 行は計測ごとに script の `git checkout` で戻した。試験の実行・全 workspace の試験・並走の負荷はしていない。

## test target と test binary（B0 → HEAD）
| crate | test target | 予測 binary | binary 実数（nextest） | 効いた変更 |
|---|---|---|---|---|
| task-api | 113 → 65 | 114 → 66 | 114 → 66 | R5 の束ね（59 target → 10 束）。R5 の記録時点の 64 から main で 1 増 |
| celeris | 55 → 45 | 57 → 47 | 57 → 47 | R5 の束ね（15 → 4 束）。R5 の再計測時点と同じ 45 |
| celerisctl | 18 → 15 | 19 → 16 | 未計測 | R5 の束ね（6 → 2 束）。R5 の記録時点の 14 から main で 1 増 |
| task-worker | 21 → 17 | 25 → 21 | 25 → 21 | R5 の束ね（5 → 1 束） |
| task-core | 2 → 2 | 3 → 3 | 3 → 3 | 無し |
| task-dispatch | 1 → 1 | 2 → 2 | 2 → 2 | 無し |
| ほかの 9 crate | 142 → 142 | 159 → 159 | 未計測 | 無し（下の注） |
| workspace | 352 → 287 | 379 → 314 | — | |

注: ほかの 9 crate（celeris-campaign 111・celeris-credentiald 7・celeris-script 7・e2e 10・celeris-mcp 1・jev-classifier 1・llm-proxy 2・routing-eval 1・task-ops 2 = 142）は B0 と全部同じ数。最大の celeris-campaign の 111 target は R6 でも束ねていない（R6 の進捗の提案 6）。合計の差 −65 は task-api −48・celeris −10・celerisctl −3・task-worker −4 の和。
- `tests/` の下の `.rs` は 373 → 402、支援 module は 21 → 115。束ねた target の中身が `tests/<束>/` の下の module になったため（inventory の `test_targets_by_location` で celeris の 4 target が入れ子）。
- 予測 binary の実数との一致は B0 と同じく 5 crate で確かめた。celerisctl とほかの crate の実数は数えていない。

## edit→verify（秒、B0 → HEAD）
| crate | build --no-run（1 回目） | --list の対象 crate | R1 | R2 | R3（settle 後） | R3 のコンパイル crate 数 |
|---|---|---|---|---|---|---|
| task-core | 44.52 → 55.69 | 14 → 14 | 144.08 → 147.12 | 139.44 → 149.04 | 142.25 → 144.68 | 14 |
| task-dispatch | 91.41 → 107.12 | 5 → 5 | 86.77 → 93.21 | 95.00 → 101.89 | 79.84 → 100.70 | 8 |
| task-api | 84.93 → 79.80 | 7 → 7 | 113.07 → 128.08 | 105.54 → 114.81 | 113.46 → 107.77 | 7 |
| task-worker | 54.74 → 40.67 | 7 → 7 | 135.76 → 185.30 | 134.77 → 143.75 | 98.13 → 103.07 | 7 |
| celeris | 82.24 → 97.60 | 2 → 2 | 32.81 → 26.27 | 81.80 → 82.89 | 85.06 → 83.87 | 12 |

- `--list` 自体は 0.10〜0.12 秒。task-api・task-core の変更では web の typecheck+unit、task-core では test-ts-verify も対象になる（B0 と同じ。未計測）。
- 交絡のない値は B0 と同じ 2 つ。celeris の R1（同じ組の直後、celeris と celerisctl の 2 crate だけ、32.81 → 26.27 秒）と task-core の 3 回（毎回 14 crate、142〜149 秒）。celeris の R1 の 6.5 秒の短縮は、R5 で celeris の test binary が 57 → 47 に減った分の link と合う向きだが、1 回の値で負荷も違うので効果の大きさは確定しない。
- task-api の R3 は B0 より 5.7 秒短い。R5 の再計測（同じ run の base と HEAD の対）では task-api の edit→verify は 99.25 → 99.16 秒でほぼ同じだった（`agent-docs/progress/2026-11-10-health-v2-r5.md`）。束ねの秒への効果はこの手順では load の揺れより小さい。
- task-worker の R1（185.30 秒）は計測中に load の 1 分値が 15.4 → 36.9 に上がった回で、B0 の R1 より 50 秒長い。R3 は 98.13 → 103.07 で同程度。
- build の 1 回目は依存の build を含む。task-core 以外は前の crate の後に測ったので、共有の依存は済んでいた（B0 と同じ）。

## 負荷の記録（/proc/loadavg の 1/5/15 分、前 → 後）
| 計測 | task-core | task-dispatch | task-api | task-worker | celeris |
|---|---|---|---|---|---|
| build | 15.17 17.62 13.55 → 15.81 17.35 13.70 | 15.81 17.35 13.70 → 7.77 14.56 13.13 | 7.77 14.56 13.13 → 10.36 13.37 12.81 | 10.36 13.37 12.81 → 10.81 13.13 12.76 | 10.81 13.13 12.76 → 16.83 14.47 13.27 |
| R1 | 25.39 24.93 19.03 → 29.37 26.75 20.48 | 24.29 24.76 18.37 → 25.39 24.93 19.03 | 33.11 23.72 17.07 → 24.29 24.76 18.37 | 15.40 14.12 13.19 → 36.91 23.94 17.03 | 12.08 13.55 12.99 → 15.40 14.12 13.19 |
| R2 | 22.28 25.27 21.77 → 22.64 24.60 21.98 | 31.08 26.77 21.79 → 22.28 25.27 21.77 | 24.82 23.27 20.16 → 31.08 26.77 21.79 | 17.54 22.35 19.48 → 24.82 23.27 20.16 | 23.24 25.52 20.18 → 17.54 22.35 19.48 |
| R3 | 23.36 25.94 23.63 → 35.63 28.27 24.67 | 16.28 22.56 21.86 → 22.29 23.44 22.25 | 27.08 25.68 23.21 → 23.36 25.94 23.63 | 22.29 23.44 22.25 → 27.08 25.68 23.21 | 31.60 26.67 23.02 → 16.28 22.56 21.86 |

- settle の前後: 22.64 24.60 21.98 → 31.60 26.67 23.02。
- 開始時の load は 15.10（1 分値）。同じ host で並行する WorkUnit（loc・loop-cost）と他の task の run が動いていた。load は cargo 自身の並列 build を含み、他 process と分けていない（B0 と同じ）。B0 の計測中の 1 分値は 2〜36、今回は 7.8〜36.9。

## 未計測と理由
- task-dispatch・task-api・task-worker の交絡のない 1 crate 編集の値: B0 と同じく、各計測の前に同じ組で settle する手順は 1 run の壁時計に収まらない。R5 の再計測（同じ run の base・HEAD の対）がこれに近い値を出している。
- web の typecheck+unit と test-ts-verify の時間、試験の実行時間: B0 と同じ範囲の外（全 workspace の試験は release gate だけで走らせる）。
- celerisctl とほかの 9 crate の test binary の実数。

## 検査
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/targets-verify.py agent-docs/progress/2026-11-15-health-v2-v0/targets.json`: inventory を流し直し、head の 287 target・314 binary との一致、5 crate の binary 数と予測の一致、edit_verify の 3 回 × 5 crate の秒と前後の load average、exit 0 を確かめる。欠ければ exit 1。提出時の HEAD で exit 0。
- `git status --short crates` は空（計測の 1 行は残っていない）。

## 原票
- `targets-raw/host.txt`・`head.txt`・`inventory.json`
- `targets-raw/build-<crate>.{json,log,binaries.json}`
- `targets-raw/edit-<crate>.{json,list,list.err,log}`（R1）、`targets-raw/round2/`、`targets-raw/round3/`（settle.txt・settle.log を含む）
