---
title: 健全性v2 G3 limits — health の review・repair の回数・費用・時間を既存の上限の中で数え、同じ指摘の往復を集約して上げる
tasks: [01M70C8NHEANARRKZD9RGKZ87D]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 limits — health の review・repair の回数・費用・時間を既存の上限の中で数え、同じ指摘の往復を集約して上げる

完了日: 2026-11-09

task 01M70C8NHEANARRKZD9RGKZ87D（G3 の段 enforce の unit limits）の記録。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の (4) 上限（D4.1〜D4.4）と、loop-points が残した D3.4 の review の経路を実装した。前の unit は [loop-points](loop-points.md)。基準は `git merge-base HEAD main`。ADR の状態（`状態: 提案`）は G3 の closeout が改めるので変えず、「状態の記録」に付記を 1 行足した。[ADR 2026-10-22-stalled-auto-replan](../../adr/2026-10-22-stalled-auto-replan.md) の D3 にも付記を 1 行足した。この unit（record）はコードを変えていない。

## 結論
- health 由来の修復・review・run は既存の上限で数える。新しい予算の欄・config の欄・表・migration は足していない（下の「新しい上限の欄が無いことの確かめ」）。
- 同じ fingerprint の集合の往復（D4.3）は、WU の check・段の統合・task の review のどれでも、repair・retry・replan を起こさない。源の task を Blocked にし、木の root に event `HealthFindingsRepeated` を追記する。受信箱の `health_repeat` は `(root, set_sha256)` ごとに 1 件で、別の leaf が同じ集合に当たっても 1 件にまとまる。
- 上限の使い切り（D4.4）は、既存の止め方のまま止まる。段の統合と task の review の修復の上限は Blocked と受信箱の質問、木の上限は root の決定の要求、token の予算は既存の `token_budget_exhausted` の Failed。段の統合と compound の review は health のために replan しない。
- health の error・unknown・unsupported・Unavailable・検査器の時間切れは、上限の前でも後でも合格にせず、修復にも回さず Blocked で止める（J4、D3.4）。loop-points の未解決 1（review の Error で task が Failed になる）は review-wire が `Trigger::ReviewBlocked` で直した。
- dispatcher に LLM の呼び出しは無い。判定 `health_review::limits::judge` は events だけを読む純粋関数。`crates/task-dispatch/src/dispatcher.rs` は 2784 行のままで変えていない。
- `health_limits_` の試験は 39 件（task の受け入れ条件 2 は 6 件以上）。
- D1.5 の計測: `check-health.sh` の所要時間の中央値は 99.5 秒（3 回、下の「D1.5 の計測」）。

## 経路ごとの止まり方
| 経路 | 同じ指摘の往復（D4.3） | 修復の上限の使い切り（D4.4） | health の Error・Unavailable（D3.4） | unit |
|---|---|---|---|---|
| WU の check | WU の retry にせず、WU と task を Blocked（質問 `health_findings_repeated: WU <key> の check: …`）。root に `HealthFindingsRepeated{site: work_unit}` | 既存の WU の `max_retries`。指摘が毎回変わって使い切ると今どおり WU は Failed（未解決 1） | Blocked と受信箱の質問（loop-points の integration-path のまま） | integration-wire |
| 段の統合 | repair を足さず、統合 WU と task を Blocked（質問 `health_findings_repeated: 段 <key> の統合: …`）。root に `site: phase` | replan せず Blocked（質問 `health_repair_limit: …`。bucket の回数と上限を書く） | repair・replan をせず Blocked（質問 `health_check_error: …`） | integration-wire |
| task の review | repair を足さず `Trigger::ReviewBlocked`（attempts 不変）で Blocked、質問 `health_findings_repeated: …`。root に `site: review` | `ReviewBlocked` で Blocked（質問 `health_repair_cap: …`）。atomic は再実行せず、compound は replan しない | `ReviewBlocked` で Blocked（質問 `health_check_error: …`）。他の不合格より先に見る | review-wire |

health の不合格に health でない失敗が混ざる場合は、どの経路も今どおり（WU の retry、段の統合は Substantive の replan、review は `ReviewFail`）。人は止まった源の task への comment・answer で再開する。往復の判定は人の再開（`is_human_resume`）の後の最初の評価を往復と数えない。

