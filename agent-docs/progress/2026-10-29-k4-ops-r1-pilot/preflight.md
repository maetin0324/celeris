---
title: K4 ops-r1 pilot 無 LLM preflight
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 ops-r1 pilot 無 LLM preflight

状態: 完了。preflight.json は ready=true、全 43 step が exit 0、leak_check.pass=true。有料試行は 0 件。LLM は呼んでいない。

成果物（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/preflight/`

- `preflight.json`: 結果（ready・leak_check・steps・gaps・proposals）。前回（fix-pipeline 前、ready=false）の結果は `preflight.prev.json`、前回の作業物は `prev-run1/`
- `run-b-path.sh`・`steps.tsv`・`rec/`・`logs/`: B の経路の通し
- `pipe/`: 採点〜集計の通し（`run-pipeline.sh`、`make-manifest.py`、`pass-1/`・`pass-2/`）
- `leak-check.json`・`leak-check.with-deny-readable.json`: 漏洩検査

## 結論

- 測定 daemon: 固定票の採用 release（`release.adopted_release`）の bin/celeris（本番と同じ bytes）を `rel/releases/<採用 release>` に写して current を向け、`ops-r1-measure-config.sh --harness stub` の config で起動した。health の release は採用 release と一致、cos_policy_version "6"。本番 daemon・本番 DB・~/.config/celeris は書き換えていない。daemon は通しの終わりに止め、終了時に process が無いことを確かめた。
- B の経路（stub）: ops-r1-intake（--send、人の principal）→ 案件 CoS の起票（cos/operations task.create、applied、policy 6）→ worker run → done → CoS の報告 → cutoff。ops-r1-trace は 6 段の時刻を出し、ops-r1-attribution（/2）は cos_project 2 行・worker 1 行を出した。2 回出して byte 一致。
- 採点〜集計: fix-pipeline の経路（ops-r1-a-raw → trace・attribution → stage-grader-inputs → ops-r1-assemble → grade-out → check → export → human-op report → final-accounting → check-out-consistency.py）を A・B の stub 記録で 2 回流し、全段 exit 0。両腕とも試行 1・分母 1、独立 grader は fail（model_failure）、consistency は consistent=true・mismatches 0。2 回の出力は consistency.json（絶対 path を含む）を除き byte 一致。
- 漏洩検査: 公開入力 tgz（v2）・kb/snapshot.md・測定 KB・daemon の HOME・CoS 初期会話と会話・CoS/worker run の入力・外部 operator への材料・run の作業場所・A/B の stub 記録を scan し、grader-private の文字列・canary・v1 tgz の hash の hit 0。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `bash run-b-path.sh` → exit 0、14 step とも exit 0
- `bash pipe/run-pipeline.sh 1 pipe/manifest.json`・`... 2 ...` → 各 exit 0、11 step とも exit 0
- `diff -r -q -x consistency.json pipe/pass-1 pipe/pass-2` → exit 0
- `check-ops-r1-leak.py ... --out leak-check.json` → exit 0、hit 0
- WU の check（preflight.json の ready・steps・leak_check の assert）→ exit 0

## 未解決（preflight.json の gaps。どれも blocking ではない）

- ops-r1 manifest が freeze の bundle に無い。preflight は template を埋めた（trial_ids pf002）。実試行用は runbook か integrate-prep で作る
- release dir の写しで起動する手順、人の principal の token、b-observed-versions の Claude Code 版（固定票の値を写した）は runbook で決める
- B の CoS 消費を試行の tokens に含めるかは人の判断待ち（今は shared_unattributable.cos）
- grader-private は同じ uid で読める（既知の限界。canary の出現検査で代える）
- stub の B の worker の harness は fake と記録される。実試行では claude_code になる（未確認）

## 提案

- runbook で、実試行用 manifest の作成（pipe/make-manifest.py と同じ手順、trial_ids t001）と、開始時の health の release と固定票の adopted_release の照合を手順に入れる。
