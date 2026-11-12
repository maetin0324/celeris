---
title: 健全性v2 R5 meas-runs — 4 crate の試験の実行時間と flaky を base と HEAD の交互実行で測る
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# 健全性v2 R5 meas-runs

## 結果

- task-api・celeris・celerisctl・task-worker の 4 crate で、base・HEAD・HEAD・base の順の 4 回の実行を同じ run で測った。各 crate の 2 組（1 回目と 2 回目、4 回目と 3 回目）は 8 組すべてが採用（included）になった。load の比は 1.00〜1.39 で、比 2 以上の組は 0。受け入れ条件 0 を満たす。
- 16 回の実行はすべて exit 0、失敗した試験は 0 件だった。flaky（4 回で結果の違う試験）は 4 crate とも 0 件。
- 実行した試験の数・SKIPPED の数は、base と HEAD で同じだった（task-api 877・2、celeris 805・2、celerisctl 205・0、task-worker 1285・9）。
- celerisctl の実行時間は base 3.233・3.487 秒、HEAD 3.304・3.332 秒で、差は 0.16 秒以下だった。前回の「HEAD が 2〜3 倍遅い」は同じ run では再現しなかった（下の「celerisctl の前回の差」）。

- コマンド: `MEASURE_MAX_LOAD=24 MEASURE_DEADLINE=1794489720 timeout --kill-after=20 2891 bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh run-compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-runs.json task-api,celeris,celerisctl,task-worker`
- 負荷の上限は決定 meas-load の答え a に従い 24 にした。
- MEASURE_DEADLINE は run の開始（2026-11-12T12:32:00Z）から 3000 秒（2026-11-12T13:22:00Z）にした。run-compare の本体（measure_summary.py）は MEASURE_DEADLINE を読まないので、外側の `timeout` で同じ時刻に止まるようにした。計測は期限に当たらなかった。
- 実行時刻は 2026-11-12T12:33:49Z〜12:45:57Z（exit 0）。初回の試行で 4 crate とも組が採用になり、再試行（MEASURE_RETRIES 既定 2）は走らなかった。
- 記録の 1 分値（16 回の実行それぞれの前・後・負荷待ちの値、48 個）は 4.50〜18.97。負荷待ちはすべて 0 秒（各実行の直前の値は 24 以下）だった。
- 実行の順は crate ごとに base → HEAD → HEAD → base。各実行の直前に、その側の `cargo nextest run -p <crate> --no-run` を走らせた（celeris は加えて `cargo build -p celeris-campaign --bins`）。この build の時間は測っていない。
- base は `git merge-base HEAD main` を `$TMPDIR` の git worktree に置いた写し、HEAD はこの branch の HEAD（どちらも記録の git_head・merge_base_main 欄にある）。写しの worktree は計測の後に消えた（`git worktree list` に残っていない）。

`bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary agent-docs/progress/2026-11-10-health-v2-r5/remeasure-runs.json` の出力:

<!-- remeasure:begin -->
| crate | stage | attempt/pair | base s | HEAD s | base nextest s | HEAD nextest s | load ratio | result | reason |
|---|---|---|---:|---:|---:|---:|---:|---|---|
| task-api | run | 1/1 | 48.547 | 25.717 | - | - | 1.39199 | included | - |
| task-api | run | 1/2 | 26.945 | 26.432 | - | - | 1.08214 | included | - |
| celeris | run | 1/1 | 41.304 | 37.876 | - | - | 1.06667 | included | - |
| celeris | run | 1/2 | 37.568 | 37.367 | - | - | 1.08753 | included | - |
| celerisctl | run | 1/1 | 3.233 | 3.304 | - | - | 1 | included | - |
| celerisctl | run | 1/2 | 3.487 | 3.332 | - | - | 1.18669 | included | - |
| task-worker | run | 1/1 | 20.948 | 19.838 | - | - | 1.10837 | included | - |
| task-worker | run | 1/2 | 19.945 | 19.772 | - | - | 1.1782 | included | - |
<!-- remeasure:end -->

秒は `cargo nextest run` の process の壁時計（cargo の起動と試験の一覧の取得を含む）。pair 1 は 1 回目（base）と 2 回目（HEAD）、pair 2 は 4 回目（base）と 3 回目（HEAD）の組。

## 件数・SKIPPED・test binary（同じ run）

nextest の `Starting` 行と `Summary` 行から。4 回とも同じ側では同じ値だった。

| crate | 実行した試験 base / HEAD | SKIPPED base / HEAD | 失敗 | test binary base / HEAD | nextest の Summary の秒（1〜4 回目） |
|---|---|---|---|---|---|
| task-api | 877 / 877 | 2 / 2 | 0 | 114 / 65 | 46.429・24.931・25.661・26.030 |
| celeris | 805 / 805 | 2 / 2 | 0 | 58 / 47 | 38.443・37.024・36.495・36.652 |
| celerisctl | 205 / 205 | 0 / 0 | 0 | 19 / 15 | 2.491・2.582・2.610・2.746 |
| task-worker | 1285 / 1285 | 9 / 9 | 0 | 25 / 21 | 19.163・19.150・19.096・19.242 |