## 上限ごとの対応
| 上限（ADR D4.1・D4.2） | health 由来の数え方 | 試験 | 使い切りの止まり方 |
|---|---|---|---|
| task の `max_repairs` | health でない修復と合算する | `health_limits_cap_max_repairs_shared_with_non_health_repairs`、`health_limits_review_compound_repair_cap_does_not_replan` | review は `health_repair_cap:` の Blocked |
| `max_repairs_per_class`（class `health:<rule_id>`） | rule ごとの bucket（`check_item::repair_count`。段の統合と review が共有） | `health_limits_cap_per_class_counts_each_rule`、`health_limits_review_repair_cap_blocks_without_rerun`、`health_limits_integration_repair_cap_blocks` | 段の統合は `health_repair_limit:`、review は `health_repair_cap:` の Blocked |
| `max_replans` | health のために replan しない | `health_limits_review_compound_repair_cap_does_not_replan`、`health_limits_integration_repair_cap_blocks`（replan 0） | 計画の版が増えない |
| 段の統合の自動修復の上限 | 上の 2 行と同じ bucket | `health_limits_integration_repair_cap_blocks` | 統合 WU と task が Blocked |
| 木の `max_tree_runs`（`tree_limit_hold_for`） | health の修復の run も木の run に数える | `health_limits_cap_tree_runs_hold_health_repair` | root に決定の要求 `limit:max_tree_runs` |
| 木の `max_tree_usd` | 同上 | `health_limits_cap_tree_usd_holds_health_repair` | root に決定の要求 `limit:max_tree_usd` |
| task の `max_tokens`（`token_budget`） | 修復の run の usage を `tree_tokens_used` に入れる | `health_limits_cap_token_budget_counts_health_repair_runs` | 既存の `token_budget_exhausted` の Failed |
| `MAX_TARGET_RESYNCS` | 修復の後の評価し直しは同期に数えない（同期の回数を食わない） | `health_limits_cap_reevaluation_does_not_count_target_resyncs` | 修復の上限で `health_repair_cap:` の Blocked |
| CoS の `review_rework`（2 回） | health だけの不合格は Failed にならず review_rework に入らない | `health_limits_cap_review_rework_sees_only_non_health_failures` | health だけは Blocked。混ざる不合格は今どおり Failed |
| 時間 `HEALTH_INPUT_TIMEOUT`（600 秒、D4.2） | 時間切れは Error | `health_limits_cap_checker_timeout_is_error_without_repair`（締め切りを試験専用の口で 2 秒に縮める） | `health_check_error:` の Blocked |
| 使い切りの後の J4（D4.4） | error・unknown・unsupported を合格にしない | `health_limits_cap_exhausted_error_unknown_unsupported_are_not_pass` | `health_check_error:` の Blocked |
| 同じ指摘の往復（D4.3） | 修復の前後の fingerprint の集合を比べる | `health_limits_repeat_*`、`health_limits_*_same_fingerprint_*`、`health_limits_same_fingerprint_escalates_once`、`health_limits_inbox_*` | 源は Blocked、root の受信箱に 1 件 |

- `max_tree_replans`・`IntegrationRepair` の上限・案件の `max_usd_per_day` は試験を足していない。health の経路は replan・IntegrationRepair・新しい run の種類を足さない（health だけの不合格は Blocked で止まる）ので、health 由来の数えが増えない（budgets の判断）。
- task の `max_turns`・`max_wall_secs`: health の修復の run は既存の `RepairClass::Health` の予算（30 turn・1800 秒。loop-points）で動き、run の壁時計は既存の `max_wall_secs` のまま（D4.2）。limits はこの 2 つの数えを変えておらず、専用の試験は無い。

