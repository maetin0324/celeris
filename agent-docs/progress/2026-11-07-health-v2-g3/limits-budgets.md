---
title: 健全性v2 G3 limits budgets — health 由来の修復・review・時間が既存の各上限の中で数えられることを試験で固定する
tasks: [01M70C8NHEANARRKZD9RGKZ87D]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 limits budgets — health 由来の修復・review・時間が既存の各上限の中で数えられることを試験で固定する

WorkUnit `budgets`（task 01M70C8NHEANARRKZD9RGKZ87D）。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D4.1・D4.2・D4.4 と J4・J6 を dispatcher の流れの試験で確かめる。基準は `$CELERIS_WU_BASE`。

結論: 完了。試験 9 件 `health_limits_cap_*`（`crates/task-dispatch/src/dispatcher/tests/health_limits_budget.rs`）が合格した。数えていない上限は見つからず、上限の owner の module は変えていない。新しい上限の欄・config の欄・表は足していない。本体の変更は `health_review/gate.rs` の試験専用の口（`#[cfg(test)]`。本番の build では締め切りを変えない）だけ。

## 実装前 brief
1. 根本原因: 既存の `health_limits_review_*`・`health_limits_integration_*` は修復の上限を health の修復だけで使い切る場面と Error の止め方を見る。health でない修復との合算、rule ごとの bucket、木の上限、token 予算、review 前の同期の上限、検査器の時間切れ、使い切りの後の error・unknown・unsupported は試験が通っていない。数えていない上限があれば、その場面で上限を超えた run・修復が起きるはず。
2. 責務 owner: 修復の数え方は `dispatcher::review_verdict::try_review_repair` と `health_review/check_item.rs` の `repair_count`（bucket `health:<rule_id>`）。task の review の止め方は `health_review/review_limits.rs`。木の上限は `Dispatcher::tree_limit_hold_for`（`dispatcher/tree_units.rs`）。token は `dispatcher/token_budget.rs`。review 前の同期は `task_ops::delivery::MAX_TARGET_RESYNCS`。CoS の review_rework は task-api の `cos/basis/review_rework.rs`。検査器の締め切りは `health_review/policy.rs` の `BaseChecker`（`with_base_checker` が渡す deadline）。
3. 既存の再利用先: 試験の土台は `health_enforce.rs` の `RepairActionAdapter`・`non_celeris_dispatcher`、`target_sync.rs` の `reviewing_task`・`commit_file`・`set_check`、`tree_gate.rs` の `seed_tree_run`。木の上限の落ち着きの見方は `tree_gate.rs` の `run_until_quiet` と同じ。
4. 選択肢: 時間切れの試験は (a) 時計の差し替え（検査器は子 process で `Instant` の締め切りを見るので tokio の時計では効かない）、(b) 終わらない stub と試験から渡す短い上限（採用）。短い上限の口は、検査器を起こす唯一の口 `gate.rs` の `with_checker` に置いた（入口の評価 `health_gate` と G2 の材料 `input.rs` の評価が共有する）。最初は `health_gate` の入口に置いたが、reviewer のある review では G2 の材料の評価（`input.rs` の `HEALTH_INPUT_TIMEOUT`）が先に検査器を起こして 600 秒止まったので移した。review_rework の上限 2 回は task-api の判定で、task-dispatch の試験からは呼べない。試験は Objective どおり `health_limits_budget.rs` に置き、dispatcher が Failed を 1 回の評価につき 1 回しか作らず、reopen を自分で足さないことを CoS と同じ `Trigger::Reopen` の往復で確かめた。
5. 新しい状態・分岐・fallback・port: `gate.rs` の `test_timeout`（`#[cfg(test)]` の module。作業ツリーの path を鍵に検査器の締め切りを早める。`#[cfg(not(test))]` 側は締め切りをそのまま返す）。状態・config の欄・event・API の欄は足さない。
6. 撤去対象: 無し（試験の追加だけ）。
7. 不変条件: health 由来の修復・review・run は既存の上限の中で数えられ、使い切りは Blocked と受信箱（木の上限は決定の要求）で止まり、修復の連鎖や終わらない待ちを作らない。error・unknown・unsupported・時間切れは合格にも修復にもならない。試験は下の表。

