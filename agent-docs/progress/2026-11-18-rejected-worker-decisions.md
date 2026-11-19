---
title: 受け付けられない worker の決定の要求を 1 件の決定にまとめて人に出す
tasks: [01M7RFVV60RQTDKYEWAEH4YYGT]
status: done
updated: 2026-11-19
---
# 受け付けられない worker の決定の要求を 1 件の決定にまとめて人に出す

完了日: 2026-11-19

## 結論
worker が `result.json` の `decisions` で出した要求を daemon が受け付けられないとき（形の誤り・読めない要素・計画に無い unit
〈integrate の unit 名を含む〉・問いの違う key の重複）、要求を 1 件の `choice` の決定 `rejected` にまとめて記録し、要求を出した
仕事（leaf / atomic の節点）を答えが出るまで止める。決定は受信箱の未回答の決定と通知に出る。答えると leaf が走り直し、前置きの
「人の決定」節に答え（元の問いと拒否の理由を含む）が入るので、worker にも届く。方式は (a)。理由は
`agent-docs/adr/2026-11-18-rejected-worker-decisions-reach-human.md`（ADR-0079 R3a 11. の「捨てて run は失敗させない」を置き換える）。

## 変更（commit 件名）
- adr: 受け付けられない worker の決定の要求を 1 件の決定にまとめて人に出す（実装の前の commit）
- worker の決定: 受け付けられない要求を 1 件の決定にまとめて人に出し、その仕事を止める
- worker の決定: 問いの文のダッシュを外し、記録する決定の列を WorkerDecisionBatch::recorded にする
- 進捗: rejected-worker-decisions（この file）

変えた file:
- `crates/task-core/src/decision.rs`: `prepare_worker_decisions` の `taken` を「key → 記録済みの問い」に変え、拒否した要求を
  `WorkerDecisionBatch.escalated`（key `rejected`、衝突すれば `rejected-2` …、`needed_before` = self）にまとめる。
  `REJECTED_KEY`・`WorkerDecisionBatch::recorded`・`rejected_decision`・`spec_summary`（束ねと共用）・`free_key`・`max_cost` を足した。
- `crates/task-dispatch/src/dispatcher/tree_units.rs`: `worker_decisions` が `batch.recorded()`（受け付けた決定とまとめ）を記録し、
  self の止めと他の unit の止めを判定する。
- `crates/task-worker/src/preamble.rs`: 決定の要求の節に「integrate の unit は指せない」「受け付けられない要求は人に回り、この仕事が止まる」を足した。
- 試験: `crates/task-core/src/decision/tests.rs`、`crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs`。

## 受け入れ条件ごとの証拠（提出時の HEAD で実行）

### 0. 実装の前の commit で ADR
- `agent-docs/adr/2026-11-18-rejected-worker-decisions-reach-human.md`。事象・原因・(a) を選んだ理由（(b) を選ばない理由）・
  needed_before を読み替えない理由（D4）・重複の扱い（D3）・試験・ADR-0079 D7 / R3a 11. との関係を書いた。commit の順は
  「adr: …」が実装の commit より前。

### 1. 本番と同じ入力で何が変わるか
- 試験 `rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf`: leaf `a` が `needed_before: ["integrate-s1"]` の要求を出す。
  - decisions の行: `rejected` の 1 行（open・origin worker・run id = a の run・`needed_before = ["a"]`、問いに元の問いと選択肢と
    `needed_before names an unknown unit "integrate-s1"`）。修正の前は 0 行。
  - 受信箱: `task_ops::decision::open_rows`（受信箱・通知の対象）にこの 1 件が出る。
  - run の結果: leaf `a` は done にならず `blocked(decision)`、`integrate-s1` は done にならず、root は ready のまま。修正の前は
    a が done、木が done まで進んだ。
  - 答えた後: a が 2 回目の run をし、前置きの「人の決定」に `- rejected worker が出した決定の要求 1 件…` の行が入り、
    integrate と root が done になる。replay は一致。
- 既存の期待値: `worker_decisions_are_validated_and_self_is_resolved`・`worker_decisions_beyond_the_cap_are_bundled` の
  `accepted`・`rejected`・`bundled` の期待値は変えていない（`taken` の型を集合から表に変えた引数だけを直した）。
  `worker_declared_decisions` は形の誤りの要素 `{"key":"broken"}` が「捨てられる」ことを確かめていた。この契約は ADR D1 で
  廃止したので、その要素と確かめ（決定の数の注記と進行の 1 行の確かめ）を外した。受け付けた決定 w1・w2 の期待値は変えていない。
  新しい期待値は上の試験が確かめる。

### 2. 決定的な試験と失敗注入
- 試験は偽のアダプタと in-memory の SQLite だけを使い、sleep の長さや CPU の負荷に頼らない（既存の `run_until_quiet` と同じく tick を回す）。
- 失敗注入 1（task-core）: `out.escalated = Some(escalated);` を外して
  `cargo nextest run -p task-core -p task-dispatch --lib --no-fail-fast -E 'test(/rejected_worker_decision/)'` → 2 件とも FAIL
  （`decision/tests.rs` の `expect("the rejected ones reach a human")`、`tree_decisions.rs` の `blocked_by_decision(&units, "a")`）。
- 失敗注入 2（dispatcher）: `tree_units.rs` で記録する列を `batch.accepted` だけにして
  `cargo nextest run -p task-dispatch --lib -E 'test(/rejected_worker_decision/)'` → 1 件 FAIL（`blocked_by_decision(&units, "a")`）。
- どちらも元に戻し、戻した後に下の 3. の試験が通った。

### 3. 検査（提出時の HEAD で実行）
- `cargo nextest run -p task-core -p task-dispatch --lib -E 'test(/decision/)'` → exit 0、116 件 pass（新しい 2 件、`tree_decisions` の
  10 件、`decision::tests`、`run_settlement` の worker の決定の試験を含む）。
- `cargo nextest run -p task-dispatch --lib -E 'test(/tree_decisions/)'` → exit 0、10 件 pass。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 8473 件 pass・22 skip、web の型検査と unit 試験 705 件 pass、
  repo-health verdict=pass new=0 worsened=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 途中の 1 回目の test-affected は exit 1 だった: human-text（新しい問いの文のダッシュ）と size.responsibility-growth
  （`crates/task-dispatch/src/dispatcher` の module の行数の増加）。ダッシュを句点にし、記録する列を task-core の
  `WorkerDecisionBatch::recorded` に移して進行の 1 行を足すのをやめ、dispatcher の行数を base と同じにした。

## 未解決事項
- `followups.json` の形の誤り（2026-11-16、01M7H1RNG5）は範囲外（ADR D5）。同じ型の取りこぼしが残る。
- 本番の 01M7Q5ZABN9NFQCH6QBTTP3EZ8 の過去の事象は直さない（人が事後に承認済み）。

## 提案
- `followups.json` の拒否も同じく人か worker に届く形にする task を起こす（決定と同じく、拒否の理由を受信箱か次の run に載せる）。

## 本番への移行
- config・DB の変更は無い。release で入る。
