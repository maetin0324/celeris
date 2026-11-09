---
title: 健全性v2 G3 loop-points — 通常の開発・統合ループの全経路に同じ適用判定と health 検査を入れ、決定的な違反は repair へ
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 loop-points — 通常の開発・統合ループの全経路に同じ適用判定と health 検査を入れ、決定的な違反は repair へ

完了日: 2026-11-09

task 01M6ZDF9Q4BWD9DX6S9KX3KKS1（G3 の段 enforce の unit loop-points）の記録。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の (1) 適用判定と (3) 修復を、D1.2 の 7 経路に配線した。証跡と適用判定の関数は前の段 [evidence](evidence.md) が作った。基準は `git merge-base HEAD main`。ADR の状態（`状態: 提案`）は G3 の closeout が実装済みにするので、この記録では変えず、「状態の記録」に付記を 1 行だけ足した。

## 結論
- 7 経路はどれも経路共通の入口 `health_review::gate::health_gate`（または review が記録した証跡）を通り、同じ適用判定 `task_core::required_checks::health::{repo_facts, applicability}` を使う。経路ごとの検査器の起動は残っていない（下の「経路ごとの起動の撤去」）。
- 統合を止めるのは決定的な検査の fail 級の finding（new か worsened の error）だけで、WU の retry・段の統合の自動修復・review の repair に回る。error・unknown・unsupported・検査器の欠け・事実の読み取りの失敗は合格にしない。根拠の無い LLM の health finding は advisory に下げ、統合を止めない。
- Celeris でない repo（base に検査器が無く、test 台本が check-health を呼ばない repo）でも、release に同梱した検査器で同じ判定が働く（試験 `health_enforce_non_celeris_*` 2 件）。
- `health_enforce_` の試験は 52 件（task の受け入れ条件 2 は 8 件以上）。