## 上限ごとの試験の対応表
試験は全部 `crates/task-dispatch/src/dispatcher/tests/health_limits_budget.rs`。検査器は試験の repo の base に commit した stub で、作業ツリーの file（`FIXED`・`STATUS`・`FINDING`・`HANG`）で結果を決める。時計は使わず、`run_until_idle`（出来事が尽きるまで tick を回す）か、決定待ちの場面は `settle_quiet`（run・判定・統合が 50 tick 続けて無い。800 tick で落ち着かなければ落ちる）で決まる。外部ネットワークに出ず、LLM を呼ばない。

| 上限（ADR） | 試験 | 確かめること | 使い切りの止まり方 |
|---|---|---|---|
| task の `max_repairs`（D4.1） | `health_limits_cap_max_repairs_shared_with_non_health_repairs` | fmt の修復（health でない）で `max_repairs` = 1 を使った後の health の Fail は、health の bucket が 0 回でも修復を足さない | `review_blocked` で Blocked（attempts 0）、受信箱の質問 `health_repair_cap:`（「bucket health:deps.violation: 0 回」「修復の合計 1 回 / max_repairs 1」） |
| `max_repairs_per_class`（D4.1、class は `health:<rule_id>`） | `health_limits_cap_per_class_counts_each_rule` | `health:deps.violation` を 1 回で使い切っても `health:size.growth` には修復が出る。同じ rule に戻るとその bucket の上限で止まる | 同上（「bucket health:deps.violation: 1 回 / max_repairs_per_class 1」） |
| 木の `max_tree_runs`（D4.1、`tree_limit_hold_for`） | `health_limits_cap_tree_runs_hold_health_repair` | health の修復の WU は作られるが、その run は起きない（`WorkerStarted` 0） | root に決定の要求 `limit:max_tree_runs` 1 件、受信箱の `Decision`、Done にならない |
| 木の `max_tree_usd`（D4.1） | `health_limits_cap_tree_usd_holds_health_repair` | 同上（木の費用 4 USD ≥ 上限 1 USD） | 決定の要求 `limit:max_tree_usd` 1 件 |
| task の `max_tokens`（D4.1、`token_budget`） | `health_limits_cap_token_budget_counts_health_repair_runs` | 修復の run の usage 5,000 token が `tree_tokens_used` に入り、`max_tokens` 1,000 を超えると次の review の reviewer を起こさない | 既存の `token_budget_exhausted` で Failed、受信箱の `Failed`、health の評価は 1 回だけ |
| `MAX_TARGET_RESYNCS`（D4.1） | `health_limits_cap_reevaluation_does_not_count_target_resyncs` | 修復の後の評価し直し 4 回（`MAX_TARGET_RESYNCS` + 1 より多い）で `ReviewTargetAdvanced`・`ReviewResyncHalted` は 0 | 修復の上限（`max_repairs_per_class` = 3）で `health_repair_cap:` の Blocked |
| CoS の `review_rework`（上限 2 回、D4.1） | `health_limits_cap_review_rework_sees_only_non_health_failures` | health だけの不合格は Failed にならず review_rework に入らない。health でない失敗、および health の Fail と health でない失敗が混ざる不合格は、評価 1 回につき `review_fail` 1 回で Failed。`Trigger::Reopen` を 2 回しても dispatcher は reopen を足さない（Failed の数 = 1 + reopen の数） | health だけは `health_repair_cap:` の Blocked。混ざる不合格は Failed のまま止まり、修復も質問も足さない |
| 時間 `HEALTH_INPUT_TIMEOUT`（D4.2） | `health_limits_cap_checker_timeout_is_error_without_repair` | 終わらない stub（`HANG`）と、作業ツリーの下だけ 2 秒に縮めた締め切りで、時間切れが Error（理由に「時間」）になり修復に回らない。600 秒待たない（試験は約 2 秒） | `health_check_error:` の Blocked |
| 使い切りの後の J4（D4.4） | `health_limits_cap_exhausted_error_unknown_unsupported_are_not_pass` | `max_repairs` = 1 を使い切った後に comment で再開し、検査器の `error`・`unknown`・`unsupported`（exit 2）のどれも合格にならない | どれも `health_check_error:` の Blocked、修復の数は 1 のまま |

