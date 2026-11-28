---
tasks: [01M7XQ9PS0AFKKMRJ7P5ZMKQQE]
wu: r1-ts
status: done
updated: 2026-11-28
completed: 2026-11-28
---
# 戦略層 v1 親 r1-ts: control-ts の Project Controller に規則 pc/r1-stall を足す

## 結論

- `control-ts/src/domains/project_controller.ts` に型 `SnapshotStall`、`SnapshotCheckpoint.stall?: SnapshotStall | null`、規則 `pc/r1-stall` を足した。
- `stall.escalate` が真の最初の checkpoint について、`escalateOnce("r1_stall:<root id>:<terminal_children>", "explicit_human", 問い, "pc/r1-stall", [<strategy_ref>])` を返す。問いには終端の子の数・no-progress の連続・K・未到達の Outcome（最大 16 件、超えた分は「ほか N 件」）を書く。
- 順は `pc/no-progress` の後、checkpoint の写し（unmapped の mapping）の前。mode は TS で見ない（shadow で効果を抑えるのは Rust の門）。
- objective に無い条件を 1 つ足した: root が満たしたが到達を記録していない Outcome がある間（規則 4 の `pc/milestone-reached`）と、TS の gap に未到達が無いとき（規則 5 の `pc/achieved`）は R1 を出さない。理由は「実装前 brief」の 4。
- Rust・schema・生成型・golden（`control-ts/fixtures/`）は変えていない。本番昇格・release.sh はしていない。

## 実装前 brief

1. 根本原因: Rust（WU r1-core）は snapshot の checkpoint に `stall` を載せるが、TS の `SnapshotCheckpoint` に欄が無く、R1 の escalate を判断に写す規則も無い。
2. 責務 owner: `control-ts/src/domains/project_controller.ts` の `decide`（方針の判断）。R1 の値（終端の子の数・連続・escalate）の owner は Rust の `parent_checkpoint::stall` で、TS は数え直さない。
3. 既存の再利用先: `escalateOnce`（`pc/no-progress` と同じ 1 回だけ聞いて後は待つ経路）、`gap.open`（未到達の Outcome）、規則 4 の「到達したが未記録」の判定、試験の `cp`・`strategySnapshot`・`decision`・`evaluation`。
4. 選択肢:
   - (a) objective の文どおり、`stall.escalate` が真なら必ず聞く。これだと、root が Outcome を満たしたが到達を記録していない間にも聞く。Rust の到達は記録（Roadmap の `reached`・`mark_milestone` の判断）から数えるので、記録の前は連続が戻らない。R1 は規則 4 より前にあるため、2 回目からの待ち（`pc/r1-stall/waiting`）が記録を塞ぎ、子がさらに終端になるまで記録も起票も進まない。全部の Outcome を root が満たした後も、記録が無ければ同じく `pc/achieved` に進めない。
   - (b) (a) に「到達を記録していない Outcome が無い」と「gap に未到達がある」の 2 条件を足す → 採用。記録が先に進み、記録の後の snapshot で Rust が連続を戻す。問いに書く未到達の Outcome が空になることも無い。
   - (c) R1 を規則 4 の後に置く → objective の順（mapping の前）と合わない。写していない評価がある間は Rust が milestone を `checkpoint_action_mismatch` で落とすので、mapping の後に置いても同じ問題が残る。採らない。
5. 新しい状態・分岐・fallback・port: 状態は無し。分岐は規則 `pc/r1-stall` の 1 つ。規則 4 の「到達したが未記録」の判定を、規則 4 と R1 で共有するため関数の先頭の `unrecorded` に移した（判定は 1 か所のまま。規則 4 の挙動は変わらない）。定数 `POLICY.stallOutcomesListed = 16`（Outcome は最大 64 件。summary を Rust の `LONG_TEXT_MAX` 2,000 字に収める）。
6. 撤去対象: 無し（置き換える旧い仕組みが無い）。
7. 不変条件と試験（`project_controller_r1_stall_` の 8 件）:
   - escalate が真の最初の checkpoint だけを key `r1_stall:<root>:<終端の子の数>` で聞き、同じ key の 2 回目は待つ。効果が failed なら聞き直し、子がさらに終端になれば新しい key で聞く。
   - escalate が偽（K 未満・到達で連続が戻った後・Outcome 全部到達）、`stall` が null、欄なし、`checkpoints` なしでは出さない。
   - paused・no-progress が先に効く。写していない評価の写しより先に効く（CoS 入力の日の上限が 0 でも聞く。escalate は CoS 入力を使わず、Rust の日の上限は replan だけに掛かる。`crates/task-core/src/project_proposal.rs` の Replan の検証）。
   - 到達を記録していない Outcome がある間は記録が先、全部到達して記録した後は achieved。
   - 既存の試験で検出できない理由: 既存の試験と golden は `stall` を置かず、base の TS は `stall` を読まない。

## 変更