## 7 経路と入口の対応
| 経路 | 入口（file） | health の評価 | Fail | Error・Unavailable | unit | 試験 |
|---|---|---|---|---|---|---|
| 1. atomic/direct の task の review（計画なしの直行経路を含む） | `Dispatcher::spawn_review`（`dispatcher/review_spawn.rs`）→ `HealthInputJob::attach`（`health_review/input.rs`）→ `review_item::evaluate_all`（`health_review/review_item.rs`） | `gate::health_gate`。base は `target_sha`、head は `reviewed_sha`。`review/required.rs` の `evaluate` が必須検査の列の最後に項目を足し、指紋を `definition_digest(cmds, 検査器)` にする。同じ束縛の G2 の評価があれば検査器を起こさずそれを使う | `Verdict::implicit_check` が `Check::Command { cmd: HEALTH_CHECK_ID }` を返し、`classify_review_failure` → `try_review_repair`（class `health:<rule_id>`） | `health_check_error: <理由>` の不合格。repair に回さず、task は Failed（未解決 1） | review-path・flow | `health_enforce_direct_review_*` 6 件、`health_enforce_non_celeris_direct_review_routes_to_repair` |
| 2. WU の check | `Dispatcher::spawn_work_unit_checks`（`dispatcher/work_units.rs`）→ `phase_integration::run_integration_checks`（引数 `health`）→ `check_item::run_item`（`health_review/check_item.rs`） | `gate::health_gate`。base は `wu.base_commit`、head は WU の HEAD。cmd の検査の列の最後に項目を足す | `WorkUnitChecksFailed`（detail `health:<rule_id>:<hash>…（evidence: <path>）`）で WU の retry | `Dispatcher::settle_health_row` が run を質問にし、task は Blocked | integration-path | `health_enforce_work_unit_check_*` 3 件、`health_enforce_non_celeris_work_unit_check_routes_to_repair` |
| 3. 段の統合 | `Dispatcher::start_integration`（`dispatcher/phase_integration.rs`）→ `run_integration_checks` → `check_item::run_item` | base は段の基点（`check_item::phase_target`）、head は統合の HEAD。cmd の検査の無い段も評価する | `schedule_integration_check_repair`（`RepairClass::Health`。数えは `check_item::repair_count` の `health:<rule_id>`） | `integration_gives_up_with` が replan せず Blocked と受信箱の質問 | integration-path | `health_enforce_phase_integration_*` 4 件 |
| 4. review の再利用 | `Dispatcher::reviewer_reuse`（`review_spawn.rs`）→ `evaluate_all`（同じ task の events を渡す） | `required_checks::reuse_health` が同じ `(repo, base, head の tree, policy, 検査器)` の Pass を写し、今の `(base, head)` に束縛し直して `reused_from` を書く。合わなければ `health_gate` で評価し直す。cmd の再利用 `reusable_passes` は検査器を含む指紋で比べる | 1 と同じ | 1 と同じ | reuse-sync | `health_enforce_reviewer_reuse_*` 6 件 |
| 5. review 前の main 同期・rebase | `spawn_review` の `ReviewSync`（ADR-0118）・`defer_stale_review` | 同期の後の `(target_sha, reviewed_sha)` を束縛にして `evaluate_all` を通る。head の tree は評価の時に読むので、rebase で tree が変われば前の証跡を写さない | 1 と同じ | 1 と同じ | reuse-sync | `health_enforce_pre_review_sync_rebase_changes_the_tree_and_reevaluates` |
| 6. 統合の修復の後 | 段の統合の修復（`schedule_integration_check_repair`・`schedule_merge_repair`）の後の再統合、review の修復（`try_review_repair`）の後の review | 修復の後の新しい head で 3 か 1 の経路に戻って評価し直す。修復の前の証跡は束縛が違うので写さない | 3・1 と同じ | 3・1 と同じ | integration-path・review-path・flow | `health_enforce_integration_repair_*` 4 件、`health_enforce_non_celeris_*` 2 件（修復の後に Pass） |
| 7. delivery の merge candidate | `celeris::delivery::check_required_evidence`（`crates/celeris/src/delivery.rs`）→ `required_checks::verify_required_checks` → `health_evaluated_target` | 検査器を起こさず、review が記録した証跡を照合する。適用判定は `target_sha` の事実で呼ぶ（evidence の delivery-verify）。証跡の `base_sha` が束縛の `target_sha` と違えば通さない（advisory 1） | 通さず再 review（`request_rereview`） | 通さず再 review。PolicyOnly も通さない | delivery-path | `health_enforce_delivery_*` 3 件 |

### 経路ごとの起動の撤去
- 検査器の report の起動は `health_review::gate` の `check_with`（中の `run_report`）の 1 つだけ。経路が呼ぶ入口は `gate::health_gate` で、呼び出しは `health_review/check_item.rs`（経路 2・3）と `health_review/review_item.rs`（経路 1・4・5）の 2 か所。`run_health_check` は private にした（entry）。
- G2 の材料（`health_review/input.rs`）と review 方針（`health_review/policy.rs` の `load_review_policy`）は `gate::with_applicable_checker` を通り、同じ適用判定で検査器を選ぶ。G2 の材料は `check_with` の report を共有し、同じ候補で検査器を 2 回起こさない（g2-sites）。
- `grep -n 'CheckerSite::Base' crates/task-dispatch/src/health_review/input.rs crates/task-dispatch/src/health_review/policy.rs` は 0 件。`CheckerSource` から `CheckerSite` への写しは `gate.rs` の `checker_site` の 1 か所だけ（evidence の「loop-points への引き継ぎ」の撤去の条件）。
- 消したもの: `spawn_review` が `reusable_passes` を直接呼んでいた分岐（reuse-sync）、`repo_facts` の `git --version` の probe（facts）、WU の check と段の統合で 2 つに分かれていた検査の項目の event の組み立て（`check_item::record_start`・`record_finish` の 1 つにした。integration-path）、review の修復の独自の class の数え（flow）、G2 の「base に検査器が無ければ当てない」3 か所と定数 `NO_CHECKER`（g2-sites）。

