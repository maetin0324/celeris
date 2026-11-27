---
tasks: [01M7XQA79910VEZ66HWTX4HZVT]
wu: safety-tests
status: done
updated: 2026-11-27
completed: 2026-11-27
---
# 戦略層 v1 子 9 safety-tests: 子 1〜7 の安全の反例・停止の試験をこの WU の HEAD で再実行する

## 結論

- 計画の check の 3 本の nextest を、この WU の HEAD で foreground で流した。3 本とも exit 0、落ちた試験は 0 件。
  - 1 本目（子 1・子 2・no-progress）: `63 tests run: 63 passed, 5568 skipped`
  - 2 本目（子 3・子 4）: `115 tests run: 115 passed, 6099 skipped`
  - 3 本目（子 2・子 3 の gate、子 5・子 6・子 7）: `206 tests run: 206 passed, 6477 skipped`
- 3 本の合計は 384 件。同じ試験が 2 本に入るものが 2 件ある（下の「重なり」）。
- 変更前に落ちる負の試験は、この WU では再実行していない。下の表は各子の進捗の節から写した子の記録である。
- userns の試験 9（celeris-credentiald の `injection_peer_uid` の本体）は流していない。`CELERIS_USERNS_TESTS` が要り、この run の sandbox は `NoNewPrivs: 1` で sudo を使えない。
  結果は子 1 の記録（operator が host で流した）を写した。
- この WU は `agent-docs/progress/2026-11-27-strategy-v1-9/safety-tests.md` だけを足した。crates/・web/・scripts/・docs/ は変えていない。

## 実行の条件

- tree: この WU の HEAD。`git merge-base HEAD main` は HEAD と同じ commit（子 8 の統合の merge。件名「integrate wu/close-verify (phase close)」を含む）で、
  基準 main 7f69057907ad649829ec5a08b5ac7c892b9fe055 はその祖先（`git merge-base --is-ancestor` が 0）。
- 環境: `CELERIS_USERNS_TESTS`・`CELERIS_ISOLATION_TESTS`・`CELERIS_DB_GUARD_TESTS`・`CELERIS_LAUNCHER_TESTS` はどれも未設定（`env` で確認）。cargo-nextest 0.9.146。
  `CARGO_TARGET_DIR` は Celeris が渡したローカルの scratch。
- 時刻（UTC）: 1 本目 13:35:48Z〜13:38:23Z、2 本目 13:38:31Z〜13:40:59Z、3 本目 〜13:43:10Z（どれも 2026-11-27）。時間の大半は build で、試験の実行は 3〜6 秒。
- 件数の内訳は、同じ filter の `cargo nextest list --message-format json` で `filter-match` が `matches` の試験を数えた（3 本とも run の件数と一致）。

## 3 本の nextest（提出時の HEAD で実行）

| # | command | exit | 結果 |
|---|---|---:|---|
| 1 | `cargo nextest run -p task-core -p task-dispatch -p task-worker -p task-ops -p celeris-credentiald --no-tests=fail --no-fail-fast -E 'test(/rejected_worker_decision\|decision_fail_closed_\|root_review_artifact_\|prompt_ancestor_objective_\|prompt_full_objective_\|planner_prompt_acceptance_\|strategy_contract_\|no_progress/) \| test(=probe_found_after_libtest_prefix) \| test(=probe_read_from_real_child_run)'` | 0 | `Starting 63 tests across 38 binaries (5568 tests skipped)`、`63 tests run: 63 passed, 5568 skipped`。failed 0。build を含め 2 分 35 秒 |
| 2 | `cargo nextest run -p task-core -p task-api -p task-dispatch -p task-worker -p celeris-mcp -p celerisctl --no-tests=fail --no-fail-fast -E 'test(/discussion_mode_\|chat_message_edit_\|chat_message_retract_\|chat_discussion_summary_\|chat_exchange_\|external_exchange_\|mcp_discussion_\|mcp_external_scope_\|rdc_celeris_chat_/)'` | 0 | `Starting 115 tests across 123 binaries (6099 tests skipped)`、`115 tests run: 115 passed, 6099 skipped`。failed 0。2 分 28 秒 |
| 3 | `cargo nextest run -p task-core -p task-api -p task-dispatch -p task-worker -p task-ops -p celeris-mcp --no-tests=fail --no-fail-fast -E 'test(/strategy_review_contract_\|strategy_critic_\|strategy_gate_\|strategy_review_shadow_\|strategy_review_regression_\|parent_checkpoint_\|parent_evaluation_\|project_controller_checkpoint_\|publish_manifest_\|publish_signoff_\|proposal_review_\|project_overview_/)'` | 0 | `Starting 206 tests across 109 binaries (6477 tests skipped)`、`206 tests run: 206 passed, 6477 skipped`。failed 0。2 分 11 秒 |