- `control-ts/src/domains/project_controller.ts`: `SnapshotStall`、`SnapshotCheckpoint.stall`、`POLICY.stallOutcomesListed`、`unrecorded`（規則 4 から移した判定）、規則 1c `pc/r1-stall`。写しの規則の番号を 1c から 1d に直した。
- `control-ts/src/domains/project_controller.test.ts`: describe「project_controller r1 stall」の 8 件。
  - `project_controller_r1_stall_escalates_once_with_the_counts_k_and_open_outcomes`
  - `project_controller_r1_stall_below_k_does_not_fire`
  - `project_controller_r1_stall_does_not_fire_after_an_outcome_reach_resets_the_streak`
  - `project_controller_r1_stall_second_time_waits_and_a_new_terminal_child_asks_again`
  - `project_controller_r1_stall_paused_and_no_progress_come_first`
  - `project_controller_r1_stall_precedes_the_checkpoint_mapping`
  - `project_controller_r1_stall_null_or_missing_field_does_not_fire`
  - `project_controller_r1_stall_waits_for_milestone_records_and_achievement`

commit（件名）:

- strategy-v1 r1-ts: control-ts の Project Controller に規則 pc/r1-stall と試験 project_controller_r1_stall_ を足す
- strategy-v1 r1-ts: WU の進捗 r1-ts.md に実装前 brief・試験・検査・LoC を書く

## 変更前に落ちる試験

- `project_controller.ts` だけを `git show HEAD:…`（規則を足す前）に戻して `vitest run src/domains/project_controller.test.ts -t r1_stall` を流した: 8 件中 4 件が落ちた（`escalates_once_…`・`second_time_waits_…`・`precedes_the_checkpoint_mapping`・`waits_for_milestone_records_…`）。残り 4 件は「発火しない」側で、base でも通る（回帰の試験）。
- 規則から記録待ちの 2 条件を外すと `project_controller_r1_stall_waits_for_milestone_records_and_achievement` だけが落ちた（1 failed・37 passed）。

## 試験と検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- check 0 `corepack pnpm@12.6.0 -C control-ts typecheck && corepack pnpm@12.6.0 -C control-ts test`: exit 0。vitest 6 file・151 passed（base は 143）、node --test 47 passed。
- check 1 `grep -qF 'pc/r1-stall' …project_controller.ts && grep -qF 'project_controller_r1_stall_' …project_controller.test.ts`: exit 0。
- check 2 `bash scripts/dev/test-affected.sh`: exit 0。TS だけの変更として ts-verify に委ねた（cargo 0 回）。ts-verify は typecheck・bundle・unit（import グラフで選んだ 5 file、139 passed）・golden（project_controller 15/15 ほか全領域一致）・safety・integration・activate を通った。repo-health verdict=pass・fail_grade=0・new 0・worsened 0。
- `corepack pnpm@12.6.0 -C control-ts compare project_controller`: exit 0、15/15 matched（golden は変わらない）。
- `cargo clippy --workspace -- -D warnings`: exit 0（警告は vendor の deno_core の既存の 1 件だけ）。`cargo fmt --all -- --check`: exit 0。
- check 3 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の path は `control-ts/src/domains/project_controller(.test).ts` と この file だけ。`git diff --quiet $CELERIS_WU_BASE HEAD -- crates docs/api control-ts/src/host.d.ts control-ts/fixtures`: exit 0。
- `sh scripts/dev/progress-index.sh --check`・`bash scripts/dev/check-doc-links.sh`: exit 0。

## LoC（base の script と toml を `git show $CELERIS_WU_BASE:…` で取り出し、`--exclusive-classes` で base と HEAD に当てた）

| 欄 | base | HEAD | 差 |
|---|---|---|---|
| production 全言語（product_only） | 394,335 | 394,376 | +41 |
| production Rust | 347,154 | 347,154 | 0 |
| production TypeScript | 36,038 | 36,079 | +41 |
| own_total_with_tests | 1,090,662 | 1,090,852 | +190 |

純増の理由: 新しい規則と型を足し、置き換える旧い仕組みが無い。規則 4 の判定を共有の `unrecorded` に移した分は行数を減らしていない（同じ判定）。

## 未確認

- 本番の snapshot で R1 が発火するか。本番の DB は Roadmap・Outcome の記録が 0 件で、`outcome_reached` は常に偽になる見込み（r1-core の未確認と同じ）。
- 本物の controller の経路（Rust の snapshot → TS → Rust の門）で `pc/r1-stall` の escalate が門を通るか。TS の単体試験だけで確かめた。Rust の escalate の検証（category・key の長さ・summary 2,000 字・evidence_refs 16 件、写しの不一致にしない）を読んで、通る値にした。実の経路の確認は WU shadow-bench が最初になる。
- 人が R1 の問いに答えた後の続き方。同じ key の間は待ち続け、子がさらに終端になるか到達が記録されるまで、R1 より後の規則（写し・起票）に進まない（`pc/no-progress` と同じ形）。

## 提案

- ADR 2026-11-27-r1-stall-shadow に付記を足す（この WU の範囲は `agent-docs/progress/2026-11-27-strategy-v1*` までで、ADR は書けなかった）: TS の `pc/r1-stall` は、到達を記録していない Outcome がある間と gap に未到達が無いときは出さない。理由は Rust の到達が記録から数えるため。
- shadow-bench では、R1 の待ちが他の規則を塞ぐ時間（`pc/r1-stall/waiting` の続いた判断の数）も数えると、効果 0 の確認に使える。