各試験の使い切りの確かめ（`assert_stopped`）は、最後の `review_blocked` の後に run・修復・`AutoReplanRequested` が無いこと、`review_fail` が無いこと、受信箱に `Question` があることを見る。`run_until_idle` が idle で返る（または `settle_quiet` が落ち着く）ことで、終わらない待ちと修復の連鎖が無いことを見る。

D4.1 の残りの上限は他の unit の試験が持つ: `max_replans` は `health_limits_review_compound_repair_cap_does_not_replan`（review-wire）、段の統合の自動修復の上限は `health_limits_integration_repair_cap_blocks`（integration-wire）。`max_tree_replans`・`IntegrationRepair` の上限・案件の `max_usd_per_day` は health の経路が新しい replan・IntegrationRepair・run の種類を足さない（review-wire と integration-wire が health だけの不合格を Blocked で止め、replan・IntegrationRepair を起こさない）ので、この unit では試験を足していない。

## 新しい上限の欄・config の欄を足していないことの確かめ方
- `git diff --stat $CELERIS_WU_BASE` → 変わった file は 3 つだけ: `crates/task-dispatch/src/dispatcher/tests/health_limits_budget.rs`（新規）、`crates/task-dispatch/src/dispatcher/tests/mod.rs`（`mod health_limits_budget;` の 1 行）、`crates/task-dispatch/src/health_review/gate.rs`（`with_checker` の 1 行と `test_timeout` の module）。進捗の file を除く。
- `git diff $CELERIS_WU_BASE -- crates/celeris/src/config crates/task-core/src crates/task-core/migrations crates/task-api/src` → 差分なし（config・Budget・ExecutionLimits・migration・API に変更なし）。

## 変更（commit の件名）
- 「G3 limits budgets: health_limits_cap_ の試験 9 件（6 件合格、木の上限 2 件と時間切れ 1 件は未完）と gate.rs の試験専用の上限の口」: 試験の file と `tests/mod.rs` の 1 行、`gate.rs` の `test_timeout`。
- 「G3 limits budgets: 木の上限と時間切れの試験を直し、review_rework の往復を確かめ、記録を書く」: 木の上限の 2 件を `settle_quiet` にした（決定を待って run を止めた task は ready のまま残るので idle にならない。`tree_gate.rs` と同じ扱い）。`test_timeout` を `health_gate` の入口から `with_checker` に移し、締め切りの `Instant` を早める形にした。review_rework の試験に reopen 2 回の往復を足した。この進捗の file。

## 証拠（提出時の HEAD で実行。最後の同期の後に再実行する）
- WU の check 1（`fn health_limits_cap_` の grep が 9 件 ≥ 8、`cargo nextest run -p task-dispatch -p task-ops -p task-api --no-tests=fail -E 'test(/health_limits_cap_/)'`）→ exit 0、`9 tests run: 9 passed, 3155 skipped`、2.6 秒。
- `cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → 結果は下の「提出時の検査」に書く。

## 未解決
- 無し。

## 提案
- `health_review/review_item.rs` の `requirement_of`・`unbound_note` と `input.rs` は `HEALTH_INPUT_TIMEOUT` から締め切りをそれぞれ作る。1 回の review で git の読み取りと検査器の起動が別々の 600 秒の締め切りを持つので、D4.2 の「評価の回数 × 600 秒」は検査器の起動の数え方としては成り立つが、review 1 回の壁時計の上限は 600 秒より長くなりうる。record の unit で ADR の D4.2 に「締め切りは検査器の起動ごと」と付記するとよい。
