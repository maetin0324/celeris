---
title: 戦略層 v1 子 1 decision-failclosed / revert — 修正の hunk を一時的に戻すと rejected_worker_decision_* が落ち、戻すと通る
tasks: [01M7XSGW4FH258N1K3AMW76Y6F]
status: done
updated: 2026-11-21
---
# decision-failclosed / revert — 修正の hunk を戻す試験

完了日: 2026-11-21

## 結論
- 受け付けなかった worker の決定の要求を `rejected` の決定にまとめる修正（commit「worker の決定: 受け付けられない要求を 1 件の決定にまとめて人に出し、その仕事を止める」と「worker の決定: 問いの文のダッシュを外し、記録する決定の列を WorkerDecisionBatch::recorded にする」）の owner の 2 hunk を worktree で戻した。
- 戻した状態はコンパイルが通り（warning は dead_code の 2 件だけ）、2 試験とも assertion で落ちた（exit=100）。leaf `a`・`integrate-s1` は `Done` まで進み、本番の取りこぼし（task 01M7Q5ZABN9NFQCH6QBTTP3EZ8）と同じく黙って完了した。
- `git checkout -- crates/` で戻すと 2 試験とも通った（exit=0）。最後に `crates/` の差分と未追跡の file が 0 件であることを確かめた。
- 試験の file は変えていない。この unit は production と試験の source を 1 行も変えていない（LoC の差分 0）。

## 戻した hunk（`revert/revert.patch`。1 行追加・12 行削除）
1. `crates/task-core/src/decision.rs` の `prepare_worker_decisions`（605 行から。`if !lost.is_empty() {` で始まる 714〜722 行）: 受け付けなかった要求 `lost` から `rejected_decision` で決定を組み立てて `out.escalated` に入れる block を消した。`escalated` は常に `None` になる。
2. `crates/task-dispatch/src/dispatcher/tree_units.rs` の `Dispatcher::worker_decisions`（362 行から。437 行）: 記録する決定の列 `recorded` を `batch.recorded()`（受け付けた決定とまとめの決定）から `batch.accepted.iter()`（修正前と同じく受け付けた決定だけ）に戻し、その上の D1 の説明の 2 行を消した。

戻した状態のコンパイルの警告（`revert/reverted.log` の 56・72 行）:
- `fields summary, reason, and cost are never read`（`RejectedRequest`）
- `function rejected_decision is never used`

どちらも warning で、コンパイル error ではない（`Finished test profile` が 86 行にある）。

## 落ちた試験と assertion（`revert/reverted.log`、最終行 `exit=100`）
`Summary [   0.577s] 2 tests run: 0 passed, 2 failed, 3292 skipped`

| 試験 | 落ちた場所 | assertion の要点 |
|---|---|---|
| `task-dispatch dispatcher::tests::tree_decisions::rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf` | `crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs:815:5` | `assert!(blocked_by_decision(&units, "a"), "{units:?}")`。leaf `a` が `blocked(decision)` でなく `status: Done, blocked_reason: None` になった。panic の文に出た unit の行では `b`・`integrate-s1` も `Done`。決定は記録されず、要求を出した leaf も統合も止まらなかった |
| `task-core decision::tests::rejected_worker_decisions_become_one_self_decision` | `crates/task-core/src/decision/tests.rs:369:10` | `batch.escalated.as_ref().expect("the rejected ones reach a human")`。`escalated` が `None` で panic。直前の `assert_eq!(batch.rejected.len(), 5)` は通った（拒否の理由は進行の 1 行には残るが、人に届く決定は作られない） |

dispatch の試験は 815 行（run の後の最初の assertion）で落ちたので、その後の decisions の行・受信箱・答えた後の走り直しの assertion には届いていない。

## 戻した後（`revert/restored.log`、最終行 `exit=0`）
- `git checkout -- crates/` の後、`git diff -- crates/ | wc -l` は 0。
- 同じ command の結果は `Summary [   2.158s] 2 tests run: 2 passed, 3292 skipped`、`exit=0`。
- repo の `.config/nextest.toml` は `status-level = "fail"` で、通った試験の名前を出さない。そこで同じ command に `--status-level pass` だけを足した run を `revert/restored-pass.log` に取った。次の 2 行が出て、`exit=0`:
  - `PASS [   0.009s] (1/2) task-core decision::tests::rejected_worker_decisions_become_one_self_decision`
  - `PASS [   2.172s] (2/2) task-dispatch dispatcher::tests::tree_decisions::rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf`

## 証拠のコマンド（この WorkUnit の worktree で実行。基準は `$CELERIS_WU_BASE`）
- 戻しの保存: `git diff -- crates/ > agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/revert/revert.patch`（2 file、1 行追加・12 行削除）
- 戻した状態: `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/rejected_worker_decision/)' > …/revert/reverted.log 2>&1; echo "exit=$?" >> …/revert/reverted.log` → `exit=100`（2 failed）
- 作業ツリーを戻す: `git checkout -- crates/`
- 戻した後: 同じ command を `…/revert/restored.log` に取った → `exit=0`（2 passed）
- 試験の名前の確認: 同じ command に `--status-level pass` を足して `…/revert/restored-pass.log` に取った → `exit=0`（上の 2 試験が PASS）
- 作業ツリー: `git status --short --untracked-files=all -- crates/` → 0 行、`git diff -- crates/` → 0 行
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → `agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/` の下の file だけ
- 進捗の形: `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`

`bash scripts/dev/test-affected.sh`・clippy・fmt は、source を変えていないこの unit では実行していない。提出時の HEAD での実行は close の unit が行う。

## 未解決
- 戻したのは最終の形の 2 hunk だけ。修正の commit の他の部分（`taken` を key と問いの組にした変更、key の重複で問いが違う要求を `lost` に入れる分岐など）は戻していない。試験 file を変えずにコンパイルを通すには、それらが要る。

## 提案
- 無し。
