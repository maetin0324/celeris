---
task: 01M59KY5FTRJTJ80PACNW3K533
wu: case-controls
status: done
completed: 2026-10-19
---
# case-controls: 短い ML case は延期（deferred）— 記録は case 探索外

- 結果: 延期状態を `crates/campaign/optional/short-ml-campaign/status.json` に保存（task ID、deferred、dev ラベル、inventory 証拠参照と commit c9654bff、理由 R1〜R3、未検証項目、再開条件、主比較への影響なし）。`cases/dev/short-ml-campaign/` は作らない。採用・無 LLM 正負例成立・実 pilot 確認は主張しない。
- 回収した前回の部分変更（commit a679ccf3）: 延期 dir を `cases/dev` に置き `CaseSet::load` で status.json だけの dir を読み飛ばしていた。これを修正: status.json を optional/ へ移動、`crates/campaign/src/cases.rs` と `tests/cases_load.rs` を c9654bff に戻し（必須 case.json 検査を保持）、読み飛ばし用 ADR を削除。loader/grader/既存 case/manifest は不変。
- 検査: `python3 crates/campaign/scripts/verify-short-ml-case.py --check` は延期記録の必須項目・整合と、cases/dev（と final）に延期専用 dir が無いことを検証する。成功は case の成立や採用を意味しない。
- inventory: inventory WU は独自 commit を持たず、base c9654bff 時点の結論 deferred を引き継いだ。
- 主実験への影響: なし。K2/K4 を置き換えない。

## 提案
- sirius の batch job で scheduler 証拠が得られる stdlib-only ML case を別 task で検討（新規資料が要る）。