（表の中の `\|` は markdown の表のための escape で、実際の command の filter は `|` である。）

- skipped は filter に合わない試験と `#[ignore]` の試験の合計である。3 本目の filter に名前が合うのに `#[ignore]` で走らなかった試験は 4 件:
  task-api `parent_checkpoint_live` の `parent_checkpoint_blind_live_checks`・`parent_checkpoint_blind_render`・`parent_checkpoint_blind_seed`・`parent_checkpoint_live_sample_checks`
  （子 6 が `--run-ignored` で流す、盲検の評価の出力・seed を読む試験）。1 本目・2 本目には該当が無い。
- 重なり: `project_controller_checkpoint_no_progress_escalates_while_unmapped`（task-dispatch）は 1 本目の `no_progress` と 3 本目の `project_controller_checkpoint_` の両方に入る。
  `chat_discussion_summary_strategy_gate_edited_is_stale`（task-api lib）は 2 本目の `chat_discussion_summary_` と 3 本目の `strategy_gate_` の両方に入る。

## 子ごと・性質ごとの表

件数は上の list で数えた。試験名に複数の接頭辞が含まれるときは、名前の中で先に現れる接頭辞に数えた。結果はどの行も全件 pass（failed 0）。

| 子 | 性質 | 試験名の接頭辞（crate・binary） | 本 | 件数 | 結果 |
|---|---|---|---:|---:|---|
| 1 | 承認要求の fail-closed | `rejected_worker_decision`（task-core 1・task-dispatch 1。基準 main にもある 11-18 の修正の試験で、子 1 が修正を戻すと落ちることを確かめた）、`decision_fail_closed_`（task-core 1・task-dispatch 14） | 1 | 2 + 15 | pass |
| 1 | root の成果物（root review の ArtifactExists が WU の成果物を見つけ、他の task・案件・workspace の外を拾わない） | `root_review_artifact_`（task-dispatch 5） | 1 | 5 | pass |
| 1 | prompt の全文配送（祖先の objective と受け入れ条件の全文、planner の acceptance） | `prompt_ancestor_objective_`（task-dispatch 3・task-worker 2）、`prompt_full_objective_`（task-dispatch 3・task-worker 1）、`planner_prompt_acceptance_`（task-worker 3） | 1 | 5 + 4 + 3 | pass |
| 1 | userns の試験 9 の親が子の `PEER-UID-PROBE` 行を読む | `probe_found_after_libtest_prefix`・`probe_read_from_real_child_run`（celeris-credentiald `injection_peer_uid`） | 1 | 2 | pass。試験 9 の本体は流していない（下の「userns の試験 9」） |
| 2 | 未承認・stale・別案件・出典欠損の戦略の拒否（純関数） | `strategy_contract_`（task-core 20。`strategy_contract_ablation_four_rules` を含む） | 1 | 20 | pass |
| 2 | 同上（`task.create` の入口。422 で拒否し task を作らない） | `strategy_gate_` のうち子 2 の 8 件（task-api lib `cos::strategy_gate::tests`。`strategy_gate_ablation_four_rules`・`_absent_field_keeps_task_create_unchanged`・`_cos_self_approval_is_422`・`_stale_revision_is_422`・`_other_project_material_is_422`・`_missing_source_is_422`・`_unapproved_discussion_and_proposed_are_422`・`_human_approved_latest_document_creates_the_task`） | 3 | 8 | pass |
| 3 | discuss・propose の API の拒否（discuss-only は task 起票・roadmap 変更・公開を 403、副作用 0） | `discussion_mode_`（`chat_discussion_mode_` 4 件を含む。task-core 5・task-api lib 1・task-api `cos_delivery_suite_c` 15・task-dispatch 4・task-worker 1） | 2 | 26 | pass |
| 3 | 編集・撤回の失効（要約の引用と revision、編集・撤回で根拠を失効させる） | `chat_message_edit_`（task-core 10・task-api lib 1・task-api `chat_message_revise` 4）、`chat_message_retract_`（task-core 1・`chat_message_revise` 1）、`chat_discussion_summary_`（task-core 9・task-api lib 1・task-dispatch 3・task-worker 1） | 2 | 15 + 2 + 14 | pass |
| 4 | 外部 LLM の scope と actor の区別・prompt injection・secret の mask・二重取り込み・入れ子の上限・既定で出ない | `external_exchange_`（task-core 28）、`chat_exchange_`（task-api `chat_exchange` 13）、`mcp_discussion_`（celeris-mcp `mcp_discussion` 8）、`mcp_external_scope_`（celeris-mcp `mcp_integration` 7）、`rdc_celeris_chat_`（celerisctl `rdc_celeris_chat` 2） | 2 | 28 + 13 + 8 + 7 + 2 | pass |
| 5 | 固定計画の先行起票の拒否（実験段階の前に本実装の子を起票させない、Critic、低抽象度 task の回帰） | `strategy_review_contract_`（task-core 13）、`strategy_critic_`（task-core 4・task-dispatch 6・task-worker 3）、`strategy_gate_review_`（task-api lib 9）、`strategy_review_shadow_`（task-api `strategy_review_shadow` 5）、`strategy_review_regression_`（task-api `strategy_review_regression` 2） | 3 | 13 + 13 + 9 + 5 + 2 | pass |
| 6 | 親の checkpoint の shadow（途中の親評価・pivot・人への判断要求） | `parent_checkpoint_`（task-core 29・task-api `parent_checkpoint_shadow` 11・`parent_checkpoint_live` 6・`parent_checkpoint_regression` 4・`parent_checkpoint_pivot_chain` 1）、`parent_evaluation_`（task-dispatch 2・task-worker 8）、`project_controller_checkpoint_`（task-core 4・task-dispatch 11・task-ops 1） | 3 | 51 + 10 + 16 | pass。`#[ignore]` の 4 件は走っていない（上） |
| 6 | no-progress と budget の停止（子 6 の分） | `project_controller_checkpoint_no_progress_escalates_while_unmapped`（task-dispatch） | 1・3 | 1 | pass |
| 既存 | no-progress と budget の停止（基準 main にある既存の guard。子 6 はこの guard に停止を任せた） | `no_progress` を名前に含む既存の 6 件: task-dispatch `continuation_cap::task_without_progress_asks_after_no_progress_limit_with_the_streak_in_the_question`・`tick_and_dispatch::budget_exhausted_run_continues_without_consuming_attempts_then_blocks_on_no_progress`・`execution_scheduler::no_progress_at_the_limit_blocks_even_under_the_continuation_cap`・`execution_scheduler::no_progress_at_the_limit_blocks_with_the_limit_reason`・`run_settlement::task_run::settle_task_run_asks_on_no_progress_before_the_hard_limit`、task-ops `derive::no_progress_streak_counts_consecutive_checkpoints_without_progress` | 1 | 6 | pass |
| 1 | no-progress の leaf の決定の要求（fail-closed の 1 件） | `decision_fail_closed_no_progress_leaf_records_decisions`（task-dispatch。上の `decision_fail_closed_` 15 件に数えた） | 1 | （15 の内） | pass |
| 7 | publish の署名は人だけ（operator・CoS・MCP・外部 LLM・worker・未認証の署名と採用を 403 で拒否） | `publish_signoff_`（task-core 2・task-api `publish_manifest` 14・celeris-mcp `mcp_integration` 1）、`proposal_review_`（task-core 3・task-api lib 1・task-api `proposal_review` 19） | 3 | 17 + 23 | pass |
| 7 | publish の manifest と概要（claims→evidence→再現条件→限界→signoff、確認済み・推定・要判断の区別、drill-down） | `publish_manifest_`（task-core 13・task-api `publish_manifest` 2）、`project_overview_`（task-api `project_overview` 23） | 3 | 15 + 23 | pass |

