---
title: script-compare.py report と本番手順の進捗ファイル
tasks: [01M6J81DZ7C8SMY7HNZTW8GT0Z]
status: done
updated: 2026-11-04
---
# script-compare.py report と本番手順

完了日: 2026-11-04

## 変更
- `report: script-compare.py report を網羅の証拠と 100 件に合わせ、本番への移行を書く` — `scripts/dev/script-compare.py`（`exhaustive` 節・standing・required 100・`--lane-reference-sha256`・table 表示）、
  `scripts/dev/tests/script-compare.sh`（網羅の試験 13 件）、`agent-docs/progress/2026-11-04-lane-rules-exhaustive.md`（完了状況と「本番への移行」節）。
- worker は本番の daemon・DB・config・release に触れていない。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `bash scripts/dev/tests/script-compare.sh` → exit 0（`script-compare: ok`）
- `bash scripts/dev/test-affected.sh` → exit 0（変更は scripts/dev と進捗ファイルだけで、script-compare.sh を含む）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → ok、`python3 scripts/dev/check-human-text.py` → 0 findings
- 範囲 check（`$CELERIS_WU_SCOPE_PATHS`）→ 変更 path は上の 4 file だけ

## 未解決事項
- 本番の網羅の所要時間は未確認（operator が手順で記録する）。
