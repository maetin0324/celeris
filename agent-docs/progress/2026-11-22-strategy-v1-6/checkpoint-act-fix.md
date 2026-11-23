---
title: 戦略層 v1 子 6 checkpoint-act-fix — 写していない評価の間の guard の wait・escalate と、continue の写し済みの判定
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: checkpoint-act-fix
status: done
updated: 2026-11-23
completed: 2026-11-23
---
# checkpoint-act-fix（WorkUnit）

状態: 完了。ts-checkpoint-rule の進捗（branch `celeris-wu/01M7XQ9QWK58ESMW42Q1TGT3WZ/ts-checkpoint-rule` の `ts-checkpoint-rule.md`）の未解決事項 1・2 を Rust 側で直した。
設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D5.2・D5.3（本文をその場で直した。末尾に付記は足していない）。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## 実装前 brief

1. 根本原因: (1) `task_core::project_proposal::validate_checkpoint_act` は、写していない評価があると rule が `pc/checkpoint/act` でない行動をすべて落とした。paused の wait・`pc/no-progress` の escalate・CoS 入力の日の上限の wait も `checkpoint_action_mismatch` になり、no-progress の guard が人に知らせずに止まった。(2) `task_ops::project_snapshot::checkpoints` の写し済みの判定は `effect_state` が `pending`・`applied`・`suppressed` の写しだけを数えた。continue・wait の写しは効果が無く `none` なので、continue の行の評価がいつまでも写していない評価に残り、起票・milestone に戻れなかった。
2. 責務 owner: (1) `validate_checkpoint_act`（Rust の検証）。(2) `project_snapshot::checkpoints` の `mapped`（snapshot の組み立て）。どちらも TS では直せない（表に無い行動は Rust が落とす／写し済みは Rust が導く）。
3. 既存の再利用先: 既存の `ProjectAction::Wait` の `phase`・`ProjectAction::Escalate`、判断の行の `effect_state`、既存の試験の helper（`checkpoint`・`unmapped`・`act`、task-ops の `root_row`・`create_task`・`cos_operation_apply_at`）。
4. 選択肢: (1) で TS の guard の rule id（`pc/project-not-active` 等）を Rust に列挙する案と、行動の種類で分ける案を比べ、後者を採った（Rust に TS の rule id を書かない）。wait は `phase = waiting` だけを通し、`wait(achieved)`（到達の記録）は落とす。(2) で「`none` を足す」と「`failed` 以外」を比べた。`effect_state` の値は `none`・`pending`・`applied`・`suppressed`・`failed` の 5 つなので同じ集合で、`failed` だけを外す書き方を採った（目的の文と同じ形）。
5. 新しい状態・分岐・fallback・port: 新しい状態・fallback・port は無し。分岐は (1) の「act でない `evaluate`・`wait(waiting)`・`escalate` は通す」1 つ（既存の evaluate の早期 return と 1 つにまとめた）。
6. 撤去対象: (1) の「act でない行動は evaluate 以外すべて落とす」規則と、(2) の `pending`・`applied`・`suppressed` の列挙。ADR D5.2・D5.3 の旧い文は本文ごと置き換えた。
7. 不変条件: 下の試験。off_track・unknown の評価から、起票・CoS 入力・milestone・到達の記録・continue は出ない（既存の `project_controller_checkpoint_act_must_map_unmapped` と新しい guard の試験）。

## 試験（試験を先に置いた）

| 試験 | 守る不変条件 | 既存の試験で検出できなかった理由 |
|---|---|---|
| `task-core` `project_controller_checkpoint_guard_while_unmapped_holds_pass_and_progress_fails` | 写していない評価がある間も、paused の `wait(waiting)`・`pc/no-progress` の escalate・`pc/checkpoint/act/daily-limit` の wait は通る。起票・`replan`（CoS 入力）・`mark_milestone`・`wait(achieved)`・`continue` は `checkpoint_action_mismatch` で落ちる | 既存の `..._act_must_map_unmapped` は rule が act の行動と `pc/outcome-open` の replan だけを見る |
| `task-ops` `project_controller_checkpoint_continue_maps_none_effect_and_returns_to_rules` | store に strategy root・`evaluate` の判断 D1（applied）・最終の記録（on_track・continue）を置く。写す前は D1 が写していない評価で表は continue・wait。効果 `none` の continue・wait の写しの後は写し済みで、milestone の提案は `checkpoint_action_mismatch` で落ちない。効果 `failed` の写しは写し済みに数えない | task-core の試験は snapshot の `unmapped` を手で置くので、snapshot の組み立てが写しを数えないことを検出できない。base に `checkpoints` を store から組む試験は無かった |

