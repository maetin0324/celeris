---
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
title: 評価 trial の worker 本文に問題文を含める
---
# 評価 trial の worker 本文に問題文を含める

`routing-eval` の manifest 問題に必須 `prompt` を追加し、WU の objective / done_when と親 task の title / objective を保存する。acceptance と checks は材料にせず、採点 command の一致部分は伏せる。問題単位と manifest 全体の SHA に本文が含まれる。prompt のない旧形式は serde の読み込みエラーになる。

`make_trials` は評価の短い注意書きと問題本文を objective に設定する。採点 command はこれまでどおり check 欄だけに置く。LLM の本番評価はこの WorkUnit では実行しない。
