---
title: P4-B 達成の後の根拠の失効と Roadmap の Outcome の決定的な checks
tasks: [01M6PAMC3JDQ9KKVQZSJBVZ1W3]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P4-B 達成の後の根拠の失効と Roadmap の Outcome の決定的な checks

完了日: 2026-11-05。第 4 フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の子（人の依頼 seq 2343）。基準は `git merge-base HEAD main`。
ADR は `agent-docs/adr/2026-11-05-phase4-p4b-evidence-revocation.md`。P4-A の G1・G2・G3 を扱う。

## 結論
- **G1（達成の後の失効）を実装した。** 根拠の root の reopen（done → 非終端）と `IntegrationReopened` が wake `evidence_changed` になり
  （migration 0115）、achieved の案件でも Rust が `recheck_achievement` で数え直し、未達があれば guard `achievement-revoked` で phase を
  `waiting` に戻して直ちに TS の判定に回す。再 review の不合格（`review_failed`）・root の失敗も同じ数え直しのきっかけになる。
  旧い `pc/achieved` の判断の行と Roadmap の `reached` の記録は消さない。
- **人の Roadmap の改版でも達成が戻る（attempt 2 の差し戻しの修正）。** controller の root は起票時に Outcome ごとの条件（acceptance・checks）の
  digest を冪等キーの行に残し、snapshot は今の版と違う Outcome を root の `stale_outcome_ids` に入れる。Rust の `root_satisfies` と TS の
  `satisfies` はその Outcome を `outcome_ids` の一致でも満たしたことにしない。人が acceptance だけ・checks だけを変えた版を承認すると、
  controller の root で達成していた案件も未達になり、新しい条件の root を起票して再達成する（試験 3 件）。
- **G2（Roadmap の Outcome の checks）を実装した。** `RoadmapOutcomeSpec.checks` は `Project.request` の Outcome block と同じ形・検証で、
  controller の root の `Check::Command` になる。版の一部なので変更は提案と人の承認（`post_decide` は `requires_human`）を通る。
- **G3（replay v2 の C06・C10）を判定した。** C06 は妥当な停止（任意の o2 を人が却下し、controller は Outcome を自分で取り下げられない）。
  C10 は不具合（CoS への指示で作った root が合格しても `attempts_exhausted` の `blocked` が解けなかった）で、Rust の検証を通る root が現れたら
  再開するよう直した。replay の raw は C10 new の `controller_achieved` だけが false → true に変わった。
- 本番・実 LLM・長時間の試験はしていない（「未実施」節）。旧 milestones API の 410（`milestone_lifecycle_endpoints_are_gone`）は変えていない。

## 変更（commit は件名で）
- `p4b: 達成の後の根拠の失効で Rust が数え直して waiting に戻す（wake evidence_changed・migration 0115）と Roadmap の Outcome の checks`
- `p4b: 試行の上限で止まった Outcome を Rust の検証を通る root が満たしたら再開する（C10）と replay の raw の更新`
- `p4b: schema 115 の版の一覧と試験を直し、ADR を足す`
- `p4b: 進捗を書く`（この file）
- `p4b: 進捗の試行の参照を arm-map の path にする`
- `p4b: 起票時の Outcome の条件の digest を root に残し、人の改版で acceptance・checks が変わった root を根拠にしない`（attempt 3。差し戻しの修正）
- `p4b: 人の改版の失効の ADR と進捗を直す`（attempt 3）

主な file: `crates/task-core/migrations/0115_project_wake_evidence_changed.sql`、`crates/task-core/src/project_wake.rs`（`EvidenceChanged`）、
`crates/task-core/src/roadmap.rs`（`checks`・`snapshot_outcomes`・`validate_proposal`）、`crates/task-core/src/project_proposal.rs`（`OutcomeCheck` の
Serialize・JsonSchema）、`crates/task-dispatch/src/dispatcher/project_wake.rs`（`wakes_for_event`・`evaluate` の数え直し）、
`project_guard.rs`（`evidence_wake`・`achievement-revoked`）、`project_decider_ts.rs`（`recheck_achievement`）、`project_resume.rs`（`outcome_evidence`）、
`crates/campaign/src/ops_r1_trace.rs`（読める schema 版に 115）。attempt 3: `project_proposal.rs`（`outcome_spec_digest`・`SnapshotRoot.stale_outcome_ids`・
`root_satisfies`）、`task-ops/src/project_snapshot.rs`（`LaunchRootMeta.outcome_specs`・`for_root`・`stale_outcome_ids`、`launch_root_claim` に outcomes）、
`project_decider_ts.rs`、`control-ts/src/domains/project_controller.ts`（`satisfies`）と test・`control-ts/fixtures/project_controller/invariants.jsonl`（再生成）。

