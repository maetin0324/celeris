---
title: 外部 operator の消費を評価記録に取り込む
tasks: [01M5XXF7Q9FFYE8S18VMNQ34MH]
status: done
updated: 2026-10-27
---
# 外部 operator の消費を評価記録に取り込む

完了日: 2026-10-27

## 変更
- `phase 1: 外部 operator の消費を欠測と帰属を保って評価記録に取り込む` — 専用 CLI、冪等な評価 JSONL、区間対応表、保持・証跡手順。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 条件0: `cargo nextest run -p celeris-campaign --test operator_usage` → exit 0、7 passed。4区分・未対応・欠測・重複 snapshot・再取り込み・競合時の原子性・同時取り込み・不正入力を確認。
- 条件1: `test -s docs/ops/operator-usage.md` → exit 0。保持日数延長、session/subagent の写し、digest 検証、対応表と取り込み、訂正時の再生成手順を記載。
- 条件2: `bash scripts/dev/test-affected.sh` → exit 0、2727 passed、9 skipped、対象 doc test 成功。対象は celeris / celeris-campaign / celerisctl / llm-proxy / task-dispatch。
- 条件3: `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check` → exit 0、fmt 差分ゼロ。
- `bash scripts/dev/check-health.sh`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。
- 最初の個別試験ビルドは compile-time CARGO_BIN_EXE が無く失敗。既存 CLI 試験と同じ runtime 環境変数参照に修正し、nextest と必須検査で成功を確認。
- 既存の dagbench_pbs_e2e に unused import 警告1件。必須検査はいずれも成功。外部ネットワーク・LLM・実 browser/userns の試験は実施していない。

## 未解決
実装上の未解決事項なし。消失済み transcript は復元・推定しない。運用時の保全設定は人が実施する。

## 提案
なし。

## 本番への移行
本番 DB の migration は不要。人が測定前に docs/ops/operator-usage.md の保持設定と session 保全を実施する。
