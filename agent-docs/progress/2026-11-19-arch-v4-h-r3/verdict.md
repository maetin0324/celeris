---
title: アーキ v4 H R3 verdict — review_verdict.rs と work_units.rs は ADR の D6 どおり変えない
tasks: [01M7THYAPKESR1BG6BQZPJ749P]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# verdict（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- [ADR 2026-11-19 dispatcher-narrow-ops-round2](../../adr/2026-11-19-dispatcher-narrow-ops-round2.md) の D6 は
  `review_verdict.rs` と `work_units.rs` を **R3 では実装しない** と決めた。この WorkUnit は決定どおり 2 file を変えていない。
- 変えた行: 無し。使った既存の操作: 無し。この WorkUnit の変更は、この記録（`verdict.md`）を足しただけである。
  `crates/`・`scripts/`・`web/`、`structure_guard.rs`・`cex_*.rs` は変えていない。
- `sh agent-docs/progress/2026-11-19-arch-v4-h-r3/measure.sh shrink --allow-same <2 file>` は exit 0
  （非試験 file 138 → 138、2 file の E1 の store＋config 116 → 116）。

## 1. 残した参照と理由（ADR の節「review_verdict.rs と work_units.rs の設計と決定」）

参照は全部残した。数は C3（1 行の中の字面）。base（[base.txt](base.txt) の commit）と作業ツリーで同じ。

| file（行数） | C3 store | C3 config | E1 store | E1 config |
|---|---|---|---|---|
| `crates/task-dispatch/src/dispatcher/review_verdict.rs`（869） | 32 | 3 | 33 | 3 |
| `crates/task-dispatch/src/dispatcher/work_units.rs`（2053） | 58 | 14 | 65 | 15 |

### review_verdict.rs（ADR の小節「review_verdict.rs（store 32・config 3）」）

関数ごとの C3（行は関数の宣言の行）: `on_review_finished`（23 行）store 28・config 2、`try_review_repair`（568 行）store 2・config 1、
`try_integration_repair`（686 行）store 1、`apply_review_repair`（754 行）store 1。

残した理由（ADR の要約）:

- `on_review_finished` は (1) review lock（supervisor の review の entry を外す）→ (2) 部長の session の記録 →
  (3) runs 行の確認（`run_index_get`）→ (4) 計画の開き直しの確認（`work_units_for` と `next_work_unit`）→ (5) 判定の反映、の順で動く。
  (3) と (4) の前後が stale review・restart・replan の反例 4 本の本体である。
- 1 回化の候補は reviewer の基盤の失敗と再投入の数を読む 2 つの `self.store.events_for(task_id)`（排他の 2 分岐の同じ読み）だけで、
  減る見込みは store −1。ほかの 30 の参照は別の読みか別の書き込み（`append_event`・`apply_transition`・`apply_transition_with_events`・delivery）で、
  まとめると transaction の範囲か書き込みの順序が変わる（ADR の D4）。
- 判断を純粋関数へ移す形は出現を移すだけで減らさない（H が成果に数えなかった形）。
- 決定の理由: 減る見込み store −1 が、反例 4 本の本体の関数に触れる risk に見合わない。並行の `cos`・`tree` と同じ段に入り、
  review の順序の変更を同じ統合で見分けにくい。
- 次の task で実装する条件（ADR から写す）: (1)〜(5) の順序を固定する試験（反例 4 本に加え、(3) と (4) を入れ替えると落ちる試験）が先にあること、
  1 回化で減る出現が 3 以上あること、変える行が 2 つの `events_for` の分岐の中に収まること。

### work_units.rs（ADR の小節「work_units.rs（store 58・config 14、本体 2053 行）」）

関数ごとの C3 の主なもの: `wu_dispatch_gate`（400 行）store 10、`spawn_work_unit_checks`（13 行）store 3・config 4、
`deferred_work_unit_trigger`（1116 行）store 5、`reconcile_parallel_tasks`（1279 行）store 5、`start_work_unit_run`（1434 行）store 4・config 1、
`work_unit_prompt_context`（1523 行）store 4・config 1、`execution_gate_if_needed`（1664 行）store 1・config 3、
`prepare_work_unit_workspace`（788 行）config 1。`commit_work_unit`（1036 行）は 0。残りの 18 関数が store 0〜3・config 0〜2 を持つ。

残した理由（ADR の要約）:

- 本体の行が閾値 2000 を超えているので、repo health の `size.responsibility-growth` と source-size が 1 行の増加でも落ちる。
- WorkUnit の遷移（`wu_dispatch_gate`・`deferred_work_unit_trigger`・`reconcile_parallel_tasks`）と commit の副作用
  （`spawn_work_unit_checks`・`record_check_hand_off`・`take_over_handed_off_checks`・`prepare_work_unit_workspace`・`commit_work_unit`）を分けるには、
  新しい file（分母の増加。成果に数えない）か行の増加と、200 行を超える関数の分割が先に要る。
- 遷移と副作用の本体は lease・restart・並列 WU の反例 4 本が守る範囲に直接入る。
- 決定の理由: 分離は R3 の数え方では成果にならない。行を増やせない file で 1 回化の候補を探す作業は、遷移の分離の設計と別の task の方が審査の範囲が狭い。
- 次の task で実装する条件（ADR から写す）: 遷移の純粋関数の置き場（既存の `task_core` の WorkUnit の module か `ExecutionCoordination`）を ADR で決めること、
  本体の行を減らす方向だけで変えること、反例 4 本と `structure_guard_*` が通ること、分母を増やす分は成果に数えないと先に書くこと。

## 2. 反例試験

ADR の表の 12 本が `crates/task-dispatch/src/dispatcher/tests/` の下にあることを
`git grep -l "fn <名>\b" -- crates/task-dispatch` で確かめた（`cex_lease_restart.rs` 8 本、`cex_review_authority.rs` 1 本、`cex_late_completion.rs` 3 本）。
この WorkUnit は試験を足さず、消さず、弱めていない。

## 3. 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、measure.sh の基準は base.txt の commit）

- `sh agent-docs/progress/2026-11-19-arch-v4-h-r3/measure.sh shrink --allow-same crates/task-dispatch/src/dispatcher/review_verdict.rs crates/task-dispatch/src/dispatcher/work_units.rs`
  → exit 0。出力: `shrink: ok files 138 -> 138 paths 2 file store+config 116 -> 116`
  （crate の非試験 file の集合が同じで、どの file も E1 の参照が増えていない。2 file の E1 の合計は 33＋3＋65＋15＝116 のまま）。
- `git diff --stat $CELERIS_WU_BASE -- crates scripts web` → 空。`sh "$CELERIS_WU_SCOPE_PATHS"` → この `verdict.md` の 1 path だけ。
- `bash scripts/dev/test-affected.sh` → exit 0（`試験対象の変更なし（docs のみ等）`。repo-health は verdict=pass、doc.links・adr.numbers・progress.index は pass）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。`bash scripts/dev/check-doc-links.sh` → exit 0。

## 未解決

無し。2 file の実装は ADR の「次の task で実装する条件」を満たす別の task で行う。