## 各 unit の記録
| unit | 内容 | 記録 |
|---|---|---|
| entry | 経路共通の入口 `health_gate` と結果の分類（`HealthGateDecision`: NotApplicable・Pass・PolicyOnly・Repair・Error）、rule ごとの repair の材料 `health_repairs` | [loop-points-entry.md](loop-points-entry.md) |
| facts | `repo_facts` の rev-parse の読み取りの失敗を `facts_unreadable` にする（advisory 2） | [loop-points-facts.md](loop-points-facts.md) |
| g2-sites | G2 の 3 か所を適用判定に付け替え、試験の土台と dagbench の driver に合格の stub の同梱の検査器を設定する | [loop-points-g2-sites.md](loop-points-g2-sites.md) |
| grounding | 根拠の無い LLM の health finding を `health:ungrounded` の advisory に下げる（D3.3） | [loop-points-grounding.md](loop-points-grounding.md) |
| integration-path | WU の check・段の統合・統合の修復の後に health を当て、Fail を既存の repair へ、Error を Blocked へ | [loop-points-integration-path.md](loop-points-integration-path.md) |
| review-path | atomic/direct の review の必須検査に health の項目を足し、Fail を review の repair へ | [loop-points-review-path.md](loop-points-review-path.md) |
| reuse-sync | review の再利用と review 前の main 同期で health を束縛どおりに扱う | [loop-points-reuse-sync.md](loop-points-reuse-sync.md) |
| delivery-path | delivery で health の証跡の `base_sha` を `target_sha` と照合する（advisory 1） | [loop-points-delivery-path.md](loop-points-delivery-path.md) |
| flow | review の修復の class を rule ごとに直し、Celeris でない repo の流れの試験を足す | [loop-points-flow.md](loop-points-flow.md) |
| record | この記録・ADR の付記・提出時の検査（コードは変えていない） | この file |

## commit（件名）
- entry: 「G3 loop-points entry: 経路共通の入口 health_gate と結果の分類・Fail の repair の材料」「G3 loop-points entry: 進捗」
- facts: 「G3 loop-points facts: rev-parse の読み取りの失敗を facts_unreadable にする」「G3 loop-points facts: 進捗の記録」
- grounding: 「phase 1: ground blocking health findings in candidate evidence」
- g2-sites: 「G3 loop-points g2-sites: G2 の 3 か所を適用判定に付け替え、G2 の材料は経路共通の評価の report を使う」「G3 loop-points g2-sites: 試験の土台と dagbench の driver に合格の stub の同梱の検査器を設定する」と進捗の commit 4 件
- 段 core の統合: 「integrate wu/facts (phase core)」「integrate wu/g2-sites (phase core)」「integrate wu/grounding (phase core)」
- integration-path: 「G3 loop-points integration-path: WU の check・段の統合・統合の修復の後に health を当て、Fail を既存の repair へ、Error を Blocked へ」「G3 loop-points integration-path: health の項目の評価・対象・質問を health_review::check_item に移し、dispatcher の行を減らす」「G3 loop-points integration-path: check_item を dispatcher::phase_integration の子 module にし、検査の項目の event 記録を共通化して dispatcher の行と依存の循環を戻す」「G3 loop-points integration-path: WU の check の health の項目に合わせて G2 の scenario の stub の report と drain の引き継ぎの試験を直す」と進捗の commit 2 件
- review-path: 「G3 loop-points review-path: review の必須検査に health の項目（途中。既存の試験 14 件が未対応）」「G3 loop-points review-path: 必須検査の件数の試験を health の項目込みにする」「G3 loop-points review-path: health の項目の型を review に移して循環を解き、G2 の試験を G3 の規則に合わせる」「G3 loop-points review-path: 試験の整形」「G3 loop-points review-path: 進捗を done にする」
- reuse-sync: 「G3 loop-points reuse-sync: health の再利用を束縛どおりにし reused_from を書く」「G3 loop-points reuse-sync: 必須検査の再利用を evaluate_all に移して循環を作らない」「G3 loop-points reuse-sync: 進捗に証拠を追記」
- delivery-path: 「G3 loop-points delivery-path: delivery で health の証跡の base_sha を束縛の target_sha と照合する」「G3 loop-points delivery-path: 進捗を書く」
- 段 paths の統合: 「integrate wu/integration-path (phase paths)」「integrate wu/delivery-path (phase paths)」（`health_review/scenario_tests.rs` の衝突を、integration-path の finding ごとの severity と delivery-path の `"rules": []` を両方保って解いた）「integrate wu/reuse-sync (phase paths)」
- flow: 「G3 loop-points flow: Celeris でない repo の WU の check と direct の review の流れの試験を足す」「G3 loop-points flow: review の修復の class を段の統合と同じ rule ごとの bucket で数える」と進捗の commit 3 件
- record: 「G3 loop-points record: loop-points の記録・ADR の付記と提出時の検査」（この記録）と、証拠の節を書いた commit