本ごとの合計: 1 本目 = 子 1 の 36 + 子 2 の 20 + no-progress の 7（子 6 の 1・既存の 6）= 63。2 本目 = 子 3 の 57 + 子 4 の 58 = 115。
3 本目 = 子 2 の 8 + 子 3 の 1（`chat_discussion_summary_strategy_gate_edited_is_stale`）+ 子 5 の 42 + 子 6 の 77 + 子 7 の 78 = 206。
子ごとの件数は各子の進捗の再実行の件数と合う（子 3 は 57、子 4 は 58、子 5 は 42、子 7 は 78）。

## userns の試験 9（流していない。子 1 の記録）

- この WU は試験 9（`real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch`）を流していない。本体は `CELERIS_USERNS_TESTS=1` と、
  別 uid の子を起こす sudo が要る。この run の sandbox は `NoNewPrivs: 1` で、4 つの env も未設定である。1 本目の filter は同じ binary の probe の 2 件だけを選ぶ。
- 子 1 の記録（[2026-11-21-strategy-v1-1.md](../2026-11-21-strategy-v1-1.md) の「replan の後」と S9。operator〈claude-code-ops-session〉が release gate の host の login shell で流した）:
  - command: `CELERIS_USERNS_TESTS=1 CELERIS_LAUNCHER_TESTS=require TMPDIR=/tmp cargo nextest run -p celeris-credentiald --test injection_peer_uid --no-capture --status-level all -E 'test(=real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch)'`
  - 修正後の tree: exit 0、`1 test run: 1 passed, 5 skipped`、`PEER-UID` の control・dac・broker・result の 4 行、`SKIP: (not passed)` の行 0 件。
  - 修正前の tree: 同じ command で exit 100（no PEER-UID-PROBE line）。