- SKIPPED の試験は両側で同じ binary・同じ名前だった（task-api は lib の 2 件、celeris は long_run_soak と tick_prod_copy_steps の各 1 件、task-worker は lib の 4 件・browser_runtime_isolated の 3 件・ssh_cluster_manual の 2 件）。これらの binary は束ねられておらず、名前は両側で同じ。
- 試験の名前（binary 名 + 試験の path）が片側にだけある件数は、task-api 325・celeris 57・celerisctl 24・task-worker 30 で、両側で同じ数だった。束ねで binary 名と module の path が変わったためで、件数は変わっていない。

## celerisctl の前回の差

- 前回は baseline（2026-11-10、baseline.json）の 4.78・4.45 秒と、HEAD（2026-11-11、after-run.json）の 7.37・13.65 秒を比べ、HEAD が 2〜3 倍遅かった。2 つは別の run で、実行前の 1 分値は baseline 29.00・27.64、HEAD 55.14・52.58 だった。
- 同じ run の今回は、base 3.233・3.487 秒、HEAD 3.304・3.332 秒。失敗 0・SKIPPED 0・205 件で、両側で同じだった。test binary は 19 → 15。
- 失敗・SKIPPED・件数の差は無く、同じ run・1 分値 5.60〜8.20 の下では差が 0.16 秒以下だった。前回の 2〜3 倍は R5 の変更によるものではなく、別の run の条件（1 分値が約 2 倍、別の日）の差と判断する。前回の差の内訳（どの試験が遅かったか）は、前回の記録に試験ごとの時間が無いので未確認。

## 読み方

- task-api の pair 1 は base 48.547 秒・HEAD 25.717 秒で差が大きい。pair 2 は base 26.945 秒・HEAD 26.432 秒で差は 0.5 秒（2%）。1 回目は記録の最初の実行で、base の build（76 個の Compiling、1 分 22 秒）の直後に 114 個の test binary を初めて走らせた回。初回の起動の遅れと見るが、原因は未確認。pair 1 の差を R5 の束ねの効果とは書かない。
- celeris・celerisctl・task-worker は、どちらの組でも base と HEAD の差が 3.5 秒以下（celeris pair 1 の 3.43 秒が最大）。HEAD の方が短い組が 6 組中 5 組だが、差は 1 組の中のばらつき（同じ側の 2 回の差: celeris base 3.7 秒）と同じ大きさなので、速くなったとは言えない。
- B0 の baseline.json の実行時間（task-api 59.56・60.64 秒、celeris 184.3・69.98 秒）は、1 分値 34〜84 の下の値で、今回（1 分値 4.50〜18.97）と同じ条件ではない。今回は同じ run の base と HEAD だけを比べる。
- flaky の判定は measure_summary.py の flaky()（4 回のうち (1,4)・(2,3)・(1,2)・(3,4) の組で結果の違う試験。束ねで変わった名前は一意に対応する時だけ base の名前に寄せる）。16 回とも失敗が 0 件なので、名前の対応のしかたに関わらず flaky は 0 件。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` → exit 0（v8 の build output 1 件）。計測の前に実行。
- 上記の run-compare → exit 0（2026-11-12T12:33:49Z〜12:45:57Z）。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require-compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-runs.json task-api,celeris,celerisctl,task-worker` → exit 0。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/remeasure-runs.json` → exit 0。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary --check agent-docs/progress/2026-11-10-health-v2-r5/meas-runs.md agent-docs/progress/2026-11-10-health-v2-r5/remeasure-runs.json` → exit 0。
- 生の記録は `remeasure-runs-raw/`（各実行の nextest の log と直前の build の log）。

## 検査（提出時の HEAD で実行。WU の基準は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh` → exit 0（137 秒。docs のみの変更として試験対象なし、repo_health_gate は合格）。
- `cargo clippy --workspace -- -D warnings` → exit 0（53 秒）。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0（変更 path 34 件はすべて `agent-docs/progress/2026-11-10-health-v2-r5/` の下）。
- 上記の summary --check → exit 0。
- commit は「phase 1: meas-runs: 4 crate の試験を base・HEAD・HEAD・base の順に負荷上限 24 の同じ run で測る（8 組とも採用、flaky 0）」と「phase 1: meas-runs: 検査の結果を進捗に書く」。

## 未解決

- 無し。

## 提案

1. run-compare の本体（measure_summary.py の execute）は MEASURE_DEADLINE を読まない。measure.sh の説明は「これを過ぎたら残りの段を走らせない」と書くので、run-compare・compare でも期限を見て残りの実行を skipped にする（今回は外側の `timeout` で代えた）。
2. measure_summary.py の parse_run は、SKIP の行の `(───────) <binary>` を試験の名前として読むので、同じ binary の SKIPPED の試験が outcomes の 1 件にまとまる（task-api は SKIPPED 2 件が 1 件）。件数（tests_run・skipped_tests）は Summary 行から読むので正しく、実行した試験の flaky の判定には影響しない。SKIP の行の名前を正しく読むように直す。
3. remeasure-runs.json は試験ごとの結果を results・pairs・comparisons の 3 か所に持つので 4.1 MB になった。pairs・comparisons は results の index を指す形にすると小さくできる。