## health_enforce_ の試験（52 件）
各試験が守る不変条件と、既存の試験で検出できない理由は各 unit の記録にある。数は `grep -rhoE "fn health_enforce_<prefix>_[a-z0-9_]+" crates/ | sort -u | wc -l` で数えた。

| prefix | 件数 | file | unit |
|---|---|---|---|
| `health_enforce_entry_` | 8 | `task-dispatch/src/health_review/entry_tests.rs` | entry |
| `health_enforce_facts_` | 3 | `task-core/src/required_checks/health_tests.rs` | facts |
| `health_enforce_g2_` | 5 | `health_review/{policy/tests.rs, input/tests.rs, policy_review/unknown_tests.rs, stub_checker/tests.rs}` | g2-sites |
| `health_enforce_llm_finding_` | 7 | `health_review/verdict/tests.rs` | grounding |
| `health_enforce_work_unit_check_` | 3 | `dispatcher/tests/health_enforce.rs` | integration-path |
| `health_enforce_phase_integration_` | 4 | `dispatcher/tests/health_enforce.rs`・`health_review/check_item_tests.rs` | integration-path |
| `health_enforce_integration_repair_` | 4 | `dispatcher/tests/health_enforce.rs`・`health_review/check_item_tests.rs`・`health_review/review_item_tests.rs` | integration-path・review-path |
| `health_enforce_direct_review_` | 6 | `health_review/review_item_tests.rs` | review-path |
| `health_enforce_reviewer_reuse_` | 6 | `task-core/src/required_checks/tests.rs`・`health_review/reuse_tests.rs` | reuse-sync |
| `health_enforce_pre_review_sync_` | 1 | `health_review/reuse_tests.rs` | reuse-sync |
| `health_enforce_delivery_` | 3 | `celeris/src/delivery/health_enforce_tests.rs` | delivery-path |
| `health_enforce_non_celeris_` | 2 | `dispatcher/tests/health_enforce.rs` | flow |

