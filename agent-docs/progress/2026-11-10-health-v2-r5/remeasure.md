---
title: R5 再計測の手順・環境・負荷
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# R5 再計測の手順・環境・負荷

2026-11-12 に各 leaf が同じ host `home-dev` で base（`git merge-base HEAD main`）と HEAD を比較した。base は run の TMPDIR の worktree、target は Celeris が渡すローカル scratch。toolchain・cargo profile・wrapper は host と run の設定を継承した。host・target は各 JSON の `host`・`cargo_target_dir`、実行コマンドは生 log に記録している。toolchain の版を原票に固定した記録は無く、その点は未確認。B0 と同じ lib の編集後の一覧取得・再 build を測り、今回の総時間には doc test も足した。全試験・web e2e・実 browser/userns は実行しない。

決定 meas-load の a により operator が他の重い task を一時停止し、計測 leaf は順番に実行した。CPU を焼く負荷は加えていない。`MEASURE_MAX_LOAD=24` で段の開始を待つ。段の終了時は 24 を超える場合もあり、採用は前後の load 最大の両側比が 2 未満かで判定する。

| leaf / 原票 | UTC 開始〜終了 | 記録した load 1 分の範囲 | 負荷待ち最長 |
|---|---|---|---|
| [meas-edit-a.md](meas-edit-a.md) / remeasure-edit-a.json | 10:20:39〜10:44:12 | 1.72〜26.81 | 35 秒 |
| [meas-edit-worker.md](meas-edit-worker.md) / remeasure-edit-worker.json | 11:34:30〜11:45:33 | 0.56〜23.65 | 20 秒 |
| [meas-edit-dispatch.md](meas-edit-dispatch.md) / remeasure-edit-dispatch.json | 11:51:55〜12:02:40 | 1.59〜23.07 | 0 秒 |
| [meas-edit-core.md](meas-edit-core.md) / remeasure-edit-core.json | 12:10:49〜12:23:17 | 1.52〜24.43 | 25 秒 |
| [meas-runs.md](meas-runs.md) / remeasure-runs.json | 12:33:49〜12:45:57 | 4.50〜18.97 | 0 秒 |

build/edit の手順は `MEASURE_MAX_LOAD=24 MEASURE_DEADLINE=<期限> bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh compare <原票> <crate一覧>`。settle 後に lib の mtime を進めて build し、編集後の test-affected の対象の再 build と doc test を測る。worker 以降は base build → HEAD build → base edit→verify → HEAD edit→verify。edit-a は修正前の側ごとの build → edit→verify で測ったが、各組の load 比・exit は採用規則を満たすため今回そのまま使う。順序差と各 1 組の制約から数秒の差を改善とは断定しない。

run は `MEASURE_MAX_LOAD=24 bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh run-compare <原票> task-api,celeris,celerisctl,task-worker`。base → HEAD → HEAD → base と交互に走らせ、各回の直前にその側を build（celeris-campaign も）する。build の時間は run の秒に含めない。期限は外側の timeout でも制限した。

原票の summary を 5 個まとめて生成し、[R5 の進捗](../2026-11-10-health-v2-r5.md) にそのまま載せた。除外は celerisctl edit→verify の 3 組（base doc test exit 101）だけ。load 比による除外は今回の 5 原票には無い。worker の以前の試行の build 比 2.93〜6.52 は除外済みで、詳細と load は meas-edit-worker.md に残す。旧 after-build・baseline の秒は今回と比較しない。

## 未解決事項と提案

- 1 組だけの build/edit の秒の差がばらつきを超えるかは未確認。task-api run の初回だけ遅い理由も未確認。
- 小さい変更で workspace doc test の費用が増える。対象規則の変更は別の設計判断として扱い、この WU では変えない。

生成表の一致検査:

```sh
bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh summary --check agent-docs/progress/2026-11-10-health-v2-r5.md agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-a.json agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-worker.json agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-dispatch.json agent-docs/progress/2026-11-10-health-v2-r5/remeasure-edit-core.json agent-docs/progress/2026-11-10-health-v2-r5/remeasure-runs.json
```