## 試験（新規 11 件、`dispatcher/tests/project_revocation.rs`、`project_achievement` の子 module。ほかに TS 1 件）
| 試験 | 確かめること |
|---|---|
| `project_revocation_reopen_after_achieved_returns_to_waiting` | reopen の event だけ（cron なし）で `achievement-revoked:o1`・waiting。記録に `evidence_changed:` の wake。`reached` の記録の数は不変。TS が達成と偽っても `achieved-unverified` |
| `project_revocation_review_fail_after_achieved` | reopen → 再 review で command の検査が落ちて failed → waiting。到達の記録は 1 件のまま |
| `project_revocation_integration_reopened_after_achieved` | `IntegrationReopened` → waiting、取り込み直し → 再達成。`reached` は 1 件 |
| `project_revocation_intact_evidence_and_other_project_stay_achieved` | 別の案件の root の reopen・cron では achieved のまま（`achieved-quiet`、revoke 0） |
| `project_revocation_human_roadmap_change_reopens` | 人や CoS の root（`outcome_ids` なし）で達成した案件で、人が o1 の acceptance を変えた版を承認 → `achieved-unverified:o1`。旧い `reached` は残る |
| `project_revocation_controller_root_acceptance_change_reopens` | controller が起票した root（`outcome_ids = [o1]`）で達成の後、人が o1 の acceptance だけを変えた版を承認 → 達成でなくなり、TS が `pc/outcome-open` で 2 本目を起票。旧い root は done・合格のまま `stale_outcome_ids = [o1]`、`root_satisfies` は偽、到達の記録があっても `unverified_outcomes` は `[o1]`（TS が偽っても Rust は拒否）。2 本目（新しい acceptance）の合格で再達成。`pc/achieved` の行と `reached` 1 件は残る |
| `project_revocation_controller_root_checks_change_reopens` | 同じく checks だけを変えた版（`test -f out/o1.txt` → `test -f out/o1-v2.txt`）。2 本目の root は新しい command を `Check::Command` に持つ |
| `project_revocation_controller_root_title_change_stays_achieved` | 条件の変わらない改版（title・priority だけ）では root は stale にならず、achieved のまま・revoke 0・root は 1 本 |
| `project_revocation_crash_and_human_race_never_rewrite_achieved` | 数え直しの commit の直前で panic（crash）→ daemon を作り直して同じ wake で waiting。同じ窓で人が mode を置き直すと commit は stale。`pc/achieved` の判断は増えない |
| `project_revocation_e2e_reach_invalidate_fix_reachieve` | Roadmap の checks が root の `Check::Command` → 到達 → reopen と command の不合格で失効 → controller が直す root（`outcome:o1:2`）を起票 → 合格で再達成。`reached` の記録は 1 件、revoke は 1 回 |
| TS `does not count a root whose outcome conditions changed after it was launched` | `stale_outcome_ids` の Outcome は acceptance の文が一致しても到達に数えず、`outcome:o1:2` を起票する |
| `project_revocation_roadmap_checks_validation_and_human_gate` | 空の command・9 件の checks は `roadmap_invalid`。checks だけを変えた版は承認まで approved に入らない。checks の無い版の JSON に欄が出ない |

e2e の各段の状態の raw: `agent-docs/progress/2026-11-05-phase4/p4b/e2e.json`（`PROJECT_REVOCATION_RAW=<dir>` で書く）。段は
`launched`（active）→ `achieved` → `revoked`（active、root は failed と ready）→ `fix_launched` → `reachieved`（achieved）。

## G3 の分析（replay v2、new 腕）
raw は `agent-docs/progress/2026-11-05-phase3/p0c/replay-v2/facts/`（trial は `arm-map/` で引く）、シナリオは `tests/fixtures/long-horizon/oracle/<case>/scenario.json`。
- C06 `auth_wait`: o1 は承認されて公開・取り込み、o2 は `required: false`・`approval: reject`。o2:1 は人の却下で cancelled になり、TS は
  `root_failed:o2:1` を人に聞いて止まる。controller に Outcome を任意にする欄は無く、取り下げは人の Roadmap の版（`dropped: true`）か依頼文の
  変更でしかできない。**妥当な停止**として変えていない（raw も不変）。
- C10 `repeat_failure`: o1:1・o1:2 が同じ理由で失敗し、TS は `attempts_exhausted:o1` を人に聞いて `blocked`。人は 86,400 秒に CoS へ指示し
  （`human_answer_instruct`）、CoS の root o1:3 が 90,000 秒に合格した。P1-G の再開は問いの key の許可・却下・依頼文の変更しか見ないので、
  controller は `blocked` のまま o1 を記録しなかった。**不具合**として `project_resume` に「o を I3 で満たす root が現れたら再開」を足した。
- before/after: `p4b/replay-v2-c10/c10-new.before.json`・`c10-new.after.json`・`raw-diff.log`。差は `controller_achieved` の 1 行だけ。
  committed の baseline（`p0c/replay-v2/facts/` の C10 new の試行。`arm-map/` で case C10・arm new を引く）を after に置き換えた。