## review の修復の class の決着（flow）
- 食い違い: task の review の修復（`dispatcher/review_verdict.rs` の `try_review_repair`）は、同じ class の修復の数を `RepairClass::Health` の `bucket()`（`health`）で独自に数え、repair WU の title も `repair (health): 修復` だった。ADR D3.2・D4.1 は class を rule ごとの `health:<rule_id>` と定め、段の統合は `health_review::check_item::repair_count` で rule ごとに数えていた。
- 直し: `try_review_repair` は失敗の reason を先に作り、段の統合と同じ `check_item::repair_count` で bucket と既にある修復の数を得る。title は `repair (<bucket>): 修復`。`repair_count` の可視性を `pub(in crate::dispatcher)` に広げた。health 以外の class の数えは前と同じ。同じ数え方を 2 か所に書いていない。
- 試験: `health_enforce_non_celeris_direct_review_routes_to_repair`（review の修復の `RepairScheduled` の class が `["health:deps.violation"]`）。直す前の本体では `left: ["health"]`・`right: ["health:deps.violation"]` で落ち、直した後に合格した。
- 経緯: flow の最初の run は不具合を見つけたが plan_issue を checkpoint にだけ書いて done を返し、計画の check で落ちた。2 回目の run が `yield.plan_issue` で申告し、replan で `review_verdict.rs` と `check_item.rs` が flow の範囲に入った。

## operator の advisory 2 件の決着
evidence（01M6YJF7HJ9J0EQHRM5SN5TWV4）の final review が残し、operator のコメント（2026-11-09）が loop-points に引き継いだ 2 件。
1. delivery の照合が health の証跡自身の `base_sha` を event の `target_sha` と比べていない → delivery-path が直した。
   - `crates/celeris/src/delivery.rs` の `check_required_evidence` は、`verify_required_checks` が合格した後に `health_evaluated_target` を呼ぶ。`(repo, target_sha, reviewed_sha)` に束縛された最新の `RequiredChecksEvaluated` の health の項目の `base_sha` が `target_sha` と違えば `Err` を返し、配送の行の `detail` に理由を残して再 review に戻す（ff しない）。
   - 試験: `health_enforce_delivery_base_sha_mismatch_is_rejected`（`base_sha` だけが違う証跡は取り込まれない。直す前は合格していた入力）、`health_enforce_delivery_review_evidence_passes`（review と同じ経路で作った証跡は通り、`base_sha` が `target_sha` と同じ。evidence の delivery-verify の提案 1）。
   - 照合を置いた場所は `delivery.rs`。正本を `required_checks::verify_health` に移す提案は「提案」2 に書いた。
2. `repo_facts` が `git rev-parse --show-toplevel` の一時的な失敗を `not_a_git_repo` に写している → facts が直した。
   - `crates/task-core/src/required_checks/health.rs` の `repo_facts` は、rev-parse の失敗の文が git の「Git 管理でない」の文（定数 `NOT_A_GIT_REPO_STDERR`）を含むときだけ `NotApplicable(not_a_git_repo)` にし、時間切れ・壊れた gitfile・`dubious ownership`・git の起動の失敗などは `Unavailable(facts_unreadable:git rev-parse --show-toplevel を読めない: <理由>)` にする（D1.1 の fail closed）。`git --version` の probe は消した。
   - 試験: `health_enforce_facts_confirmed_non_repo_is_not_applicable`、`health_enforce_facts_transient_rev_parse_failure_is_unavailable`、`health_enforce_facts_git_cannot_start_is_unavailable`。入口では `health_enforce_entry_unavailable_is_not_pass` が `facts_unreadable:` を Error（合格にしない）として確かめる。