- 子 1 の未解決 U2（試験本体の中の 2 段目の skip。release gate は `CELERIS_LAUNCHER_TESTS=require` を立てない）は、子 1 の記録のとおり別 task 01M7YWAC1BVNG09WKDQJ6CA86Z で扱う。
  この WU では状態を確かめていない（未確認）。

## 変更前に落ちる負の試験（子の記録。この WU では再実行していない）

この WU は実装前の source に戻す操作をしていない。下の表は各子の進捗の節（括弧内）から、子・WU・command（操作）・記録された exit と件数を写した。
exit が進捗に書かれていないものは「exit の記録なし」と書いた。

| 子 | WU（出典の節） | command・操作 | 記録された結果 |
|---|---|---|---|
| 1 | decision-failclosed（子 1 §6、decision-failclosed.md） | 11-18 の修正の owner の 2 hunk（`decision.rs`・`tree_units.rs`）を戻して `rejected_worker_decision*` | exit=100（`2 tests run: 0 passed, 2 failed`）。戻すと exit=0 |
| 1 | decision-failclosed（同上） | 4 つの before.log（fix・settle-leaf・settle-failure・settle-atomic）で `decision_fail_closed_*` 14 件 | 4 つとも exit=100、assertion で落ちた（3・4・4・3 件 failed）。after.log は全部 exit=0 |
| 1 | root-artifacts（子 1 §6、root-artifacts.md） | 修正前 A（候補を review する run だけにした tree）で `root_review_artifact_*` | exit=100、`5 tests run: 2 passed, 3 failed`（`not found at artifacts/final-report.md` など）。修正前 B（workspace の外の除外を外した tree）で `root_review_artifact_ignores_paths_outside_workspace` が fail（exit=100、`5 tests run: 4 passed, 1 failed`） |
| 1 | prompt-fix（子 1 §6、prompt-fix.md） | 修正前の production の tree に試験 file だけを足して dispatcher の `prompt_ancestor_objective_*` 2 件 | 4 経路とも fail（`parent objective tail x0 (want x1)` など）。exit の記録なし。task-worker の 2 件と設計 review の 1 件は修正前の tree で compile できない |
| 1 | userns-skip（子 1 §6、userns-skip.md） | 変更前の build で env 無しに対象の 9 件 | exit 0、`9 tests run: 9 passed, 1225 skipped`（本体を走らずに pass と数えた。負の結果） |
| 1 | peer-uid-probe（子 1 §6、peer-uid-probe.md） | 読み取りを旧い `stdout.lines().find_map(Probe::parse)` に戻して probe の 2 件 | exit 100、`2 tests run: 0 passed, 2 failed` |
| 1 | 試験 9 の host の実行（子 1 S9） | 上の「userns の試験 9」の command を修正前の tree で | exit 100（no PEER-UID-PROBE line）。operator の記録 |
| 2 | strategy-gate（子 2 §5） | `operations.rs` を WU の base に戻して `cargo test -p task-api --lib strategy_gate` | exit 101、0 passed・8 failed（400 `unknown field strategy`） |
| 2 | strategy-gate（同上） | `create_operation` の `req.check_strategy(…)?;` の 1 行だけを消して同じ command | exit 101、1 passed・7 failed（負の 7 件が 200 で task を作った） |
| 2 | request-prompt（同上） | base の worktree に試験 file だけを写して `cargo test -p task-dispatch prompt_project_request` | exit 101、1 passed・2 failed |
| 2 | request-prompt（同上） | 同じ tree で `cargo test -p task-worker prompt_project_request` | exit 101、compile できない（E0432 1 件・E0560 3 件） |
| 2 | contract-core（同上） | base に `strategy` の module が無い | compile できない。exit の記録なし |
| 3 | mode-gate（子 3「変更前に落ちた負の試験」） | 同じ試験 file を base の src に当てて `cargo test -p task-api --test cos_delivery_suite_c discussion_mode` | exit=101、`1 passed; 10 failed`（400 `unknown field discussion_mode`、base は discuss でも task を作った） |
| 3 | edit-retract（同上） | route を base の表に戻して `cargo test -p task-api --test chat_message_revise` | `1 passed; 4 failed`（405・404）。exit の記録なし |
| 3 | edit-retract（同上） | `chat_revision.rs` から失効の処理と `message_revised` の追記を外して `cargo test -p task-core --lib chat_revision` | `5 passed; 6 failed`。exit の記録なし |
| 3 | summary-cite（同上） | 本番のコード 4 file を実装前に戻して `cargo test -p task-api --lib -- chat_discussion_summary_strategy_gate_edited_is_stale` | `left: 200 right: 422`（編集された発言を根拠にした戦略で task ができた）。exit の記録なし |
| 3 | cite-attach（同上） | `workspace_files.rs` を実装前に戻して `cargo test -p task-dispatch --lib -- discussion_files` | 7 件とも FAILED。exit の記録なし。全文の log は repo に無く、子 3 の close も読んでいない（未確認） |
| 3 | web-mode・web-cite（同上） | base の source に web の試験を当てた・変異 | web-mode は 8 failed と 1 file の読み込みの失敗、web-cite は `Tests 7 failed \| 21 passed (28)`。exit の記録なし |
| 4 | envelope-fix（子 4「変更前に落ちた負の試験」） | base の `envelope.rs` と base の API で書いた仮の試験 2 件 | exit 100、`2 tests run: 0 passed, 2 failed`。最終版の試験は base で compile できない（exit 101、36 errors） |
| 4 | exchange-core（同上） | `chat_events_page` の type の除外だけを外して `external_exchange_share_records_not_in_event_page` | exit 100（`unknown variant` で SSE の replay が落ちる） |
| 4 | rest-import（同上） | `.merge(exchange::routes())` を外して `chat_exchange_` | `12 tests run: 0 passed, 12 failed`（404・405）。exit の記録なし |
| 4 | mcp-tools（同上） | registry の 3 行を外して `cargo nextest run -p celeris-mcp -E 'test(/mcp_discussion_/)' --no-fail-fast` | exit 100（道具が無く `-32601`） |
| 4 | scope-guard（同上） | 文書と `decisions.rs` 等を base に戻して `mcp_external_scope_` | `7 tests run: 2 passed, 5 failed`（base では `chatgpt-rdc-2` の scope で決定に答えられた）。exit の記録なし |
| 4 | rdc-path（同上） | 推奨 scope を前の一覧に戻す・`celeris-chat` を `set -x` にする | 前者は `2 tests run: 0 passed, 2 failed`、後者は `rdc_celeris_chat_token_not_in_output` が FAIL。exit の記録なし |
| 4 | depth-guard（同上） | `format.rs` を base のまま `cargo nextest run -p task-core --lib -E 'test(/external_exchange_format_depth_/)' --no-fail-fast` | exit 100、`2 tests run: 0 passed, 2 failed`（stack overflow で SIGABRT） |
| 4 | depth-guard（同上） | 同じ状態で `cargo nextest run -p task-api --test chat_exchange -E 'test(chat_exchange_import_deep_nesting_rejected)' --no-fail-fast` | exit 100、`1 test run: 0 passed, 1 failed`（試験の process 全体が SIGABRT） |
| 5 | contract-review（子 5「変更前に落ちた負の試験」） | 型と空の `check_execution` だけの HEAD で `cargo nextest run -p task-core --no-fail-fast -E 'test(/strategy_review_contract_\|strategy_contract_/)'` | exit 100、`33 tests run: 21 passed, 12 failed` |
| 5 | critic-review（同上） | 試験を先に置いた HEAD で `cargo nextest run -p task-core -p task-worker -p task-dispatch -E 'test(/strategy_critic_/)'` | exit 101（3 crate とも compile で落ちる） |
| 5 | experiment-gate（同上） | 試験だけを先に commit し base の gate で `strategy_gate_` | exit 100、`19 tests run: 1 passed, 18 failed` |
| 5 | experiment-gate（同上） | `StrategyRef.candidate` だけを足した gate（規則なし） | exit 100、`19 tests run: 10 passed, 9 failed` |
| 5 | shadow-case（同上） | `check_execution` の呼び出しを外して `cargo nextest run -p task-api --test strategy_review_shadow --test strategy_review_regression --no-fail-fast` | exit 100、`7 tests run: 2 passed, 5 failed`（段 2 の子 A〜H の compound が 200 で通った） |
| 6 | checkpoint-core（子 6「変更前に落ちた試験」） | 型と署名だけ、本体は `todo!()` | `parent_checkpoint_contract_` 25 件すべて fail。exit の記録なし |
| 6 | evaluator-wire（同上） | 型・欄・節が base のまま | `parent_evaluation_wire_` が compile error 20 件、exit 101 |
| 6 | strategy-read（同上） | `strategy_task_creates` が空を返す stub | `strategy_reads_` 3 件 fail、exit 100 |
| 6 | controller-checkpoint（同上） | 試験だけ先に置いた | compile error 15 件、exit 101 |
| 6 | checkpoint-act-fix・controller-launch（同上） | 試験だけ足した状態・起動と決着の段が無い | `project_controller_checkpoint_` 5 件中 2 件 fail、task-dispatch の 11 件中 9 件 fail。exit の記録なし |
| 6 | second-gate-shadow（同上） | 効果を外した変異 M1・M2 | M1 で 11 件中 10 件、M2 で 11 件中 5 件 fail。exit の記録なし |
| 6 | ts-checkpoint-rule・live-evaluator-sample・案 A の WU（同上） | 規則の追加前・評価の JSON が無い・base の fixture | vitest の新しい 9 件 fail、`parent_checkpoint_live_sample_checks` fail、blind-eval-input の 3 件 fail など。exit の記録なし |
| 7 | proposal-api（子 7「変更前に落ちる負の試験」） | production の file を base に戻して `proposal_review_` | `19 tests run: 0 passed, 19 failed`。exit の記録なし |
| 7 | publish-api（同上） | `signoff_actor` の 2 行と `check_before_sign` の 2 か所を外す | `16 tests run: 7 passed, 9 failed`、MCP は `1 test run: 0 passed, 1 failed`。exit の記録なし |
| 7 | drilldown-api（同上） | `project_overview.rs` を変える前に `cargo nextest run -p task-api --test project_overview -E 'test(/project_overview_drilldown_/)' --no-fail-fast` | exit 100、`4 tests run: 0 passed, 4 failed` |
| 7 | drilldown-e2e（同上） | 手書きの fixture の link を戻して `vitest run e2e/support/fake-daemon.test.ts -t project_overview_fixture_links` | exit 1 |
| 7 | close-audit（子 7「close-audit が guard を外して再現した結果」） | `plan_answer`・`plan_revise` の `signoff_actor(&row, by)?;` 2 行と `reviewer` の人だけの照合を外して `cargo nextest run -p task-api -p celeris-mcp -E 'test(/publish_signoff_\|proposal_review_/)' --no-fail-fast` | exit 100、`35 tests run: 24 passed, 11 failed`（operator・CoS・MCP・外部 LLM の署名が通った）。戻すと exit 0・35 passed |

