---
title: K4 ops-r1 pilot 採点後の道具の再 pin（repin-stop）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 ops-r1 pilot 採点後の道具の再 pin（repin-stop）

状態: 完了。fix-b-stop の `--b-stop` を含む celeris-campaign を pin し直し、固定票に「試行の後の採点・集計の道具だけの改訂」として記録した。課金は 0（LLM を呼んでいない）。

成果物（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/freeze/`

- `bin/celeris-campaign-repin-stop-<build 時の WU HEAD の 12 字>`（path と sha256 は固定票の `tool_binaries.celeris-campaign`）。旧 `bin/celeris-campaign`・前回の repin の binary は残した（前の値は `tool_binaries.previous.celeris-campaign_history`）
- `fixed-ticket.json`・`fixed-ticket.md`: `tool_binaries.celeris-campaign` を更新し、`revisions`（kind「試行の後の採点・集計の道具だけの改訂（--b-stop。試行の条件・入力・model・daemon は変わらない）」、理由 grade-pipeline の plan_issue）と「改訂履歴」の repin-stop 節を足した。JSON で変えた key は tool_binaries・gap_resolution・revised_at・revisions だけ
- `gap-resolution.json`: `g-ops-r1-b-stop` を resolved=true で足した（証拠は fix-b-stop の試験 2 件と exit 0。all_resolved true）
- `digests.sha256`: 作り直した（9 件。既存 6 件の bin・measure の hash は前と同じ）

## 結論

- WU の HEAD（件名「fix-b-stop: ops-r1-assemble に --b-stop を足し、停止した B を分母に残して集計する」）は段 fixpipe の統合と fix-b-stop を含むので、一時 merge なしでそのまま build した。crates/campaign の git tree hash は固定票の `tool_binaries.celeris-campaign.built_from` に書いた。
- measurement_id・condition_id・trial_id は変えない。変わるのは試行の後に流す採点・集計の道具だけで、入力・model・limits・grader・release・B の daemon.toml は変わらない。
- 旧 bundle・台帳・raw（artifacts/ops-r1/raw を含む）は読み書きしていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo build --release -p celeris-campaign --bin celeris-campaign` → exit 0
- `<新 binary> ops-r1-assemble --help | grep -c -- --b-stop` → 1
- `cargo nextest run -p celeris-campaign --test ops_r1_pipeline_assemble` → exit 0、7 passed（`ops_r1_pipeline_b_stop_condition_drift_passes_the_stages`・`ops_r1_pipeline_b_stop_rejects_a_foreign_trial` を含む）
- fixed-ticket.json の celeris-campaign の sha256 と実ファイルの sha256 が一致（python で照合、True）
- `(cd <freeze> && sha256sum -c digests.sha256)` → exit 0、9 件 OK
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check` の結果は報告に書く

## 未解決

- 固定票の `paid_trials_started` は 0 のまま（freeze 時の値）。A-t001・B-t001 は実行済みなので、post 段（grade-pipeline か report）で実際の件数を固定票の外の記録と照らして書く。この WU の範囲外として変えていない。
- B の CoS 消費を試行の tokens に含めるか（fix-pipeline の未解決）は人の判断待ちのまま。

## 提案

- 試行の後に道具を直す度に repin が要る。固定票に「採点・集計の道具」と「試行の条件」を別の欄として分け、前者の改訂では condition_id を変えないことを契約に書いておく。