## task の目的文・ADR との違い
closeout が ADR の付記に写すための一覧。1〜3 は ADR の「状態の記録」に付記を 1 行足した。
1. 試験名の prefix: task の目的文の `health_enforce_` を使った。ADR の「後続 unit の path・試験名の prefix」節は loop-points を `health_gate_path_`（7 件以上）と書く。delivery-verify が提案した `health_gate_path_delivery` は `health_enforce_delivery_review_evidence_passes` になった。
2. D3.3 の担当: ADR は D3 の試験（`health_limits_ungrounded_llm_finding_does_not_block`）を limits に割り当てるが、根拠の判定は loop-points の unit grounding が `health_review/verdict.rs` に実装した（試験 `health_enforce_llm_finding_` 7 件）。task の目的文の「具体的な証拠（path・行）」に合わせ、D3.3 の anchor（report の fingerprint・`owner_rows` の行・`brief.removals` の項目）に加えて、候補の差分の path と正の行番号の引用（`path:12`・`path#L12`）も根拠にする。
3. delivery の PolicyOnly: 通さない（core-binding の提案 1 の決着。delivery-path）。D2.6 の「PolicyOnly で G2 の `policy_review_gate` の承認があれば通す」は実装していない。G2 の承認は `Approval` の子 task の状態と `ReviewVerdict.reason` の自由文にしかなく、`(repo, target_sha, reviewed_sha)` に結び付ける event の欄が無いため。試験は `health_enforce_delivery_policy_only_is_rejected_without_bound_approval`。
4. D3.4 の review の経路: review の health の Error・Unavailable は `health_check_error: <理由>` の不合格で repair に回さないが、task は Blocked でなく Failed になり、受信箱の `health_check_error` の項目も出ない（reviewing → blocked の Trigger が無い。review-path の未解決）。WU の check と段の統合は D3.4 のとおり Blocked と受信箱の質問。
5. D1.3 の口: `run_health_check` は private にし、経路は `health_gate(&HealthGateRequest)` だけを呼ぶ。同梱の検査器は `HealthGateRequest.bundled` の引数で渡し、経路の中で process の `bundled_checker()` を読まない（entry）。G2 の材料と review 方針は `with_applicable_checker` を通る（g2-sites）。
6. D3.2 の `repair_hint` の bucket: `RepairClass` は `Copy` で `bucket()` が `&'static str` なので、`RepairClass::Health`（bucket `health`、予算 30 turn・1800 秒）を 1 つ足し、rule ごとの `health:<rule_id>` は `check_item::repair_count` と `ReviewRepairHint.class` が作る（integration-path・review-path・flow）。
7. D1.2 の表の review の評価の場所: 表は `review/required.rs` の `evaluate` と書くが、評価は `health_review/review_item.rs` の `evaluate_all`（review の非同期 task の中の `HealthInputJob::attach`）が行い、`required.rs` は項目を列に足して指紋を作るだけ。`review`・`dispatcher` から `health_review` への辺を作ると deps.cycle の循環が増えるため（review-path・reuse-sync）。同じ理由で `check_item.rs` は `dispatcher::phase_integration` の子 module として `#[path]` で読む（integration-path）。
8. delivery の `base_sha` の照合は `required_checks::verify_health` でなく `delivery.rs` の `health_evaluated_target` に置いた（delivery-path の範囲が `crates/celeris/src/delivery*` だけだったため）。
9. 試験の土台と dagbench の driver は、合格の stub の同梱の検査器（`health_review::stub_checker`）を process に設定する。base に検査器の無い一時 repo が `Unavailable(checker_missing)` で設計 review・承認待ちに回るのを避けるため。本番の規則は変えていない（g2-sites）。

## limits への引き継ぎ
- 往復の集約（D4.3）: 未実装。今は修復のたびに `check_item::repair_count` が rule ごとの bucket の数を数えるだけで、同じ fingerprint の集合の往復を検出して木の root の受信箱に 1 件にまとめる処理は無い。
- 上限（D4.1・D4.4）: health の修復は `RepairClass::Health`（予算 30 turn・1800 秒）と `max_repairs_per_class` の bucket `health:<rule_id>` で数える。使い切りの扱いが D4.4 のとおりかを確かめる試験（`health_limits_repair_cap_blocks`）は limits が置く。
- D3.4 の review の経路（上の 4）: review の Error を Blocked にし受信箱に `health_check_error` を 1 件出すには、task-core の Trigger と `review_verdict` の分岐が要る。D3 を持つ limits で扱う。
- 費用・時間（D1.5・D4.2）: 記録済みの G2 の材料を使う review（reviewer の再試行など）は、束縛の同じ Pass が無ければ review の health の項目が検査器を 1 回起こす（Celeris の repo で 1 回 約 22〜95 秒。review-path の計測）。`edit_verify_secs` の増分は limits が計測する。
- `dup.contract` の名前: task-core の `required_checks::health::UnavailableReason` が `route_select::availability::UnavailableReason` と同名で、check-health の new の warning が 1 件出続ける（合否には使わない）。名前を分けるかは limits か closeout が決める。
- D3.3 の試験: ADR が limits に割り当てた `health_limits_ungrounded_llm_finding_does_not_block` と同じ不変条件は `health_enforce_llm_finding_*` が確かめる。limits は同じ試験を二重に書かない。

