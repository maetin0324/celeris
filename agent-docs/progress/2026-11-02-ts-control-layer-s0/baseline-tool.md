---
title: S0 baseline-tool — edit→activate 計測 script と policy の baseline
tasks: [01M6DCPKPJZZ9F9J4QJ47FH8KK]
status: done
updated: 2026-11-02
---
# S0 baseline-tool — edit→activate 計測 script と policy の baseline

完了日: 2026-11-02

## 変更
- `scripts/dev/edit-activate-baseline.sh` を追加（`--scenario policy|cos --mode warm|cold --reps N --out`、`--dry-run`、`--validate`）。
- `baseline/policy.jsonl`（warm 3 試行 + prime 1 周 + cold 1 試行 + release 記録の推定行）と `baseline.md`（要約表）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `--dry-run --scenario policy` / `--scenario cos` → exit 0
- warm 3 試行 → exit 0（全 18 段 exit 0、約 16 分）。cold 1 試行 → exit 0（6 段 exit 0）
- `--validate baseline/policy.jsonl` → exit 0（130 行、problems 0）。欠けた行を入れた写し → exit 1
- 実行後の `git status --short` に crates/ の差分なし

## 未解決
- cos scenario の実測は WorkUnit baseline-cos。cos の編集（job_wait_extend::validate の分岐）は check・clippy（`-p task-api`）が exit 0 で通ることだけ確かめた（rep 1 回、記録は残していない）。

## 提案
- sccache server が動いている状態の cold も別に測ると、cache の効果を分けられる。
