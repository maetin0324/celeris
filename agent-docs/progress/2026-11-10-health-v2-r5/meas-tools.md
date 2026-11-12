---
title: R5 再計測の負荷条件と集計
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
---
# R5 再計測の負荷条件と集計

完了日: 2026-11-12

## 実装前 brief

1. 根本原因: compare は負荷を記録するだけで比較への採否を判定していなかった。
2. owner: measure.sh と measure_summary.py。製品の試験・健全性の検査器は変更しない。
3. 再利用: 既存 compare の build・edit_verify・その側の test-affected 呼び出し、旧形式の validate。
4. 選択: 計測の手順を維持し、比・除外・require・summary の判定を Python に集約する。
5. 新状態: 全試行と各段の負荷待ちの記録。fallback・port は無し。
6. 撤去: 公開 compare の /1 出力を /2 に置換。過去の原票の /1 検証は保持する。
7. 不変条件: 比 2 以上、欠測、失敗、待ち期限超過を採用しない。fixture で境界と逆方向、再試行、表の byte 一致を確認する。

## 変更

- `phase 1: meas-tools: 負荷待ちと比較の除外・交互実行・表生成を追加する`
- compare は各段の前に既定 24 以下まで最長 600 秒 sleep で待つ。組が揃わなければ既定 2 回再試行し、全回を保存する。
- run-compare は base・HEAD・HEAD・base の順に実行する。共有 target のため各実行の直前にその側の --no-run を行う。celeris は campaign の binary も build する。
- summary は除外を含む全試行の表を生成する。require は除外されていない段が各 crate に揃うことを検査する。
- flaky は結果が異なる同じ試験名を記録する。束ねで増えた module の suffix が旧 target と一意に対応すれば旧名に照合する。対応が不明な名前の欠落だけでは flaky にしない。
- MEASURE_MAX_LOAD・MEASURE_LOAD_WAIT・MEASURE_RETRIES を指定できる。

## 証拠

提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`。この leaf では Cargo の計測を実行していない。

- `python3 agent-docs/progress/2026-11-10-health-v2-r5/test_measure_summary.py` → exit 0、13 件合格。Cargo と git の実行を mock にした交互実行・再試行の試験を含む。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/after-build.json` → exit 0、4 crate、両側 build 4 件、skipped 1 件。
- `bash scripts/dev/test-affected.sh` → exit 0。健全性 pass、fail 級 0 件。検査器の試験と gate 合格。Rust の試験対象は無し。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check`、`bash scripts/dev/check-doc-links.sh` → exit 0。
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0、write-set R5: ok。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、変更は本 leaf の 4 file のみ。
- `git diff --check`、`bash -n agent-docs/progress/2026-11-10-health-v2-r5/measure.sh` → exit 0。

## 未解決

無し。

## 提案

無し。
