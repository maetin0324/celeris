---
title: "main-4 pilot 実行票（K4 A0→B0、通過後に K2 A0/B0）"
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: done
updated: 2026-10-25
---
# main-4 pilot 実行票（K4 A0→B0、通過後に K2 A0/B0）

完了日: 2026-10-25

WorkUnit `pilot-ticket`。基準は `$CELERIS_WU_BASE`。成果は `docs/ops/mlsys-main-4-pilot-runbook.md` だけで、実装は変えていない。
pilot の LLM 試行は実行していない（人の指示）。

## 内容
- 順序・構成・固定値（measurement_id `mlsys-main-4-v3` と各 sha256）は `artifacts/main4-ready/freeze-report.json` から写した。停止条件・出力先・共有台帳・既承認枠と残枠・人操作の記録・開始可否の節を書いた。
- 共有台帳 `/local/celeris/data/measure/main-4/global-ledger` は未初期化（dir が無い）。main-4 の既承認枠・残枠は承認記録が無いので「未取得」と書いた。過去消費として main-2 の global-ledger（1,359,694 tokens・2 試行）と main-3 の out ledger（K4 A 1,192,762 tokens、B は欠測）を参考に載せた。
- 開始可否は「開始不可」。条件は C1〜C8（RELEASE の昇格、A の login、OAuth と env-lock、sirius の ControlMaster と cap-probe、承認の記録と台帳の初期化、実 preflight、K4 先行の manifest、新 revision での件数照合）。

## 未解決事項
- 現 freeze の manifest は `problems` を整列して K2 を先に計画し、`approved_trials = 2` なので K4 に届かない。K4 先行には K4 だけ・K2 だけの pilot manifest（同じ shared_ledger）を作り、lock を作り直す必要がある（実行票 C7）。

## 証拠（提出時の HEAD で実行）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0、`sh scripts/dev/check-doc-links.sh` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし、docs のみ）
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0

## 提案
- pilot 用 manifest を問題ごとに分ける手順を freeze の段で標準にする（主測定でも順序を明示できる）。
