---
title: 健全性v2 G4 measure — fixture の行列で測った精度・誤検出・追加遅延・費用
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-11
---
# 健全性v2 G4 measure — fixture の行列で測った精度・誤検出・追加遅延・費用

完了日: 2026-11-11

`python3 -I agent-docs/progress/2026-11-10-health-v2-g4/measure.py --out <json>` で、3 kind × 16 variant（64 repo）に本物の検査器を各 5 回当てた（全 336 回、exit 0）。検査器・policy・baseline・builder は変えていない。一時 repo は `$TMPDIR` の下だけで、実行後に消えている。

## 数値の要点
| 項目 | 結果 |
|---|---|
| 候補の検出（dup.*） | 8/8、100%。検出のうち期待どおり 8/8 |
| fail 級の検出（deps.violation・policy.changed・history-smuggle） | 48/48、100%。検出のうち期待どおり 48/48 |
| fail 級の適合率 | 48/62、77.4%。差の 14 件はすべて `deps.cycle`（rust 2 repo × 7 variant）。fixture が Cargo の循環を実際に作るので検査器の誤りではない |
| good・security-double・vendor-generated・docs-only の fail 級 | 0 件（profile 有り 16 repo・profile 無し 16 repo） |
| 同じ 4 variant の候補の warning | profile 有り 102 件（うち新規 20 件、`link.unlinked` 16・`dup.contract` 4）。profile 無し 90 件（うち新規 24 件） |
| 検査器 1 回の実時間 | ts 中央値 0.110 秒・最大 0.169 秒。rust 0.106・0.167。multi（2 repo の和）0.289・0.926 |
| python3 の起動（基準） | 中央値 0.013 秒 |
| 検査器の費用 | token 0、0 USD |
| 設計 review・review の LLM の費用 | 未測定（有料の再測定をしない） |

- load average（1 分）は測定前 28.4、測定後 24.5。5 回の結果はすべての repo で一致した。
- `validate_report.py` は report のあるすべての repo で合格。
- 参考: この repo に検査器を 1 回当てた実時間は 147.6 秒（load average 28、1 回だけの測定）。

## 変更
- `phase 1: measure generic health fixtures` — 測定の台本と、この進捗の数値。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 測定の commit の後に実行した。その後で変えたのはこの節の文面だけで、コードは同じ。
- `python3 -I agent-docs/progress/2026-11-10-health-v2-g4/measure.py --out <json>` → exit 0。precision・false_positives・latency・cost の 4 欄と、meta・matrix を出力した。
- `bash scripts/dev/test-affected.sh` → exit 0。Rust の対象 crate は無し（`試験対象の変更なし`）。health は verdict=pass、fail_grade=0、new=0、worsened=0。`test_repo_health*.py` は 94 tests、`OK (expected failures=2)`。`repo_health_gate: all checks passed`。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict=pass、fail_grade=0）。
- `sh scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/progress-index.sh --check` → exit 0。`python3 scripts/dev/check-human-text.py --root .` → exit 0（0 findings）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更は `measure.py` と `measure.md` の 2 path。
- `git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh size-baseline.json scripts/tests/repo_health_fixtures.py` → exit 0（検査器・policy・baseline・builder に差分なし）。
- 全体試験（nextest の全件・doc test・web e2e・実 browser/userns）は実行していない（release gate だけで走らせる規則）。

## 未解決
- 未測定: reviewer の LLM の費用のうち health の材料による増分、`RequiredChecksEvaluated` の時刻の差（ADR D4.3）、profile 無しの検出率。
- 候補の warning が reviewer の判断を変えるかは未確認。
- 実在する OSS の repo での精度は未確認。