### 新しい上限の欄が無いことの確かめ
- 「integrate wu/record (phase close)」（loop-points の最後の統合）から HEAD までの差分で、`crates/celeris/src/config`・`crates/task-core/migrations`・`crates/task-core/src/store`・`crates/task-core/src/pricing*`・`crates/task-core/src/quota*`・`crates/task-ops/src/project_budget*`・`crates/task-dispatch/src/dispatcher/token_budget.rs`・`dispatcher/quota_book.rs` の変更は 0 件（`git diff --name-only <その commit> HEAD -- <path…>`）。
- 足したのは event の種類 1 つ（`health_findings_repeated`、追記専用）、受信箱の種類 1 つ（`health_repeat`）、`Trigger::ReviewBlocked`（reason `review_blocked`）、質問の先頭の語 3 つ（`health_findings_repeated`・`health_repair_limit`・`health_repair_cap`。既存の `health_check_error` は review の経路でも使うようになった）と、試験専用の締め切りの口（`gate.rs` の `#[cfg(test)]` の `test_timeout`。本番の build では締め切りを変えない）。

## 各 unit の記録
| unit | 内容 | 記録 |
|---|---|---|
| repeat | 往復の判定の純粋関数 `health_review::limits::judge`（範囲 WU・段・review、repo ごと）と、人の再開の判定 `plan_gate::is_human_resume` の共有 | [limits-repeat.md](limits-repeat.md) |
| escalation | event `HealthFindingsRepeated`、root への追記 `task_ops::inbox::health_repeat::record_findings_repeated`、受信箱 `health_repeat`、API の schema・web の表示と realtime | [limits-escalation.md](limits-escalation.md) |
| integrate-core | 段 core（repeat ∥ escalation）の統合。merge の repair の unit は無い | この file |
| integration-wire | WU の check と段の統合で、往復と修復の上限の使い切りを Blocked と root の受信箱で止める。replan を止める語の一覧 `check_item::stops_without_replan` | [limits-integration-wire.md](limits-integration-wire.md) |
| review-wire | task の review の Error・往復・上限の使い切りを `Trigger::ReviewBlocked` と受信箱の質問で止める（`health_review/review_limits.rs`） | [limits-review-wire.md](limits-review-wire.md) |
| integrate-wire・merge-wire-review-wire | 段 wire の統合。`dispatcher/tests/health_enforce.rs` の stub の検査器の衝突を、integration-wire の `FP`（数）と review-wire の `FINDING`（fingerprint の 1 行）を両方保って解いた（`FINDING` を優先し、無ければ `FP`、どちらも無ければ `deps.violation:0000000000000001`）。`FP` の path の変数は `fp_file` に改名 | この file |
| budgets | 既存の各上限の中で数えることを `health_limits_cap_` の 9 件で固定。試験専用の締め切りの口 | [limits-budgets.md](limits-budgets.md) |
| integrate-bounds | 段 bounds の統合。merge の repair の unit は無い | この file |
| record | この記録・ADR 2 本の付記・D1.5 の計測・提出時の検査（コードは変えていない） | この file |

## health_limits_ の試験（39 件）
各試験が守る不変条件と、既存の試験で検出できない理由は各 unit の記録にある。数は `grep -rhoE "fn health_limits_[a-z0-9_]+" crates/ | sort -u | wc -l` で数えた。

| prefix・名前 | 件数 | file | unit | 確かめること |
|---|---|---|---|---|
| `health_limits_repeat_` | 8 | `task-dispatch/src/health_review/limits_tests.rs` | repeat | 同じ集合・消えた fingerprint の再出現は Repeat、別の集合・初回・Error の行・人の再開の後は Proceed、範囲と repo を混ぜない、集合の鍵は順に依らない |
| `health_limits_inbox_` | 6 | `task-core/src/model/health_repeat.rs`（1）・`task-ops/src/inbox/health_repeat/tests.rs`（5） | escalation | `(root, 集合)` ごとに 1 件、源が Blocked を出るか root が終端なら消える、範囲つきの受信箱も同じ項目、event の serde |
| `health_limits_work_unit_` | 3 | `task-dispatch/src/dispatcher/tests/health_limits_integration.rs` | integration-wire | WU の check の往復は retry 1 回で Blocked、指摘が変われば retry を続ける、混ざる失敗は retry |
| `health_limits_same_fingerprint_escalates_once` | 1 | 同上 | integration-wire | 2 つの子 task の同じ集合は root の受信箱に 1 件 |
| `health_limits_integration_` | 3 | 同上 | integration-wire | 段の統合の往復・修復の上限の使い切り・Error は Blocked で、replan しない |
| `health_limits_stops_without_replan_only_health_stops`・`health_limits_only_health_fails_ignores_mixed_rows` | 2 | `task-dispatch/src/health_review/check_item_tests.rs` | integration-wire | replan を止める語は 3 種だけ、往復の判定は health だけの不合格の列だけ |
| `health_limits_review_` | 7 | `task-dispatch/src/dispatcher/tests/health_limits_review.rs` | review-wire | review の Error・Unavailable・往復・上限の使い切りは `ReviewBlocked`、compound は replan しない、混ざる失敗は `ReviewFail`、comment で再開 |
| `health_limits_cap_` | 9 | `task-dispatch/src/dispatcher/tests/health_limits_budget.rs` | budgets | 上の「上限ごとの対応」 |

