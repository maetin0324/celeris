---
title: K4 case を公開入力 v2 に揃える
tasks: [01M5SRJMA9CFNG2C9KCCS5VS4B]
status: done
updated: 2026-10-25
---
# K4 case を公開入力 v2 に揃える

完了日: 2026-10-25

## 要約
- archive WU の manifest は `changed: true`。K4 case を新 archive `main-v3-n2-43706-public2.tgz`
  （sha256 `0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e`）に揃えた。
  旧 `main-v3-n2-43706.tgz`（`128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1`）は版名 v1。判断は ADR 2026-10-25-k4-public-input-v2。
- 評価規則・分母・期待値（32 セル、30 有効、FS 別 6/8/8/8、BenchFS W2 の 2 セルは `missing`・`job_failed(exit=-20)`）は変えていない。
- 旧 raw・bundle・採点・main-4 revisions には書いていない（repo 内の file だけを変えた）。

## 変えたもの
| file | 変更 |
|---|---|
| `crates/campaign/cases/final/k4-cmp4-validity-analysis/case.json` | `inputs[0]` の name・sha256・fetch、`materials_sha256`（`8b4e5c6463baab47df43abf2f9a8cc4ccf1d47714435329aac38d6c66463e89f`）、`grade_case_ref.sha256`（`7751a8577dd09d3f8cd5729179bfbc819aaf87249bb46f16e3ecb751ce48ca54`）、`private_ref.sha256`（`46ea074be15202c4b3d2fca143563ce9c06822607f51e656469da6ad5f6871da`） |
| `…/materials/task.md` | 展開後の構成を v2 に（`runs/` と `review28/` の scheduler 原票）。除いた file には触れない |
| `…/kb/snapshot.md` | 変更なし（archive に触れていない） |
| `fixtures/grade-cases/final/k4-…/k4-…json` | `input_archive` の path・sha256（generator で再生成。`pinned_raw`・metrics・cells は v1 と同じ） |
| `fixtures/grader-private/final/k4-…/private.json` | `expected.input` だけ |
| `fixtures/grade-cases/tools/gen_recompute.py` | 使い方を v2 に |
| `src/cells_validity.rs` | `install_k4_fixture` が case の `inputs[0].name` も合成用の名前に替える |
| `scripts/verify-real-assets.sh`、`scripts/campaign/test_check_main_pins.py`、`docs/ops/measure-run-final.md` | 既定 path・名前を v2 に |
| `tests/k4_cells_validity.rs` | 既定 path を v2 に。数値誤りの独立負例 `numeric_error_in_one_valid_cell_is_rejected_on_its_own` を足す。実 archive 試験に「失敗 job の W2 だけ有効」「欠測を分母から落とす（30/30）」「有効セル 1 つの帯域 2% 誤り」の負例を足す |
| `tests/k_cases_grade.rs`、`tests/real_asset_grade.rs` | 既定 path と固定 sha256 を v2 に |
| `tests/preflight_input_archive/real.rs` | 実 case の入力名（v2）で実 archive を置く |
| `tests/preflight_setup/mod.rs`、`tests/lock_execution.rs` | `K4_INPUT` が合成 raw の名前であることを doc comment に書く |

commit: 「campaign: K4 case を公開入力 v2（main-v3-n2-43706-public2.tgz）に揃え、数値誤りの独立負例を足す」、「progress: K4 case v2 の検査結果を記録」。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- generator の再生成で差分ゼロ: `python3 -I crates/campaign/fixtures/grade-cases/tools/gen_recompute.py <main-43223.tgz> <public2.tgz> crates/campaign/fixtures/grade-cases && git diff --stat` → 差分なし。
  case.json の `grade_case_ref`・`private_ref` の sha256 は実 file と一致、`inputs[0]` の name・sha256 は grade-case の `input_archive` と一致。
- `CAMPAIGN_K4_TGZ=<public2.tgz> cargo test -p celeris-campaign --test k4_cells_validity --test preflight_input_archive --test k_case_judged_scope --test real_asset_grade --test k_cases_grade -- --nocapture`
  → exit 0。k4_cells_validity 11 passed（`real_k4_archive_grades_thirty_of_thirty_two` を含め SKIP なし）、preflight_input_archive 12 passed・1 ignored（実 archive の audit は下で別に回した）、k_case_judged_scope 2 passed、
  `real_k4_raw_recompute_matches_the_grader`・`k3_k4_recompute_rejects_a_wrong_report_and_accepts_the_raw_derived_one` も ok。
  real_asset_grade の SKIPPED 2 件は K2 の asset（`CAMPAIGN_ASSET_DIR`）で、本件と無関係。
- `K4_AUDIT_DIR=… K4_REAL_ARCHIVE=<public2.tgz> cargo test -p celeris-campaign --test preflight_input_archive -- --ignored real_archive_offline_audit` → 1 passed。
  stage/grade/grade-out/check/export は exit 0、全セル有効の grade・archive 欠落と sha256 不一致の stage は exit 1。記録は WU の artifacts `real-audit/summary.json`（fixture の写しは容量のため消した）。
- `python3 scripts/campaign/test_check_main_pins.py` → OK。
- `bash scripts/dev/test-affected.sh` → exit 0、2645 tests run: 2645 passed、7 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決事項
- 合成 raw の fixture 名は `main-v3-n2-43706.tgz` のまま（実 case の v1 と同じ名前）。`crates/celeris/tests/campaign_lock_e2e.rs` の `K4_INPUT` がこの名前を使い、本 WU の範囲外のため。
- 判断要 26 件（`runner/` の実行コード、`qstat-43722.txt`、`v3-submissions.json` など）は v2 に残っている（audit.md・archive.md）。

## 提案
- 合成 raw の fixture 名を `k4-synth-raw.tgz` 等の実 archive と紛れない名前にし、`campaign_lock_e2e.rs` と campaign の試験の `K4_INPUT` を同じ変更で替える。
