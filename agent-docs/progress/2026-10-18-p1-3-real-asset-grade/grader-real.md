---
task: 01M56HCNTPN0H2VGP4JE0JYRM9
wu: grader-real
status: done
completed: 2026-10-18
---
# grader-real: K2 実資産の正負例 grader 検証と K4 実 raw 再計算

> 注（履歴）: 本書の実走は旧 known-good（p0-final.patch 単独）で、原因は by-ID 経路の欠落（tree 側）だった。最新結論は `../2026-10-18-p1-3-known-good-fix.md`（k2-fix.patch, k2-known-good-v2）。

## 変更
- `crates/campaign/src/grader.rs`: `CheckResult.env_failure` と `GradeReport.failure_class`（`model_failure` / `env_failure`、不合格時のみ）。
  env_failure = grader 側 asset（pristine/隠しテスト）の欠落・改変、評価 command が起動不能、出力に build 環境不良の印
  （system library/linker/依存取得/pkg-config 等）があり、成果物の失敗（`... FAILED`・`error[E`・`panicked at`）が無いもの。
  timeout は成果物の暴走もあり得るので env にしない。
- `grade_out.rs`: grades.jsonl の各行に `failure_class`。
- `crates/campaign/scripts/verify-real-assets.sh <asset_dir> [--build]`: 実物検証（無 LLM）。SHA256SUMS 照合 → bundle から固定 commit → submodule tar → lock 照合 → known-good patch 適用 → 正負の印 → K4 raw の hash と再計算。`--build` で実 cargo。
- `crates/campaign/tests/real_asset_grade.rs`: fixture の env/model 分離、K4 合成 raw の再計算（一致/6% ずれ/pin 違い）、lock 不一致、実物検証（CAMPAIGN_ASSET_DIR 設定時のみ）。

## 証拠
- `cargo test -p celeris-campaign --test real_asset_grade`: 9 件 ok（fixture）。
- `CAMPAIGN_K2_FIX_PATCH=<p0-final.patch> CAMPAIGN_K4_TGZ=<main-v3-n2-43706.tgz> sh crates/campaign/scripts/verify-real-assets.sh /local/celeris/data/campaign-assets/k2`: 実 asset で組み立て・lock 一致・patch 適用・K4 再計算が ok。
- 同 `--build`: 実 build は env_failure と分類して停止（下記）。モデル失敗とは混ざらない。

## 未解決事項
- 実 build は `async-ucx`（pluvio の入れ子 submodule `lib/pluvio/pluvio_ucx/lib/async-ucx`）が asset の pluvio tar に無く、crates.io 取得も無いため失敗する。UCX 本体も要る。
  人手順: asset store に入れ子 submodule の snapshot（と UCX/ vendor 済み依存）を追加し lock に固定 → `--build` で正負が通るまで確認。
- known-good 用 patch（P0 の `p0-final.patch`）は asset store に無い。`k2-fix.patch` として置くか `CAMPAIGN_K2_FIX_PATCH` で指す。
- 本 task では lock の再封印はしていない（grade-case/入力は変更なし）。今後 grader を変えたら `grade_case_ref` と campaign lock を新しい測定 ID で作り直す（旧 lock では採点が拒否される。試験済み）。
## 提案
- 実 build の可否は A sandbox 同一経路の cap-probe 側で確認し、ここでは env_failure として分離できることまでを保証した。