時計に依る試験は無い。dispatcher の試験は `run_until_idle`（出来事が尽きるまで tick を回す）か `settle_quiet`（決定待ちの場面）で決まり、時間切れの試験は終わらない stub と試験専用の 2 秒の締め切りで決まる（約 2 秒）。外部ネットワークに出ず、LLM を呼ばない。CPU を焼く負荷は無い。

## commit（件名）
- repeat: 「G3 limits repeat: health の同じ指摘の往復を判定する health_review::limits と、人の再開の判定 is_human_resume の共有」「G3 limits repeat: 往復の判定の記録と提出時の検査」
- escalation: 「G3 limits escalation: HealthFindingsRepeated と root の受信箱 health_repeat」「G3 limits escalation: 進捗に範囲の外の 2 file の申告のやり直しを書く」「G3 limits escalation: web の realtime の種類と invalidation に health_findings_repeated を足す」「G3 limits escalation: 進捗を done にし提出時の検査の結果を書く」
- 段 core の統合: 「integrate wu/escalation (phase core)」「integrate wu/repeat (phase core)」
- integration-wire: 「G3 limits integration-wire: WU の check と段の統合で health の往復と修復の上限の使い切りを Blocked と root の受信箱で止める」「G3 limits integration-wire: 進捗に提出時の検査の結果を書く」
- review-wire: 「G3 limits review-wire: review の health の Error・往復・修復の上限の使い切りを ReviewBlocked と受信箱の質問で止める」「G3 limits review-wire: transition の Blocked → Ready の同形の分岐を 1 つにまとめ、関数の行を基準より減らす」「G3 limits review-wire: health の止め方を free 関数の module にまとめ、Dispatcher の method と dispatcher の行を増やさない」「G3 limits review-wire: health の止め方を health_review/review_limits.rs に移して #[path] で読み、health_review_input の Error の試験を Blocked に直す」と進捗の commit 3 件
- 段 wire の統合: 「integrate wu/integration-wire (phase wire)」「integrate wu/review-wire (phase wire)」（後者が merge-wire-review-wire の衝突の解消を含む）
- budgets: 「G3 limits budgets: health_limits_cap_ の試験 9 件（6 件合格、木の上限 2 件と時間切れ 1 件は未完）と gate.rs の試験専用の上限の口」「G3 limits budgets: 木の上限と時間切れの試験を直し、review_rework の往復を確かめ、記録を書く」「G3 limits budgets: 進捗に提出時の test-affected の結果を書く」
- 段 bounds の統合: 「integrate wu/budgets (phase bounds)」
- record: 「G3 limits record: limits の記録・ADR の付記・check-health の所要時間」と、証拠の節を書いた commit