## この WU の 3 本に入っていない試験（この WU では流していない。未確認）

子の進捗が再実行した試験のうち、次は計画の 3 本の filter に入らないので、この WU では結果を確かめていない。
- 子 1: e2e の `scenarios` 4 件・`delegation_scenarios` 5 件、`userns-skip/check-skip.sh`（9 件が env 無しで SKIP）、`replay.py verify`、試験 9 の本体（上）。
- 子 2: `prompt_project_request`（task-dispatch 3・task-worker 3。Project.request の prompt への配送）。
- 子 3: web の `vitest run features/chat`。
- 子 6: `reviewer_side_run_`・`strategy_reads_`、`#[ignore]` の 4 件（上）、TS の `project_controller_checkpoint_` の vitest。
- 子 7: `cos_ops_registry_`、web の vitest と playwright の e2e（14 件）。

## 検査（提出時の HEAD で実行）

- 上の 3 本の nextest → 3 本とも exit 0（63・115・206 件 pass、failed 0）。
- 計画の check 4: `test -s agent-docs/progress/2026-11-27-strategy-v1-9/safety-tests.md && grep -q 'tests run' agent-docs/progress/2026-11-27-strategy-v1-9/safety-tests.md && git diff --quiet $(git merge-base HEAD main) -- crates/ web/ scripts/ docs/ Cargo.toml Cargo.lock` → exit 0（crates・web・scripts・docs・Cargo の差分 0）。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 変更 path はこの file 1 つだけ、範囲外 0 件。
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。`sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）。
- test-affected・clippy・fmt はこの WU では流していない。この WU は文書 1 file だけを足し、Rust と web を変えていない（check 4 の差分 0）。

## 未解決

- 変更前に落ちる負の試験は子の記録だけで、この WU では再現していない。exit が進捗に書かれていない行がある（上の表）。
- 試験 9 の本体と、子 1 の U2（2 段目の skip）は、この WU では確かめていない。
- 上の「3 本に入っていない試験」は、この WU の HEAD での結果が未確認である。
