---
tasks: [01M59KY5FNM0K4XGPPS61D0CA2]
wu: spec
status: done
completed: 2026-10-19
---
# dagbench spec: 固定シナリオ・指標・障害・baseline 仕様と再利用表

## 成果
- `agent-docs/adr/2026-10-19-dag-fault-comparison-driver.md`: 再利用表（R1〜R14、試験名と commit 付き）、新規最小差分、
  baseline 公開仕様（D2）、指標定義（D3）、障害 3 種と event 到達 trigger（D4）、外側/内側並列の分離（D5）、
  dataset 分離 `control_measured` vs `llm_campaign`（D6）、正常/障害を別 RQ にしない（D7）。
- `crates/campaign/fixtures/dagbench/scenarios.json`: `schema_version: 1`、seed 3 個、K2 縮小 DAG 2 本（k2-mini / k2-wide）、
  資源・外部 job 移行点・観測上限、固定点（同時 campaign 数 1/2/4、load 0.5/0.9/1.5、外部待機率 0.25/0.5/0.75、
  one-factor-at-a-time で 7 点）、障害 4 定義（a1/a2/b/c）。有料 LLM/job 0。

## 証拠
- `python3 -c 'import json; json.load(...)'` で JSON 妥当、派生値（total_worker_secs・arrival interval）を再計算して一致。
- `sh scripts/dev/check-adr-numbers.sh` / `check-doc-links.sh` exit 0。
- 検査 3 本の結果は報告本文（run の result.json）に記録。

## 未解決事項
- Celeris 側で worker 枠を scenario の `worker_slots` に写す具体の設定欄（`max_parallel_work_units` と daemon 全体の同時 run 上限の組合せ）は celeris-drv で確定する。
- 制御 process を子 process にして SIGKILL する形で、Dispatcher を daemon と同じ配線で起こせるかは celeris-drv で確認（足りない口は ADR 付記を書いてから最小で開ける）。

## 提案
- なし（仕様変更の提案はない）。