## 検査（提出時の HEAD で実行）
attempt 3（差し戻しの修正の後、提出時の HEAD）:

| command | exit | 要点 |
|---|---|---|
| `cargo nextest run -p task-dispatch -p celeris -E 'test(project_revocation_)'` | 0 | 11 passed（新規 3 件を含む） |
| `cd control-ts && npx vitest run src/domains/project_controller.test.ts`・`npx tsc --noEmit` | 0 | 20 passed、型エラー 0 |
| `bash scripts/dev/test-long-horizon-bench.sh --out "$TMPDIR/lhb"` | 0 | replay 40・grade 24・contract 9 passed、committed の v2 raw と差 0（digest の変更で replay は変わらない） |
| `PROJECT_REVOCATION_RAW=<dir> cargo nextest run … -E 'test(project_revocation_e2e)'` | 0 | `p4b/e2e.json` と bytes が同じ |
| `bash scripts/dev/test-affected.sh` | 0 | 7709 tests run: 7709 passed（22 skipped）、web vitest 711・control-ts vitest 132 passed、script bundle SKIP 0 件 |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告 0 |
| `cargo fmt --all -- --check` | 0 | 差分 0 |

attempt 1・2:

| command | exit | 要点 |
|---|---|---|
| `cargo nextest run -p task-dispatch project_revocation_ project_achievement_ project_recovery_ project_controller_ project_wake` | 0 | 189 passed（新規 8 件を含む） |
| `bash scripts/dev/test-long-horizon-bench.sh --out "$TMPDIR/lhb"`（baseline 更新前） | 1 | replay 40 passed、raw の差は C10 new の `controller_achieved` だけ（`p4b/replay-v2-c10/raw-diff.log`） |
| 同（baseline 更新後） | 0 | replay 40・grade 24・contract 9 passed、CLI 採点一致。new: reach 1.00 autonomous 1.00（log `p4b/bench.log`） |
| `bash scripts/dev/test-affected.sh` | 0 | 7703 tests run: 7703 passed、script bundle SKIP 0 件（1 回目は schema 115 の版の一覧で 15 件落ち、直して再実行） |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告 0 |
| `cargo fmt --all -- --check` | 0 | 差分 0 |
| `UPDATE_SCHEMA=1 cargo nextest run -p task-api committed_schema_matches_generated -p task-core event_row_schema_matches_committed` | 0 | 生成物の差分なし（Roadmap の型は API schema の生成対象外） |

## 本番への移行
- migration 0115 は起動時に `project_wakes` を作り直す（行はそのまま）。config の変更は無い。release の配備・promote は人の操作で、この task では
  していない。戻し方: 0115 の DB は旧 binary（SCHEMA_VERSION 114）では `SchemaTooNew` で開けないので、release の rollback の前に DB の
  backup（release の手順のもの）から戻すか、0115 を残したまま新しい binary を使う。`evidence_changed` の wake の行は旧 binary の CHECK に合わない。

## 未実施（人の承認が要る・範囲外）
- 本番の shadow・promote・systemd・本番 config・実 LLM の評価・24〜72 時間の稼働はしていない（未承認）。
- 実 LLM の new 腕の C06・C10 の再試行は未実施。

## 未解決
- attempt 2 までの記述「checks だけを変えた版では戻らない」は範囲が狭かった。実際には controller が起票した root（通常の経路）では acceptance を
  変えた版でも戻らなかった（`root_satisfies` が `outcome_ids` の一致だけで真を返した）。attempt 3 で起票時の条件の digest により直した。
- 残り: 人や CoS が作った root（`outcome_ids` なし、acceptance の文の一致で数える）は、人が checks だけを変えた版を承認しても根拠のまま数えられる
  （この root には起票時の digest が無い）。
- digest の無い controller の root の記録（この変更より前の行）は `stale_outcome_ids` に入り根拠にならない（誤 achieved 0 を優先）。本番の導入は
  未実施なので、本番に該当の行があるかは未確認。
- 再達成の後も Roadmap の `reached` の evidence は最初の root を指す。
- migration 0115 の「既存の wake の行を写す」ことを直接確かめる試験は無い（0113 の migration の試験が 0115 を同じ DB で当て直す経路だけ）。

## 提案
- 人や CoS の root でも checks の変更を効かせるなら、root の `Check::Command` の command と今の Outcome の checks の照合を Rust が行い、結果を
  `stale_outcome_ids` と同じ形で TS に渡す。
- 再達成の時に Roadmap へ新しい根拠の root を `root_linked` で残す。

## 次の gate
- 親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の統合（P4-C と `project_wake.rs`・migration の番号が衝突しないこと。C は 0116）。
- release gate（人）: 全試験と migration 0115 の配備。
