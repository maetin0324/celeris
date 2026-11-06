---
wu: repair
status: done
completed: 2026-11-06
---
# repair: 統合後の木で検査を通し、旧欄名の残りを消す

## 変更
- `crates/task-core/src/tree.rs`: コメントから旧欄名を除いた。
- `crates/task-api/tests/project_controller.rs`: `get("max_tokens_per_day")`（旧欄が応答に無いことの確認）に `LEGACY_REJECT` を付けた。
- `scripts/dev/general-cos-compare/test_compare.py`: 旧 `max_tokens` を拒む試験の 2 行に `LEGACY_REJECT` を付けた。
- `docs/api/v1/api.md` の `budget_today` は usd・usd_unknown_runs・price_table_version の説明に直し済み。`driver.py` に旧欄名は残っていない。

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 101。落ちるのは task-core の既存試験 4 件（`project_proposal_tests.rs:194`、`script_exhaustive.rs:296`、`tests/project_retention_scale.rs:100`、`chat/plan_rejudge_tests.rs:78`）。この task は触っていないので直さない。
- `cargo fmt --all -- --check`: exit 0。

## 未解決事項
- 上の既存 clippy 指摘 4 件は別 task で直す提案。

## 本番への移行
この WU に本番 config の変更は無い（config WU の節を参照）。
