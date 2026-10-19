---
title: "root/子/孫/retry の各試行で凍結条件を照合"
tasks: [01M59W8DFMVT3M8WRM4XP8ZEWE]
unit: per-run-conditions
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# per-run-conditions

`scripts/dev/jev-live-evidence.py`: arm 和集合の候補検査を外し、各 B/C trial の `proposed_candidates` を凍結 manifest の Sonnet/Opus 共通候補（source_id/model_id/harness_id の 2 件。重複・未知 ID・空・単一・別 harness/provider は拒否）と個別に照合。`proposed_first` も凍結候補内を要求。実効 pin は source/lower_policy/evidence_mode/evidence_table_id/online_update を root と一致させ、B/C 全試行で mode 以外同一を要求（`condition-mismatch`）、online_update=false 必須。実コマンド引数は不変。

試験 `scripts/dev/tests/jev_live_evidence/test_per_run_conditions.py`: reviewer 再現（C1 r2=Sonnet 単一+exec Sonnet）の専用回帰、r1〜r4・B 側へのパラメータ化負例、pin 欄の負例、原記録と export を整合させた改変で出自エラーが混ざらないこと、export のみ改変は照合で落ちること。

## 証拠
- `python3 -m unittest discover -s scripts/dev/tests/jev_live_evidence` → 87 件 OK
- `... -p test_per_run_conditions.py` → 10 件 OK

## 未解決・提案
- 凍結候補は読取元 DB が manifest 本文を持たないため script 内定数（`FROZEN_CANDIDATES`）。本文との自動照合は別途。実測・Rust 変更なし。