実装前の実行（試験だけを足した状態）: `cargo nextest run -p task-core -p task-ops --no-fail-fast -E 'test(/project_controller_checkpoint_/)'` は 5 件中 2 件 fail。

- task-core の新しい試験: `paused の wait` の assert で `Err([ProposalViolation { code: "checkpoint_action_mismatch", … "rule \"pc/project-not-active\" / wait does not map an unmapped checkpoint evaluation (None)" }])`。
- task-ops の新しい試験: 効果 `none` の continue の写しの後の `assert!(after[0].unmapped.is_empty())` で fail（`Continue { watch_root_ids: [..], next_wake_after_secs: 60 }`）。写す前の assert（D1 が写していない評価、表が continue・wait）は通ったので、fixture は正しい。
- 提出時の HEAD でも再確認した: `project_proposal.rs` と `project_snapshot.rs` の production の差分だけを `git apply -R` で戻すと同じ command で 5 件中 2 件 fail（`project_proposal_tests.rs:483`・`project_snapshot.rs:1208`）、戻した差分を元に戻すと 5 件 pass。

## 変更

- `crates/task-core/src/project_proposal.rs` `validate_checkpoint_act`: rule が act でない `evaluate`・`escalate`・`wait(phase = waiting)` は、写していない評価があっても通す。doc comment を直した。
- `crates/task-ops/src/project_snapshot.rs` `checkpoints` の `mapped`: `effect_state != "failed"` にした。試験の module `checkpoint_tests` を足した。
- `agent-docs/adr/2026-11-22-parent-checkpoint-evaluator.md`: D5.2 の `checkpoint_action_mismatch` の段落と D5.3 の本文をその場で直した（新しい付記なし）。

## 検査（提出時の HEAD で実行）

- `cargo nextest run -p task-core -p task-ops -p task-dispatch -p celeris --no-fail-fast -E 'test(/project_controller|checkpoint/)'`: exit 0、115 件 pass（新しい 2 件を含む）。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 8732 件 pass・31 件 skip（対象 crate は task-core・task-ops とその逆依存）。web の typecheck と unit 745 件 pass、ts-verify も通った。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0（`cargo fmt --all` を当てた後）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` は ADR・`project_proposal.rs`・`project_proposal_tests.rs`・`project_snapshot.rs` とこの進捗だけ。
- dispatcher・store・migration・events の差分: `git diff $CELERIS_WU_BASE -- crates/task-dispatch crates/task-core/src/store crates/task-core/migrations crates/task-core/src/model/events.rs` は 0 行。

## 行数（`git diff --numstat $CELERIS_WU_BASE`）

- production（Rust）: `project_proposal.rs` +14 −7（純増 7。`matches!` の整形が多い）、`project_snapshot.rs` の本体 +2 −1（comment 1 行と条件の置き換え）。純増の理由は、guard の行動を通す分岐を足したこと。
- test: `project_proposal_tests.rs` +94、`project_snapshot.rs` の `checkpoint_tests` 約 +254（store に strategy root と評価の記録を組む fixture）。
- ADR: +4 −2。

## 親の目的への寄与

- 確認済み（試験）: 写していない評価がある間も、既存の paused・no-progress・CoS 入力の日の上限の guard が働く（改善 C の「繰り返す停滞は既存の guard で止まる」）。on_track・at_risk の continue の評価を写した後、controller は次の起票・milestone に戻る（途中評価が計画を止めたままにならない）。
- 未確認: 本物の TS（ts-checkpoint-rule の bundle）と Rust の検証を合わせた端から端までの確認。ts-checkpoint-rule の突き合わせで invalid だった guard の 3 例が通るかは、統合（integrate-controller）の後に second-gate-shadow で確かめる。

## 未解決事項

1. 判断の保持の GC（`crates/task-core/src/store/project_retention.rs` の `DECISION_GUARD`）は、効果 `none` で 30 日を過ぎ、新しい順の 256 件の外の判断を消す。continue・wait の写しの判断が消えると、その評価は写していない評価に戻り、TS が act の continue・wait をもう 1 回出す（写し直すだけで、計画は進まない）。コードを読んで導いた結論で、試験での再現はしていない（未確認）。直すなら store の条件と trigger（migration）の変更になり、この WU の範囲（store・migration を変えない）の外。

## 提案

- 未解決事項 1: 害は 1 回の余分な判断なので、直すかどうかは second-gate-shadow の結果を見て決める。直すなら `DECISION_GUARD` と同じ条件の削除の trigger に、rule が `pc/checkpoint/act` の判断を除く条件を足す（migration が要る）。store の WU で行う。
