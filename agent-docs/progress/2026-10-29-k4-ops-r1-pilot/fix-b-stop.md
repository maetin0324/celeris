---
title: 停止記録で止めた B を ops-r1-assemble と checker で集計する（fix-b-stop）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
---
# 停止記録で止めた B を ops-r1-assemble と checker で集計する（fix-b-stop）

完了日: 2026-10-30

## 結論

`celeris-campaign ops-r1-assemble` に任意の `--b-stop <ops-r1-stop/1>` を足した。B-t001 のように runbook §5 の停止記録
（stop_id `condition_drift`）で止めた B は、分母に残したまま outcome `cancelled`・reason `condition_drift`・wall は受付〜停止の実測
で組まれ、grade-out・check・final-accounting・export・human-op report・check-out-consistency.py を exit 0 で通る。checker は停止の
reason の試行について、task の run が 0 件だけでは拒否しない。`--b-stop` を渡さない経路のコードは変えていない。daemon は変えていない。
仕様は `docs/ops/ops-r1-pipeline.md` §4.6、判断は ADR `agent-docs/adr/2026-10-30-ops-r1-pipeline-manifest.md` の付記（D-stop-1〜6）。

## 変更

- `crates/campaign/src/ops_r1_assemble.rs`: `--b-stop`、`StopRecord`・`load_b_stop`（schema・measurement_id・trial_id・stop_id・decided_by の検査）、
  `REASON_CONDITION_DRIFT` ほか runbook §5 の reason と `STOP_REASONS`・`is_stop_reason`、停止時の outcome/reason/wall の上書き、
  trace の intake が指す CoS chat run の共有行への写し、records.jsonl の `trial_stop` 行（decided_by・decided_by_note・observed_cos_runs）。
- `crates/campaign/src/checker.rs`: `reconcile_set_fixed` で停止の reason の試行は observed 0 件・worker 無しを違反にしない。
- `crates/campaign/tests/ops_r1_pipeline_assemble.rs`: 試験 2 件。
- 文書: `docs/ops/ops-r1-pipeline.md` §4.6 と §8 の実行例、ADR 付記。

commit（件名）: 「fix-b-stop: ops-r1-assemble に --b-stop を足し、停止した B を分母に残して集計する」

## 判断（根拠）

- outcome は `cancelled`: A の operator_stop と同じ。途中で止めた試行は failed（実行器の失敗）でも timeout（cutoff 到達）でもない。
  `cancelled` は `executor_cancelled` に分類され、grader が pass でも成功に数えない。
- CoS の chat run は `observed` に入れない: `observed` は set 宣言（model・role）の照合先で、CoS は §4.4 で試行の外側の共有消費として
  扱うため。代わりに `trial_stop` 行に role `cos_project` で載せ、消費は共有の CoS 行（`role=cos`）として写す。
- checker は executor_error と同じ扱い（照合から外す）にしなかった: 止める前に動いた task の run があれば固定条件で照合すべきため、
  0 件と worker 無しだけを許す。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p celeris-campaign --test ops_r1_pipeline_assemble`: 7 passed（既存 5 件＋新 2 件）。新試験で B は分母 2 の 1 行、
  cancelled・condition_drift・final_passed 0、wall 229 秒、check violations 0、check-out-consistency.py exit 0。trial_id 不一致は
  `--b-stop: trial_id` の error で out を作らない。
- `bash scripts/dev/test-affected.sh`: exit 0、2852 passed、9 skipped（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決

- 本番の raw（artifacts/ops-r1/raw/stop.json・B の trace/attribution）での assemble の実行は grade-pipeline WU の担当。この WU では
  stop.json が `StopRecord` の欄（schema・measurement_id・stop_id・trial_id・decided_by・decided_by_note）を持つことを読んで確かめただけ。
- `--b-stop` 入りの celeris-campaign の再 pin は repin-stop WU の担当。

## 提案

- 停止で止めた試行の B の tokens は task の run が無いので 4 欄 0 になる。CoS の消費（B-t001 では約 55 万 tokens）は共有の CoS 行に
  出るだけなので、報告では試行の tokens と CoS の消費を並べて書くこと（consumption WU の区分と合わせる）。
