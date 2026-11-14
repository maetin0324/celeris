---
title: "健全性v2 R4 review-decide: review の判定と起動の判断を review_decision へ分ける"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 review-decide

完了日: 2026-11-14。基準は `$CELERIS_WU_BASE`（この WU の base）。検査は提出時の HEAD で実行した。

## 結論

- `crates/task-dispatch/src/review_decision.rs` に review の判断の純粋関数を置いた。依存は `task_core` だけで、`review`・
  `dispatcher` に依存しない（verdict は trait `VerdictView` を通して読み、`review::Verdict` が実装する）。
- D4 review-result: `review_decision::failed_checks` が決定的 check の結果（`CheckResult` の列）から失敗一覧（`FailedCheck`）を
  作る唯一の関数になった。使う経路は 3 つ: 最終 review（`review_verdict.rs` の `try_review_repair` と `judge_review`）、WU の checks
  （`work_units.rs` の `WorkUnitCheckRun::failure` が `failed_work_unit_checks` を呼び、これが `failed_checks` の結果を event の形
  `FailedWorkUnitCheck` に写す）、段の統合の checks（`phase_integration.rs` の `schedule_integration_check_repair`）。
  `review.rs` の `failed_checks` と、WU・段の統合の `filter(!pass).map(..)` の写しを消した。check の実行（`review::run_work_unit_checks`
  など）は変えていない。
- D3 `judge_review`: 最終 review の結論（`passed` は失敗一覧が空のときだけ true、不合格の理由の連結、`review_discrepancy` の結果）。
  reviewer が合格でも決定的 check が落ちていれば成功にしない。
- 他に分けた判断（いずれも入力 → 提案。適用は呼び元）:
  - `reviewer_criteria_passed`: reviewer の条件が全部合格なら `ReviewerPassed` に残す条件の idx。
  - `delivery_verdict`: delivery の行の照合（worker run・review run・`Reviewing`・行の条件の verdict）と結論（`MergeQueued` か
    `Blocked`、detail）。reviewed candidate の照合の条件は移す前と同じ 4 つで、減らしていない。
  - `review_repair_class`: Execute の task だけ、reviewer run を起こさなかった項目を外し、`classify_review_failure` で class を選ぶ。
  - `repair_cap_reached`: repair の上限（task 全体か同じ bucket）。最終 review と段の統合の 2 か所の同じ比較を 1 つにした。
  - `resync_gate`・`target_quiet`: review の入口の再同期を続けるか、止めるか（最初の回か）、静かなら再開するか。入力は
    `pre_review_stale_state` の結果と `MAX_TARGET_RESYNCS`。
  - `reviewer_reuse_overlap`: reviewer の合格の再利用の照合（target の進みと候補の差分の重なり）。入力の合格は `bound_reviewer_pass`、
    差分は git（呼び元）。
- 既存の純粋関数（`health_review::gate::health_repairs`・`health_review::limits::judge`・`pre_review_stale_state`・
  `bound_reviewer_pass`・`pending_integration_repair`・`review_discrepancy`・`repair_bucket_of_title`）は写さず、定義はそれぞれ 1 つ。
  結果を facts として呼び元が渡す。
- git・event の追記・delivery の保存・通知・snapshot・review run の起動は `review_verdict.rs`・`review_spawn.rs` に残した。
  transaction の範囲は変えていない。
- `REVIEWER_NOT_EVALUATED_REASON` の定義を `review_decision` へ移した（`review_repair_class` が使う判断の語のため）。verdict を作る
  `review.rs` は `pub(crate) use` で同じ定数を使う（定義は 1 つ）。
- ADR 2026-11-11-dispatcher-state-ownership は変えていない。D3 の表の `plan_review` は、spawn_review の中で git の fact を挟む 2 か所の
  判断（`resync_gate` と `reviewer_reuse_overlap`）として置いた。
- `docs/architecture-map.md` に「review の判断」の行を足し、「planner/reviewer の起動・判定」の行から参照した。

## 実装前 brief

1. 根本原因: `on_review_finished`・`try_review_repair`・`spawn_review`（`target_resync_exhausted`・`reviewer_reuse`）は、結論・repair の
   選択・再同期の可否・候補の照合を store の読み書きと交互に書いていた。失敗一覧を作る処理は最終 review・WU・段の統合の 3 か所に
   別々にあり、repair の上限の比較も 2 か所にあった。
2. 責務 owner: 判断は `review_decision`（ADR D3・D4 の表）。副作用は `dispatcher/review_verdict.rs`・`review_spawn.rs`・
   `work_units.rs`・`phase_integration.rs`。
3. 既存の再利用先: `review_discrepancy`・`pre_review_stale_state`・`bound_reviewer_pass`・`repair_count`・`health_review::limits`
   （`review_limits` 経由）・`task_core::classify_review_failure`。
4. 選択肢: (a) 1 つの大きな提案の struct を返す（on_review_finished の途中に store の読み出し〈delivery の行・子 task・policy の gate〉が
   挟まるので、順序を変えずに 1 回の呼び出しにできない。不採用）。(b) 判断ごとの小さい純粋関数を、今の順のまま呼ぶ（採用）。
   (c) `review_decision` が `review::Verdict` を直接使う（health の deps.cycle が dispatcher↔review の循環に review_decision を
   足すと検出した。trait `VerdictView` にして不採用）。
5. 新しい状態・分岐・fallback・port: 状態・fallback は無し。新しい型は入出力の値（`CheckResult`・`ReviewFacts`・`ReviewJudgement`・
   `DeliveryRow`・`DeliveryVerdict`・`RepairUsage`・`HaltedReview`・`ResyncGate`）と読み口の trait `VerdictView` だけ。
