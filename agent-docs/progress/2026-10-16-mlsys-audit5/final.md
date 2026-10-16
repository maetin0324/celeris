---
task: final-fix (監査修正5)
status: 実装済み・試験合格
completed: 2026-10-16
---
# final.json の C 要件・placeholder・alias・K2 時間（final-fix）

## 変更
- `CampaignManifest::check_resolved`（split=final の `validate` が呼ぶ）: `<…>` placeholder、`installed/latest/current/head` alias、celeris の harness_version が 12 桁 hex でない、B/C の routing 欠落、`lower_policy` 欠落、`online_update` が false でない、C の `evidence_table_id` 欠落を拒否。
- final.json: C に `routing`（mlsys-min-1 / `<routing evidence table id at H1>` / online_update=false）、A の alias を placeholder に。committed final.json は未解決なので `validate` は失敗する（H1 後の置換で通る。run-final.md §1 に置換手順）。
- `CaseSet::check_within_budget`: case の per_case wall/token と experiment.walltime が manifest の per_case に収まるか。
- 人の決定 k2-time（30 分へ縮小）に従い K2 case を縮小: `budget`/`per_case`/`per_arm` wall 7200→1800、`experiment.walltime_secs` 7200→1800、`params.storage` を [io_uring, posix]→[posix]（問い=xattr 付き restart の正しさと計測は不変。規模だけ縮小）。K4 の wall も 3600→1800（manifest の 30 分と不整合だったため）。本測定の出力は未見なので事後調整ではない。2 時間版が要れば別 campaign で人の承認（node-h）。
- materials・grade-case は不変（materials_sha256・grade_case_ref の hash は変わらず、`k_cases_grade` の再生成一致が通る）。予算（token_basis all、300 万/30 万/130 万、並列 1）は変更なし。

## 検証
- `cargo test -p celeris-campaign` 全 pass（新規 `tests/final_resolved.rs` 4 件: 未解決拒否・解決後通過・alias/routing 欠落拒否・時間整合）。
- 既存 final split fixture（cli_dryrun・runner_checker・lib 試験）は routing 付きの解決済み形に更新。

## 未解決
- 実際の claude-code 版・release sha12・evidence table id は H1 後に人が置換（本 WU は LLM CLI を起動しない）。
- 全件 preflight 経路から `check_resolved`/`check_within_budget` を呼ぶのは preflight WU。