## D1.5 の計測（`edit_verify_secs` の増分）
- 方法: 提出時の HEAD（作業ツリーに未 commit の変更なし）で `bash scripts/dev/check-health.sh --out "$TMPDIR/ch/r<n>.json"` を 3 回、1 回ずつ順に実行し、前後の `date +%s%N` の差を取った。他の cargo・試験は並走させていない。負荷はかけていない（計測の直後の load average は 1 分 2.80、host は 24 core）。
- 結果: 104.6 秒・92.6 秒・99.5 秒。3 回とも exit 0、report の `inputs.mode` は `full`、`repo_health_gate: all checks passed`。中央値は 99.5 秒で、これを `edit_verify_secs` の増分とする。
- 内訳: 1 回の中に、`repo_health.py` の report に加えて検査器自身の unit 試験（2 回目の log で `Ran 73 tests in 12.843s` と `Ran 1 test in 0.401s`）とゲートの試験 7 件（`repo_health_gate.sh`）を含む。ゲートの試験の時間は分けて測っていない。
- 比較: B0 の基準（[slices.json](../2026-11-07-health-v2-b0/slices.json) の `baseline_metrics.edit_verify_secs`、test-affected の対象 crate 全部の `nextest --no-run` の秒）は task-dispatch 79.84 秒・task-core 142.25 秒・task-api 113.46 秒・task-worker 98.13 秒・celeris 85.06 秒。`test-affected.sh` は check-health を nextest の前に順に呼ぶので、1 回の edit → verify はこの基準より約 99.5 秒長い。
- 注: task の Objective の書き方 `sh scripts/dev/check-health.sh` は dash では 13 行目の `${BASH_SOURCE[0]}` で `Bad substitution`（exit 2）になり、検査器を起こさずに終わる。`test-affected.sh` は 45 行目で `bash scripts/dev/check-health.sh` と呼ぶので、同じ `bash` で測った。

## task の目的文・ADR との違い
closeout が ADR の付記に写すための一覧。1・2・4 は ADR の「状態の記録」の付記に書いた。
1. D3.4 の review の経路: loop-points の時点では review の health の Error で task が Failed になっていた。review-wire が `Trigger::ReviewBlocked` を足し、Blocked と受信箱の質問 1 件にした。既存の試験 `health_review_policy_unknown_flow_missing_report_is_not_a_pass` の期待を Failed から Blocked に直した（契約の変更）。
2. D4.4 の段の統合: 修復の上限を使い切ると `integration_gives_up_with` が `max_replans` の余りで replan していた。replan を止める語の一覧 `check_item::stops_without_replan`（`health_check_error`・`health_findings_repeated`・`health_repair_limit`）で止めた。
3. ADR の「後続 unit の path・試験名の prefix」節の例のうち、`health_limits_fail_routes_to_repair` は loop-points の `health_enforce_*`（Fail を repair へ）が、`health_limits_ungrounded_llm_finding_does_not_block` は `health_enforce_llm_finding_*`（loop-points の grounding）が同じ不変条件を確かめるので、limits では書いていない。`health_limits_error_blocks_without_repair` は `health_limits_review_error_blocks_without_repair` と `health_limits_integration_error_blocks_without_repair`、`health_limits_repair_cap_blocks` は `health_limits_integration_repair_cap_blocks` と `health_limits_review_repair_cap_blocks_without_rerun` になった。
4. `stalled_replans` の契約: 進展の無い自動 replan の連なりは、人の replan の依頼・answer に加えて comment・reopen でも切れる（`is_human_resume` を health の往復の判定と共有したため）。ADR 2026-10-22-stalled-auto-replan の D3 に付記した。`pending_integration_auto_replan` は今どおり replan の依頼・answer だけで切る。
5. D4.2 の締め切り: review の 1 回の評価で、git の読み取りと検査器の起動がそれぞれ `HEALTH_INPUT_TIMEOUT` から締め切りを作る（budgets の提案）。「評価の回数 × 600 秒」は検査器の起動の数え方としては成り立つが、review 1 回の壁時計は 600 秒より長くなりうる。

## 本番への移行
- config・DB の表・migration の変更は無い。本番の config の書き換えは要らない。
- 増えるもの: event の種類 `health_findings_repeated`（木の root に追記。古い events はそのまま読める）、受信箱の種類 `health_repeat`（表示名「修復の後も同じ health の指摘が残る」。answer は 409 `native_action_required` で、人は源の task への comment で再開する）、遷移の reason `review_blocked`（replay は attempts を進めない理由として読む）。API の schema（`docs/api/v1`）と web の生成型・realtime の種類は同じ release で入る。
- release を昇格すると、health の往復と修復の上限の使い切りで task が Blocked になり、受信箱に源の task の質問と root の `health_repeat` が出る。前は repair・retry・replan・Failed（CoS の review_rework）に進んでいた。
- `stalled_replans` は comment・reopen でも数え直すので、人が手を入れた後の自動 replan は新しい連なりとして 1 から数える。
- 人の操作は要らない。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
（検査の後に書く）

