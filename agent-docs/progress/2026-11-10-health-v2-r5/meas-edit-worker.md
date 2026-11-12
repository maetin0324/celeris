---
title: 健全性v2 R5 meas-edit-worker — task-worker の base と HEAD の build と edit→verify を同じ run で測る
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: blocked
updated: 2026-11-12
---
# 健全性v2 R5 meas-edit-worker

## 結果

- edit→verify は base と HEAD の組が 3 つ取れ、3 つとも load の比が 2 未満で採用（included）になった。
- build は 3 つの組がすべて load の比 2 以上で除外された。受け入れ条件（build と edit→verify の両方に比 2 未満の組）は満たしていない。
- build の除外は計測の手順そのものが作る負荷による（下記「build が除外された理由」）。同じ台本で WU を再試行しても同じ結果になる見込みが高いので、提案 1 の直しを計画に求める（plan_issue）。

- コマンド: `MEASURE_MAX_LOAD=24 MEASURE_DEADLINE=<開始 + 3000 秒> bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json task-worker`
- 負荷の上限は決定 meas-load の答え a に従い 24 にした。
- 実行時刻は 2026-11-12T10:51:20Z〜11:24:41Z（2001 秒、exit 1）。3 回の試行（初回 + 再試行 2 回）をすべて走らせた。
- 記録の 1 分値（各段の前後）は 2.67〜32.07。段の開始前の値はすべて 24 以下で、負荷待ちは最長 20 秒だった。32.07 は試行 2 の base の edit→verify の直後の値。
- base は `git merge-base HEAD main`、HEAD はこの branch の HEAD（どちらも記録の git_head・merge_base_main 欄にある）。

`bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json` の出力:

<!-- remeasure:begin -->
| crate | stage | attempt/pair | base s | HEAD s | base nextest s | HEAD nextest s | load ratio | result | reason |
|---|---|---|---:|---:|---:|---:|---:|---|---|
| task-worker | build | 1/1 | 37.46 | 36.31 | - | - | 6.51622 | excluded | load ratio >= 2 |
| task-worker | edit_verify | 1/1 | 117.35 | 109.75 | 108.58 | 94.29 | 1.02063 | included | - |
| task-worker | build | 2/1 | 37.48 | 36.02 | - | - | 2.9276 | excluded | load ratio >= 2 |
| task-worker | edit_verify | 2/1 | 109.99 | 117.35 | 101.55 | 101.84 | 1.74768 | included | - |
| task-worker | build | 3/1 | 38.38 | 35.98 | - | - | 3.56833 | excluded | load ratio >= 2 |
| task-worker | edit_verify | 3/1 | 109.12 | 108.74 | 100.14 | 93.44 | 1.25692 | included | - |
<!-- remeasure:end -->

build の秒は「task-worker の lib と全 test target の作り直しと link」、edit→verify の秒は doc test 込み、nextest s は doc test を除く edit→verify（`edit_to_verify_secs`）。

## build が除外された理由

3 回とも段の順は base の build → base の edit→verify → HEAD の build → HEAD の edit→verify だった（crate が 1 つなので、試行ごとに順が入れ替わらない）。

| 試行 | base の build の 1 分値（前 → 後） | HEAD の build の 1 分値（前 → 後） | 比 |
|---|---|---|---:|
| 1 | 2.67 → 3.39 | 22.09 → 13.90 | 6.52 |
| 2 | 7.32 → 6.54 | 21.43 → 12.98 | 2.93 |
| 3 | 6.00 → 5.17 | 21.41 → 13.31 | 3.57 |

- HEAD の build の直前に base の edit→verify（14 crate の settle と作り直し、約 110 秒）が走り、1 分値を 22 前後に上げる。HEAD の build はその直後に始まるので、開始時の 1 分値が base の build の 3〜6 倍になる。
- 上限 24 は超えていないので、負荷待ちは起きない。1 分値の半減は約 42 秒なので、待てば下がる値だった。
- 秒そのものは base 37.46・37.48・38.38、HEAD 36.31・36.02・35.98 で、組の中の差は 1.2〜2.4 秒だった。負荷の差が秒にどれだけ効いたかは未確認。

## test binary と link の数（同じ run、3 回とも同じ）

| 段 | base | HEAD |
|---|---:|---:|
| build の test binary（link した数） | 25（25） | 21（21） |
| edit→verify の対象全体の test binary（link した数） | 334（334） | 266（266） |

## 読み方（edit→verify）

- doc test 込みの edit→verify は base 117.35・109.99・109.12 秒、HEAD 109.75・117.35・108.74 秒。3 組の差は −7.60・+7.36・−0.38 秒で、向きがそろっていない。
- nextest 部分は base 108.58・101.55・100.14 秒、HEAD 94.29・101.84・93.44 秒で、3 組中 2 組は HEAD が 7〜14 秒短い。
- doc test は base 8.44〜8.98 秒（`-p` の組）、HEAD 15.30〜15.51 秒（`--workspace`）。meas-edit-a と同じく、HEAD の doc test の呼び方の費用が nextest 部分の短縮を打ち消す。
- 前回の HEAD の 190.58 秒（measure-build、負荷 23〜84）と B0 の 98〜136 秒は条件が違う。今回の同じ run の組では、HEAD は base と同じ程度（108.74〜117.35 秒）で、悪化は見られない。
- 1 回ずつの組が 3 つなので、数秒の差がばらつきを超えるかは未確認。

## 証拠（基準は `git merge-base HEAD main`）

- `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` → exit 0（v8 の build output 1 件）。
- 上記の compare → exit 1（2001 秒。build の 3 組がすべて除外されたため）。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json` → exit 0。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require-compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json task-worker:edit_verify` → exit 0。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require-compare agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json task-worker` → exit 1（build の採用の組が 0）。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary --check agent-docs/progress/2026-11-10-health-v2-r5/meas-edit-worker.md agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json` → exit 0。

## 検査（commit 前の作業ツリーで実行。変更は上記の 3 path だけ）

- `bash scripts/dev/test-affected.sh` → exit 0（140 秒。docs のみの変更として試験対象なし、repo_health_gate は合格）。
- `cargo clippy --workspace -- -D warnings` → exit 0（52 秒）。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0（変更 89 path はすべて `agent-docs/progress/2026-11-10-health-v2-r5/` の下）。

## 未解決

- task-worker の build の採用の組が無い（受け入れ条件 0 の build の部分）。

## 提案

1. measure.sh の compare で、build の段の前に 1 分値が下がるのを待つ（例: 段の直前の 1 分値が同じ crate の相手側の build の開始時の 2 倍未満になるまで待つ）か、両側の build を両側の edit→verify より先に測る順にする。除外の規則（比 2 未満）は弱めない。どちらも measure.sh（meas-tools の範囲）の変更なので、この WU では変えていない。
2. 直した後に、この WU の compare を同じコマンドで測り直す。
