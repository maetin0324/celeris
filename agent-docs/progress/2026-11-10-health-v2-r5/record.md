---
title: 健全性v2 R5 record — R5 の進捗をまとめ、提案を書いて文書検査を通す
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# 健全性v2 R5 record

完了日: 2026-11-12

WorkUnit `record`（段 record）。基準は `$CELERIS_WU_BASE`（slices-check は `git merge-base HEAD main`）。コードは変えていない。

## 変更
- `agent-docs/progress/2026-11-10-health-v2-r5.md` を足した。task の進捗（front matter の tasks・status・完了日）で、各 leaf の結果の要約、test target と test binary の前後の数、build/link・edit→verify・実行時間・flaky の前後（`baseline.json`・`after-build.json`・`after-run.json` から）、消した試験と mutation の結果、残した target と理由、未解決事項、提案を書いた。
- ADR 2026-11-10-test-target-bundles は変えていない（差 0 行）。1 回目の run は状態の行を「採用・R5 で実装済み」にし、付記「実装との突き合わせ」に 2 行（affected-features の付記の案の写しと R5 の実装の結果の要約）を足したが、celeris が run の後に走らせた範囲 check（許す path は `agent-docs/progress/2026-11-10-health-v2-r5.md` と `agent-docs/progress/2026-11-10-health-v2-r5/` だけ）が `agent-docs/adr/2026-11-10-test-target-bundles.md` を範囲の外として exit 1 にした。2 回目の run で ADR を `$CELERIS_WU_BASE` の内容に戻し、ADR を直す文を task の進捗の「提案」10 に移した（affected-features・bundle-api-b の付記の案もそこから参照する）。
- この file。

## 数値の出典の確かめ方
- HEAD の test target と binary: `python3 scripts/dev/test-target-inventory.py --json` の `totals`（`test_targets` 284、`expected_test_binaries` 311）と crate ごとの値（task-api 64/65・celeris 44/46・celerisctl 14/15・task-worker 17/21）。
- base の test target: `inventory.json` の 352 件（`classify.py --check` の `base_sha … 352`）。
- 束: HEAD の `crates/<crate>/tests/*/main.rs` が task-api 10・celeris 4・celerisctl 2・task-worker 2 の計 18。
- keep・r6 の数と理由: `inventory.json` の `leaf` と `leaf_reasons` の先頭。
- 段の統合で落ちた試験の名前: task の workspace の `integration-checks/` の log の `FAIL` の行（integrate-final の 2 つの log で 3 件）。

## commit（件名）
- phase 1: record: R5 の進捗をまとめ、ADR に付記を足す
- phase 1: record: 提出時の検査の結果を進捗に書く
- phase 1: record: ADR の変更を範囲の外として戻し、付記の案を進捗の提案に移す

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (419 files)`）
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0（`check-doc-layout: ok`）
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0（`write-set R5: ok`）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（352 target、base も 352、18 束、0 error）
- `python3 scripts/dev/test-target-inventory.py --json` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（184 秒、開始時の load average 61.62。`repo_health_gate: all checks passed`、`test-affected: 試験対象の変更なし（docs のみ等）`）
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（77 秒）
- `bash scripts/dev/check-health.sh` は単独では走らせていない。同じ repo health の判定は test-affected の `repo_health_gate` が通った
- 範囲 check（WU の check と同じ command。`sh "$CELERIS_WU_SCOPE_PATHS"` の path から許す path を除く）→ RESULT_SCOPE

## 未解決事項
- task の進捗の「未解決事項」のとおり。この leaf で足した事項は、ADR が R5 の実装の結果をまだ反映していないこと（R5 の WU の範囲 check が ADR を許さない）だけ。

## 提案
- task の進捗の「提案」1〜10 のとおり。10 は ADR を直す文。