## 未解決
1. WU の check の health だけの不合格で指摘が毎回変わり、WU の retry（`max_retries`）を使い切った場合は今どおり WU が Failed になる。その後に自動 replan が起きるか（D4.4「health のために replan を足さない」に当たるか）は試験で確かめていない（未確認。integration-wire の未解決 1 を budgets は扱っていない）。
2. 修復の上限の使い切りの質問の先頭の語が、段の統合は `health_repair_limit:`（`check_item.rs`）、task の review は `health_repair_cap:`（`review_limits.rs`）で違う。意味は同じで、CoS・人が語で見分けるときに 2 つを知る必要がある。
3. `health_review::limits` は `RepeatVerdict::Repeat.set_sha256` を自前の `set_sha256`（`limits.rs`）で求める。規則（重複を除いて昇順、改行でつないで sha256）は `task_core::health_repeat::fingerprint_set_sha256` と同じで、同じ仕様が 2 か所にある。本番の呼び出し側（`check_item.rs`・`review_limits.rs`）はこの値を使わず、受信箱の鍵は task-core が作る（escalation の提案 2 が採られていない）。
4. 往復の判定と root への追記の組（`limits::judge` → `record_findings_repeated`）が `check_item::health_repeat_stop`（WU の check・段の統合）と `review_limits::stop_on_repeat`（task の review）の 2 か所にある（integration-wire の提案 1 が採られていない）。
5. 往復で止めた源の task は Blocked なので、受信箱には root の `health_repeat` とは別に源の質問の項目が出る。root が源と同じ task でも 2 件になる（review-wire の提案 1）。
6. check-health の optional の warning 2 件（`dup.contract`: `task_core::required_checks::health::UnavailableReason` と `route_select::availability::UnavailableReason` の同名、`dup.public-surface`: `health_review/limits.rs` の `fn judge` と `campaign/src/jev_smoke.rs`）が new として残る。合否には使わない。

## 提案
1. 修復の上限の使い切りの質問の語を 1 つにそろえる（未解決 2。`check_item.rs` に定数を置き、`review_limits.rs` も使う）。
2. `limits.rs` の `set_sha256` と `RepeatVerdict::Repeat.set_sha256` を消すか、`task_core::health_repeat::fingerprint_set_sha256` を呼ぶ（未解決 3。規約 2 `duplicate_implementation`）。
3. 往復の判定と root への追記を 1 つの関数にし、WU の check・段の統合・task の review が同じ関数を呼ぶ（未解決 4）。
4. 受信箱で、`health_repeat` の源である間は源の質問の項目を出さないかを決める（未解決 5）。
5. CoS の一次対応の方針で、`health_check_error:`・`health_repair_cap:`・`health_repair_limit:`・`health_findings_repeated:` の質問を人に上げる規則を置くかを決める。CoS が自由記述で答えると、指摘や環境が直らないまま再開して同じ理由でまた止まりうる（止まるたびに質問 1 件で、修復・再実行は増えない。review-wire の提案 2）。
6. closeout が ADR の D4.2 に「締め切りは検査器の起動ごと」と付記する（上の「違い」5、budgets の提案）。
7. `HealthRepeatSite` の module を `crates/task-core/src/lib.rs` の `#[path = "model/health_repeat.rs"]` から `model.rs` の `mod health_repeat;` と `pub use` に移す（escalation の提案 1）。
8. D1.5: Celeris の repo では 1 回の edit → verify に check-health が約 99.5 秒足される。D1.2 の health の項目（commit の評価）と重なる分を減らすかを、別の task で決める（[G3 の task の記録](../2026-11-07-health-v2-g3.md) の D1.5 の提案と同じ）。
