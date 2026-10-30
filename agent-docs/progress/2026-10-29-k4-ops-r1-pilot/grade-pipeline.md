---
title: K4 ops-r1 pilot 2 試行の独立採点と採点〜整合性 checker の通し（grade-pipeline）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 ops-r1 pilot 2 試行の独立採点と採点〜整合性 checker の通し（grade-pipeline）

状態: 完了。人が置いた raw（raw/A・raw/B・raw/stop.json）を読むだけで、固定票の celeris-campaign（repin-stop 後）と固定票の grader で
grade→check→export→human-op report→final-accounting→check-out-consistency.py を 2 回流し、全 step exit 0・2 回の出力が byte 一致した。課金は 0（LLM を呼んでいない）。

成果物（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/post/`

- `pipeline.json`（schema `ops-r1-post-pipeline/2`）: trials・steps・regen_identical・分母・整合性・停止記録の主体
- `run-post.sh`: 流した手順。`pass-1/`・`pass-2/`・`pass-1.log`・`pass-2.log`・`regen-diff.txt`（0 byte）
- `v1/`: 前回（前の repin の binary、`--b-stop` なし）の pipeline.json・pass-1・pass-2。check が B を拒否した記録として残した

## 結論

| trial | verdict | end_reason | grader_verdict | 集計の class |
|---|---|---|---|---|
| mlsys-main-4-ops-r1-k4p1-A-t001 | success | exited | pass | validated_success |
| mlsys-main-4-ops-r1-k4p1-B-t001 | fail | condition_drift | pass | executor_cancelled |

- A は assemble の outcome=passed と grader の final_pass=true の両方で success。grader の自己申告だけで決めていない。
- B は raw/stop.json の condition_drift で止めた試行として分母に残した（n=1、ran_trials=1、final_passed=0）。grader は CoS 作業場の写しを pass と
  判定したが、条件 B（CoS 受付→起票→task の実行）を守っていないので success に数えない。grader の判定は `grader_verdict` に別に残した。
- B の停止は外部 operator（Claude Code）が runbook 4.2/8 に従って記録したもので、人はまだ確認していない（pipeline.json の `b_stop.human_confirmed=false`）。
- 消費: 試行の tokens の合計は 194,226（A のみ。B は task の run が 0 件で 0）。B の CoS の 550,684 tokens は final-accounting の
  `shared_unattributable.cos` にあり、合計へは足さない。trial 別の cash cost は欠測（null）。

## run-post.sh の v1 からの変更

- binary を固定票の `tool_binaries.celeris-campaign`（repin-stop）に替えた。sha256 は固定票と一致。
- `ops-r1-assemble` に `--b-stop raw/stop.json` を渡す。
- raw/human-ops.jsonl を `<out>/human-ops.jsonl` に写し、cmp で一致を確かめる（docs/ops/eval-human-ops-runbook.md §1 の置き場所）。
  check-out-consistency.py は out の human-ops.jsonl で共有記録を数えるので、写さないと「shared_overhead has 18 records, human-ops.jsonl has 0 shared」で exit 1 になった。
- 出力 path を毎回 `post/work` に固定した。consistency.json が出力の絶対 path を含むので、pass ごとに path を変えると diff が出る。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `(cd raw && sha256sum --quiet -c SHA256SUMS)` → 実行前 exit 0、実行後 exit 0（raw は不変）
- `bash post/run-post.sh`（2 回）→ どちらも exit 0。steps 16 個すべて exit 0（a_raw・b_trace・b_attribution の再生成は raw の写しと cmp で一致）
- `diff -r post/pass-1 post/pass-2` → exit 0、0 byte。`cmp pass-1.log pass-2.log` → exit 0
- check-out-consistency.py → consistent=true、mismatches 0 件
- WU の check（pipeline.json の trials 2 件・verdict の値域・steps 全 exit 0・regen_identical）と、B の end_reason=condition_drift・verdict≠success → exit 0
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check` の結果は報告に書く

## 未解決

- B の停止（condition_drift）を人が確認していない。B を作業場所の渡し方を直した新しい condition_id でやり直すかは人が決める。
- B の CoS 消費を試行の tokens に含めるか（fix-pipeline の未解決）は人の判断待ちのまま。
- 固定票の `paid_trials_started` は 0 のまま。実際は 2 件開始した。report 段で記録する。

## 提案

- runbook の B 手順で、CoS thread の作業場にも inputs/ を置くか、CoS が task を起票しない場合の扱いを condition に明記する。
