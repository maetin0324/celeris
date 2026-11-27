---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: close-verify
status: done
updated: 2026-11-27
completed: 2026-11-27
---
# 戦略層 v1 子 8 close-verify: 提出時の HEAD で試験と検査を流し直し、task の進捗を done にする

## 結論
- close-audit の commit の後の HEAD で、objective に挙がった 11 個の command を流した。すべて exit 0。
- 子 8 全体の試験を流すため、`git merge-base HEAD main` を基準にした `test-affected.sh` も流した（exit 0、nextest 7,144 件 pass）。
- 全部 exit 0 だったので、task の進捗 [2026-11-24-strategy-v1-8.md](../2026-11-24-strategy-v1-8.md) の front matter を `status: done`・`completed: 2026-11-27` にした。
- 採点の結果・判定の本文（結論・採点の結果・機械の軸・指標・評価器の質など）は変えていない。

## 変更（commit の件名）
- `strategy-v1 8 close-verify: 提出時の HEAD で試験と検査を流し直し、task の進捗を done にする`
  - `agent-docs/progress/2026-11-24-strategy-v1-8.md`: front matter（status・completed）、冒頭の状態の行、「現在地」、「WorkUnit と commit」に close-verify の行、「## 検査（提出時の HEAD で実行）」の節を足した。
  - `agent-docs/progress/2026-11-24-strategy-v1-8/close-verify.md`: この file。
- crates・web・control-ts・scripts・試験・testdata・ADR・root の artifacts は変えていない。

## 流した command と結果（repo の root。`ART=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts`）
- `python3 -I scripts/dev/parent-eval-blind/pilot_score.py check-scored $ART/pilot-scoring.md --decision agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json` → exit 0（出力 0 行）。
- `python3 -I -m unittest discover -s scripts/dev/parent-eval-blind -p 'test_*.py'` → exit 0、81 件 OK。
- `python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/replay/replay.py verify` → exit 0、12 slices・漏れ 0、否定対照 N1・N2・N3 を検出、`verify: ok`。
- `cargo nextest run -p task-api --test parent_checkpoint_live` → exit 0、6 件 pass・4 件 skip（`#[ignore]` の seed・render・sample・live の検査）。
- `cargo nextest run -p task-dispatch -E 'test(/parent_evaluation_input_/) | test(/undeclared_nested_name_/)'` → exit 0、7 件 pass。
- `bash scripts/dev/test-affected.sh`（WU の基準 `$CELERIS_WU_BASE`）→ exit 0。基準と HEAD の差分が無く「試験対象の変更なし」、repo-health は verdict=pass・fail_grade=0。
- `CELERIS_WU_BASE=$(git merge-base HEAD main) bash scripts/dev/test-affected.sh` → exit 0。crate 9 個（celeris・celeris-campaign・celeris-mcp・celerisctl・llm-proxy・task-api・task-dispatch・task-ops・task-worker）の nextest 7,144 件 pass・33 件 skip、script bundle の SKIP 0 件。web は tsc -b が通り、vitest 108 file・773 件 pass、node --test 110 件 pass。repo-health は verdict=pass・fail_grade=0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/check-adr-numbers.sh` → exit 0（482 files）。`sh scripts/dev/progress-index.sh --check` → exit 0。
- root の artifacts の `pilot-scoring.md` はある（sha256 `17b5d734fd764f29979bb108719dbd69a7ff9314d3cd9bfe7b50b4af402bf7ee`）。

## 未確認
- この commit（`agent-docs/progress/` の 2 file だけ）の後には、子 8 全体の test-affected を流し直していない。ほかの command は commit の後に流し直し、結果を task の報告に書く。
- そのほかの未確認は task の進捗の「未確認の事項」のまま。

## 提案
- 無し（task の進捗の「提案」のまま。子 9 の採点の分担は人の確認待ち）。
