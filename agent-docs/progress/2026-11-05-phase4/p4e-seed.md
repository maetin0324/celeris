---
title: P4-E tier=llm の seed の段が実 LLM の worker に渡る不具合の修正
tasks: [01M6PZ88JMRZ52BP46GVK9Y1EN]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P4-E tier=llm の seed の段の修正

完了日: 2026-11-05。基準は `git merge-base HEAD main`。実 LLM は起動していない。本番の DB・daemon・config・release には触れていない。1 対の再試行は operator が行う。

## 結論
seed の段は tier に依らず stub の harness で起こすようにした。seed の段で fake 以外の adapter の run が見つかれば、その試行を失敗にして残りを始めない。

## 原因
`compare.py` の Trial は seed の段で `write_config(cos=False)` を呼ぶ。`write_config` は tier=llm のとき、`cos` に関わらず人の `--llm-config` を harness の block に使っていた。`seed N [seed:fail]` の task が `scripted_cos.py` ではなく実 LLM の claude_code の worker に渡り、実 LLM が task を done にした。結果は `seed task … ended done, want {'failed'}`（exit 1、stopped は usd_unknown、spent tokens 0）。daemon.log には adapter=claude-code・source=claude_oauth の dispatch が 2 件あり、その消費は記録されていない（実消費は未確認）。

raw は [p4e/llm-pair1](p4e/llm-pair1/)（run-summary.json、raw/07-read-three-old-r1.json、同 daemon.log）。`sk-ant`・`Bearer`・`accessToken`・`refreshToken` を grep し、3 file とも 0 件を確かめた。

## 変更（commit の件名）
- `p4e: seed の段は tier に依らず stub の harness にし、fake 以外の adapter の run があれば失敗にする`

内容:
- `write_config`: `tier == "stub" or not cos` のとき stub の config。`--llm-config` を seed の段に入れる分岐は無い。
- `Trial.check_seed_stub_only`: seed 後に daemon の `runs` から `adapter != 'fake'` を調べ、あれば RuntimeError。

## 試験
- `cd scripts/dev/general-cos-compare && python3 -m unittest test_compare` → exit 0、25 件 OK（既存の stub の試験を含む）。
- 新規 4 件: seed の段の config に `--llm-config` と cos-route の中身が無いこと、本番の段にあること、stub は両段とも stub であること、fake 以外の adapter の run を拒むこと。実 LLM は起こさない。

## 未解決
- 1 対の再試行は未実施（operator が行う）。
- seed の段で起動した実 LLM の 2 run の消費は記録が無い。実際の請求は未確認。

## 提案
なし。
