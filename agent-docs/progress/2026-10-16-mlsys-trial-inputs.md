---
title: "MLSys 監査修正 6a: 試行の入力隔離と独立な実験証拠"
tasks: [01M530Y2NWW4ATM6N5WMQCCS9F]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# 監査修正 6a

ADR: `agent-docs/adr/2026-10-16-mlsys-trial-inputs-and-observed-evidence.md`。

| 項目 | 区分 |
|---|---|
| 試行 KB を workspace 内 `kb/` に接続（`runner.rs` build_seed）。A/B/C が同じ初期 materials/kb を参照 | 実装済み・mock 検証済み（`tests/trial_inputs.rs`） |
| 通常運用 KB・grader 私有値・他試行の書き込みが試行の木に無いこと | mock 検証済み（同上） |
| grader の実験証拠を評価側取得記録と照合（`celeris.observed-jobs/1`、`--observed`、grade-out の `observed/`） | 実装済み・mock 検証済み（`grader_independent.rs` 新 4 件） |
| 実 harness（claude / Celeris task）が `kb/` を実際に使うかの実経路確認 | 未実施（承認待ち: 本測定は未承認） |
| 実クラスタ job（K1/K2）の qstat 取得と observed 記録の作成 | 未実施（人または評価 command が実行。手順: job ごとに `qstat -x -f` の exit_status と生データ sha256 を `observed/<problem>/<Config>-rep<N>.json` に書く） |

## 未解決・提案
- A の `claude -p` は host の HOME を継ぐ。operator の個人設定が KB 相当として読める点は既存 sandbox の範囲で、今回は変更していない。本測定前に A の実行 user/HOME 隔離を決めること（人の決定）。
- プロンプトは workspace の `kb/` の存在を明示しない。必要なら case の prompt 側で書く。

## 証拠
- `bash scripts/dev/test-parallel.sh` → ok（passed 5560, failed 0, ignored 15, nextest_exit 0、doctest_exit 0。前回は k_cases_grade.rs と grade_case_gen.rs の GradeOptions に observed_jobs が欠けて compile 失敗していたのを修正）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