6. 撤去対象: `review::failed_checks`、WU と段の統合の失敗一覧の写し、2 か所の repair の上限の比較、on_review_finished の
   reviewer の合格・delivery の照合と結論・不合格の理由の組み立て、try_review_repair の kind・not evaluated の除外・分類、
   target_resync_exhausted の stale の記録の取り出し、reviewer_reuse の重なりの計算。
7. 不変条件: 決定的 check が落ちていれば成功にしない。delivery の結論は同じ候補の行にだけ書く。repair は上限の内だけ。再同期は
   上限を超えたら止める。reviewer の合格は差分が重ならないときだけ再利用する。確かめる試験は `review_decision/tests.rs` の表の
   試験 10 本と、既存の試験（`cex_`・`dispatcher/tests/{review,work_unit_check_failures,health_enforce,target_sync}.rs` など、
   test-affected）。

## 試験

- `crates/task-dispatch/src/review_decision/tests.rs`（10 本、すべて表の形）: `failed_checks`・`verdict_results`・`judge_review`・
  `reviewer_criteria_passed`・`delivery_verdict`・`review_repair_class`・`repair_cap_reached`・`resync_gate`・`target_quiet`・
  `reviewer_reuse_overlap`。既存の試験は daemon の経路で 1 つずつの場合を見るだけで、境界（上限ちょうど・行の照合の欄ごとの
  不一致・止めた記録が無い場合・reviewer の合格と決定的 check の不合格の組み合わせ）を表で押さえていなかった。
- 試験は消していない・弱めていない。`cex_` の試験は変えていない。

## 前回の run の check の不合格（repo-health）と直し方

- 前回の run の後、celeris の check `celeris:repo-health/1` が 2 件の fail で落ちた: `size.responsibility-growth`（`work_units.rs` の
  本体の行が base の 2107 から 2108 に増えた。閾値 2000 を base で既に超えているので、1 行でも増えると worsened）と `source-size`
  （`check-source-size.py` が `work_units.rs` の 2109 行を許容の 2108 行より多いとした）。原因は `WorkUnitCheckRun::failure` で
  `failed_checks` の結果を `FailedWorkUnitCheck` へ戻す `filter_map` を `work_units.rs` に書いたこと。
- 直し方: この写しを `review_decision::failed_work_unit_checks` に移した。失敗の選び方は `failed_checks` のまま 1 つで、
  `failed_work_unit_checks` は command の check を `cmd`・`expect_exit`・`detail` に写すだけ。`work_units.rs` は 2109 行 → 2099 行
  （base の 2108 行より 9 行少ない）。WU の event の中身と順は変わらない。
- 試験: `review_decision_failed_checks_table` に WU の形の確認（合格を外し、`cmd`・`expect_exit`・判定文を写す）を足した。既存の
  `dispatcher/tests/work_unit_check_failures.rs` は daemon の経路で 1 件の不合格を見るだけで、`expect_exit` が 0 以外の写しを
  押さえていなかった。

## 計画の check 2 の `fn judge` の数について

- check 2 は `fn judge\b` の定義が crate で 2 つであることを求める。計画を承認した時点の 2 つは `health_review::limits::judge` と
  `health_review/gate.rs` の private な report の判定だった。2026-11-12 に別の task が `disk_admission.rs` に `Gate::judge` を足し、
  この WU の base では 3 つになっていた（この WU の変更の前から check 2 は落ちる）。
- `health_review/gate.rs` の private な `judge`（検査器の report から evidence と verdict を書く）を `judge_report` に改名した。
  `health_review` の中で `judge` の名を `limits::judge` だけにする。振る舞いは変えていない（呼び出しは 1 か所）。
- `review_decision` に `fn judge` は無い。`limits::judge` の定義は 1 つのまま。

## 証拠（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh`: exit 0（nextest 3927 passed、11 skipped）。
- `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check && bash scripts/dev/check-health.sh`: exit 0。health は
  verdict=pass、fail_grade=0、new=1（`migration.remnant`、`crates/task-dispatch/src/host_maintenance.rs` の `legacy`。この変更の
  前からあり、この WU の差分ではない）。
- 計画の check 2（純粋さと関数の定義の数）: exit 0。
- 計画の check 3（`cargo nextest run -p task-dispatch -E 'test(cex_) | test(review_decision)'`）: exit 0（35 passed）。
- 計画の check 4（D4 の表の review-decide の行。定義 1 か所、呼び出し 2 か所以上）: exit 0（`failed_checks(` の呼び出しは
  review_verdict.rs・phase_integration.rs と review_decision.rs の `judge_review`・`failed_work_unit_checks` の 4 か所。WU の
  checks は `failed_work_unit_checks` を通して使う）。
- 計画の check 5（範囲）: exit 0。
- `python3 scripts/dev/check-architecture-map.py`: exit 0。

## commit

- 「phase decide: review の判定と起動の判断を review_decision の純粋関数へ分け、D4 review-result の failed_checks を 3 経路で共有する」
- 「phase decide: review_decision を review に依存させず（VerdictView）、WU と段の統合の呼び出しを短くする」
- 「phase decide: review-decide の進捗を書く」
- 「phase decide: WU の失敗一覧の写しを review_decision へ移し、work_units.rs の行を base より増やさない」

## 未解決事項

- 無し（この WU の範囲）。

## 提案

- 計画の check の「同名の関数の定義の数」は、crate 全体の数でなく対象の module（例 `health_review/`）に絞るか、「この WU で増えない」
  （base と head の数の比較）にすると、別の task が無関係な同名の method を足しても落ちない。
