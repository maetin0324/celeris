---
title: 成果が案件の tree に無い outcome を到達に数えない（2 と 4）
tasks: [01M6MJEG72MD982T211EKAM74G]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# 成果が案件の tree に無い outcome を到達に数えない（2 と 4）

完了日: 2026-11-05（WorkUnit arrival-close。pilot-git・review-check・root-row の統合後の木を検査した）

## 結論
- 問題 2: repo 付き案件の root が、成果が workspace にしか無いのに「取り込み済み」に数えられていた。`root_row` は `IntegrationMerged` だけを数えるようにした（root-row）。pilot の作業場所は git にし、stub が成果を commit するようにした（pilot-git）。
- 問題 4: reviewer の合格で done になる原因は、決定的な検査が落ちても done になる経路ではなかった。command の検査を持たない acceptance（reviewer だけ）が合格になっていた。食い違いの記録は足した（review-check）。
- 統合後の木で、この WorkUnit は修正を要する食い違いを見つけなかった。コードは変えていない。

## workspace と案件 tree の関係
root-row の進捗（`2026-11-05-a9-fix-arrival/root-row.md`）の「workspace と案件 tree の関係」を参照。要点は、成果は task の workspace に先に在り、案件の tree に載るのは delivery が登録 repo の既定ブランチへ merge して `IntegrationMerged` を記録したときだけ。

## 各 WorkUnit の変更（件名）
- `pilot-git: pilot の作業場所を git にし stub が成果を commit する`
- `review-check: 決定的な検査が落ちたら reviewer の合格でも done にしない試験と食い違い event`
- `root-row: repo 付き案件は IntegrationMerged だけを取り込み済みに数える`
- 先の統合で pilot-git の check「crates/ に差分なし」が root-row・review-check の変更で必ず落ちた。その check は消してある。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `bash scripts/dev/test-affected.sh` → exit 0。この WorkUnit の base は統合後の HEAD と同じで差分が無いため「試験対象の変更なし」と出て crate の試験は走らなかった。文書検査（architecture-map、doc-links、adr-numbers、human-text、progress-index、commit-hashes）は ok。
- 統合した各 WorkUnit の試験は各進捗に記録済み（root-row: test-affected で nextest 7611 passed、task-ops `root_row_` 5 passed、`project_controller_e2e s5` 2 passed。review-check: `review_check_wins` 2 passed。pilot-git: `test-git-seed.sh` OK）。統合後の木では再実行していない（未確認）。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 4 の別原因: pilot 課題側に command の検査（`check-public-<id>.sh`）が無く、reviewer だけの acceptance が合格になる点は未修正（ADR D2 の課題側）。stub reviewer を「検査できない基準は pass にしない」にする D4 の 4 と `driver.py`・`stub_harness.py` の該当変更も未実施。
- 実 LLM の reviewer と実 daemon での通し（`test-long-horizon-pilot.sh`、repo 登録 API）は未確認。
- review-check の進捗に、test-affected で script runtime の試験が回ごとに別の 1 件落ちた記録がある（base での再現は未確認）。

## 提案
- 遠隔 repo の案件で `IntegrationNotApplicable` を到達に数えるかは人の判断（現状は数えない）。
- pilot 課題に command の検査を持たせる task を立てる（4 の残り）。
