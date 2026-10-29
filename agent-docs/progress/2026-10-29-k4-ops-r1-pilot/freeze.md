---
title: K4 ops-r1 pilot 固定票（freeze）— blocking な不足 5 件のため停止
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: blocked
updated: 2026-10-29
---
# K4 ops-r1 pilot 固定票（freeze）

状態: 停止（blocked）。固定票・bundle・digests.sha256 は作っていない。

WU freeze の Objective は「inventory.json の blocking_gaps が空のときだけ進む」。
2026-10-29T03:09Z に確かめたところ、blocking_gaps は 5 件で空ではなかった。
このため measurement_id・condition_id・trial_id を発行していない。旧 bundle・台帳にも触れていない。

停止記録（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/freeze/stop.json`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `python3 -c "json.load(open('inventory.json'))['blocking_gaps']"` の結果は 5 件。g-b-cos-intake・g-b-trace-export・g-b-trial-attribution・g-cos-measure-daemon・g-grader-private-isolation
- コードは変えていない（進捗ファイルだけ）ので、test-affected・clippy・fmt の対象はない

## 未解決

- 決定 b-env（測定専用 daemon を別 state で起こし、本番 release 94f86928dc13 の binary を使う）は受けた。ただし固定票をまだ作っていないので、この決定は固定票に反映していない。
- d-ops-r1-models（B の model 条件）が未決定。inventory では needed_before が freeze になっている。

## 提案

blocking な 5 件それぞれに修正 task を 1 件起こす。済んだら freeze をやり直す。各 task の条件は inventory.json の tooling_gaps にある。

1. g-b-cos-intake: B の依頼を案件 CoS thread の message として投入し、trial_id・condition_id・受付時刻・message_id を raw に残す CLI と、それを確かめる無 LLM 試験
2. g-b-trace-export: 受付→起票→run→成果確定→報告→cutoff の 6 段を時刻と参照つきで出す読み取り専用 CLI。DB が同じなら出力が byte 一致すること
3. g-b-trial-attribution: CoS が起票した task とその子孫の run、案件 CoS・統括 CoS の turn を trial に帰属させる。監督 CoS の消費は shared のまま分けておく
4. g-cos-measure-daemon: 測定 daemon の設定（release 94f86928dc13・[cos] enabled・policy 6・skill 配送・route 固定）と、stub harness で CoS 経路を通す preflight
5. g-grader-private-isolation: private.json・旧判定・旧 v1 tgz が A/B・CoS の run、KB、memory、外部 operator の入力から読めないことを、canary で確かめる preflight