## 本番への移行
- config・DB の表・migration の変更は無い。event の欄も増やしていない（evidence が足した任意欄 `health` に値を入れるようになっただけ）。
- 定義指紋に検査器が入るので、health の当たる候補では、この変更を含む release を昇格した時点の手持ちの必須検査の証跡が一致せず、1 回だけ評価し直す（evidence の「本番への移行」と同じ扱い）。人の操作は要らない。
- evidence と loop-points は同じ release に入れる（ADR の「本番への移行」）。loop-points の前の evidence だけでは、review が health の項目を記録しないので配送が再 review に戻る。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
（提出時の検査の後に書く）

## 未解決
1. review の Error で task が Blocked にならず Failed になる（上の「ADR との違い」4。limits へ引き継いだ）。
2. 人が承認した policy の変更（PolicyOnly）の候補は delivery の照合で止まり、再 review に戻る。再 review の後も同じ理由で止まると考えられるが、その後の流れ（`ReviewResyncHalted` で受信箱に出るか）は試験で確かめていない（未確認。delivery-path）。
3. 配送の局所修復の後に review の経路に戻って評価し直すことを通しで確かめる試験は無い（段の統合と review の修復の後は `health_enforce_integration_repair_*` と `health_enforce_non_celeris_*` が確かめる）。
4. WU の check の head は WU の HEAD の commit なので、未 commit の変更は WU の check で評価されず、段の統合で評価される。段の統合の health は先頭の git の repo だけを評価する（cmd の検査と同じ。integration-path）。
5. `health_note`（NotApplicable の理由など）は `RequiredRepoResult` と tracing の info に残るが、event には書かない（review-path）。

## 提案
1. `check_item::repair_count` の doc comment は「段の統合の修復の bucket」のままである。task の review の修復も同じ関数を使うので、「段の統合と task の review の修復の bucket」に直す（flow の提案。この unit はコードを変えないので closeout か limits の変更で行う）。
2. delivery の `base_sha` の照合の正本を `task_core::required_checks::verify_health` に移し、`delivery.rs` の `health_evaluated_target`（束縛の最新の証跡を引く処理が `required_checks::latest_bound` と重なる）を消す（delivery-path の提案 1）。
3. PolicyOnly を承認つきで通すには、`policy_review_gate` が承認の verdict と同じ transaction で、承認の子の id と `(repo, target_sha, reviewed_sha)` を持つ構造化した記録を events に残し、`verify_health` がそれを照合する（delivery-path の提案 2。範囲は task-core の events・task-dispatch の policy_review・required_checks）。
4. 同梱の検査器を process の global（`OnceLock`）でなく値として dispatcher に渡す（`DispatchConfig` に持たせる）。試験の土台と dagbench の driver は stub の値を config に入れるだけになり、同じ process で stub あり・なしの試験を混ぜられる（g2-sites の提案）。
5. `repo_facts` に渡す git の closure（`celeris::delivery::git_text` と `gate` の `git_stdout`）は git を `LC_ALL=C` で起こす。翻訳された「Git 管理でない」の文は `NOT_A_GIT_REPO_STDERR` に合わず `facts_unreadable` になる（合格にはならないが、Git 管理でない作業場所でも health が error になる）。今はどちらも `LC_ALL` を設定していない（facts の提案。`grep -n LC_ALL` で 0 件を確かめた）。
