---
title: R5 pick-g4 — G4 の review 完了待ちの取り込み
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
---
# R5 pick-g4 — G4 の review 完了待ちの取り込み

完了日: 2026-11-11

## 実装前 brief
1. 根本原因: review の必須検査は tick の外で進み、固定 60 tick では review 結果が揃う前に待ちが終わる。
2. 責務 owner: task-dispatch の dispatcher/tests/git_workspace.rs（地図の worker 起動・完了処理）。
3. 既存の再利用先: G4 の commit と既存の run_until_state・STATE_WAIT_GUARD。
4. 選択肢: 独自修正はせず、指定 commit を cherry-pick -x。今回の Objective に従い main と resync-g4 は merge しない。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 対象試験の固定 tick 回数による review 待ち。
7. 不変条件: review の結果を tick の回数でなく終端状態・証拠という出来事で待つ。既存の成功・失敗の assert を維持し、git_workspace の試験で確認する。試験の削除・新設は無い。

## 変更
- `phase 1: wait for terminal state and evidence in git_workspace review tests` を G4 の branch から cherry-pick -x。衝突なし。git_workspace.rs だけの変更。
- `phase 1: record G4 cherry-pick verification for R5` — この進捗と検査結果を記録。
- main の merge・rebase・checkout は実施していない。
- R2・R3 は既存の base に含み、R4 は含まない。R4 後の調整は R6。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 条件0: 対象関数の本体を抽出して `run_until_idle(&mut d, 60)` が無いことを assert → exit 0。
- 条件0: `cargo nextest run -p task-dispatch -E 'test(dispatcher::tests::git_workspace::)'` → exit 0、23 passed・1638 skipped、実行10.864秒。
- 条件1: 元commitの差分と取り込み後の差分を Python で byte 比較 → 一致。`git rev-list --merges "$CELERIS_WU_BASE"..HEAD` → 出力なし。最終 scope は下記。
- 条件2: `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf adjust-api agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0、4 moved・266 keep/r6・error 0。`--verify-leaf adjust-worker` → exit 0、6 moved・266 keep/r6・error 0。
- 条件2: `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-api --base "$CELERIS_WU_BASE"` → exit 0、試験関数856→856、表757→757、不変条件229件、removals 0。`--crate task-worker` → exit 0、試験関数1292→1292、表1106→1106、不変条件564件、removals 0。
- 条件3: `bash scripts/dev/test-affected.sh --list` → exit 0、celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch の5 crate。
- 条件3: `bash scripts/dev/test-affected.sh` を run 内で foreground 実行 → exit 0。全体858.656秒。nextest は3712 passed・11 skipped（11 slow）、304.917秒。doc test は3 passed・2 ignored・失敗0。health gate は verdict=pass、policy.changed findings=0。検査器自己試験78件合格。loadavg（1/5/15分）は開始95.24 / 84.23 / 79.09、終了75.95 / 74.86 / 74.39。
- 条件3の制限: control-ts の offline install が失敗し、既存スクリプトは bundle を渡さず続行した。bundle を必要とする内部 SKIP が59件（nextest の11 skippedとは別）。検査器・対象規則を変更していない。失敗試験は無いため `$CELERIS_WU_BASE` での再現確認は不要。
- 条件4: `cargo fmt --all -- --check` → exit 0。

- 条件4: `cargo clippy --workspace -- -D warnings` → exit 0、1分35秒。
- 条件4: `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → 各 exit 0。
- 条件1・4: `sh "$CELERIS_WU_SCOPE_PATHS"` と `git diff --name-only "$CELERIS_WU_BASE" HEAD` → git_workspace.rs とこの進捗だけ。
- 条件4: 上記差分を B0 `slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` に渡す → exit 0、write-set R5: ok。

## 未解決事項
失敗なし。bundle 内部 SKIP 59件という検証範囲の制限は上記のとおり。

## 提案
無し。
