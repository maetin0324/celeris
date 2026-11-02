---
title: S0 棚卸し — TS 制御層の候補ロジック（TS 移行／Rust 維持／判定のみ切出し）
tasks: [01M6DCPKPJZZ9F9J4QJ47FH8KK]
status: done
updated: 2026-11-02
---
# S0 棚卸し — TS 制御層の候補ロジック

完了日: 2026-11-02

親 task 01M6DCMB75G5F0DSE58ZNZMBAK の S0 (1)。コードは変えていない（この WorkUnit の差分はこのファイルだけ）。

## 結論
- 方針の表（lane 規則表・gate の得点表・route の順位付け）と、CoS の決定的な文面組み立て（digest・定期見直しの入力）は TS へ出せる。
  どれも I/O を持たない関数か、I/O と判定の境目が 1 か所で切れる。
- CoS の定型代答の検査（`basis::validate`・plan_conforms・job_wait_extend・review_rework）、CoS operations の認可と監査、
  WorkUnit の状態機械、worker の起動・lease、ready の並び順は Rust に残す。親 task の制約（安全検査を TS へ移して削除・緩和しない、
  scheduler・認可・永続状態は Rust）に当たるため。
- S4 の第一候補は「受信箱一次対応の digest」（`cos_chat/digest.rs:digest_message`）。理由は §4。

## 1. 調べた時点と前提
- 対象: この WorkUnit の base（`$CELERIS_WU_BASE`）。base は main の先端（件名「phase 1: question で終わった run の usage を runs と
  worker_finished に記録する」、2026-11-01）と同じ tree。`git rev-list --count $CELERIS_WU_BASE..main` は 0。
- 変更頻度の期間: 2026-08-04〜2026-11-02（90 日）。repo の最初の commit は 2026-09-13 なので、実質は約 50 日分。
  30 日（2026-10-03 以降）の commit 数も併記した。
- `--follow` を使っていない。分割・改名で生まれた file（例: dispatcher.rs から切り出した dispatcher/*.rs）は、分割前の履歴を数えない。
- 逆依存 crate 数は通常依存（`-e normal`）の推移的な数で、その crate 自身を除く。dev 依存を含めると task-core 12、task-api 6。
- toolchain: cargo 1.98.1（2026-08-05）。

## 2. 分類表

列「変更頻度」は `90 日の commit 数 / 追加行-削除行 / 30 日の commit 数`。列「試験」は `#[test]`・`#[tokio::test]` の数と置き場所。
path は `crates/` を省いた。`D` = `task-dispatch/src/dispatcher`、`C` = `task-api/src/cos`、`RS` = `task-core/src/route_select`。

### 2.1 TS 移行（判定全体を TS に置き、Rust は入力の組み立て・出力の検証・永続化だけを持つ）

| 対象 | file:関数 | 変更頻度 | 逆依存 crate | 試験 | 結合する状態/安全条件 | 分類 | 根拠 |
|---|---|---|---|---|---|---|---|
| lane 規則表 | `task-core/src/model_policy.rs`: `RULES`・`ModelPolicy::decide`・`TaskFeatures::infer`/`infer_with_hints` | 20 / +1758-493 / 11 | 11（task-core） | 11（`model_policy/tests.rs`）。`model_policy::` を参照する file は 35 | 入力は `Task` だけで I/O なし。`LaneCeiling::clamp`（天井＝予算側の上限）と `TierSource::Human` の人の明示は安全側の条件 | TS 移行（`RULES` と `infer`）。`clamp` と人の明示の優先は Rust に残し、TS の結果に Rust が後から当てる | 規則表は `when: fn(&TaskFeatures)->bool` の 5 行で、方針変更の頻度が高い（30 日で 11）。task-core を触ると 11 crate が再ビルドになる |
| route の候補順 | `task-core/src/llm_route.rs`: `route_priority` | 12 / +1193-390 / 12 | 11 | 10（`llm_route/tests.rs`） | config の `[routing] route_defaults`（読み込みは Rust）。候補が registry に在るかは Rust の `route_select` が確かめる | TS 移行（順序付けだけ） | 入力は config の列と execution class だけの純粋な並べ替え。出力は候補の順序で、許可の判定を含まない |
| 受信箱一次対応の digest | `D/cos_chat/digest.rs`: `digest_message`・`kind_label`・`judgment`・`options_line` | 6 / +256-26 / 6 | 4（task-dispatch） | 11（`D/tests/cos_chat_triage_inbox_thread.rs`。`digest.rs` 内は 0） | 入力は `cos_inbox_items` の行（`CosTriageItemReport`）と run の状態。止まった run の `running` 項目を `pending` に戻す処理（`Dispatcher::triage_digest`）は Rust に残す | TS 移行（`digest_message` 全体） | 既に `(run_id, run_state, reason, items) -> (String, Vec<ChatCard>)` の純粋関数。出力は assistant message 1 件で、状態遷移を持たない |
| 定期見直しの入力文 | `D/cos_chat/review.rs`: `review_text`（`push_list`・`wait_line`・`cut`） | 3 / +323-27 / 3 | 4 | 7（`D/tests/cron_cos_review.rs`、`review_text` を直接呼ぶ試験あり） | 冪等 key（`review_key`・`chat_message_by_client_id`）、back thread への 1 件投入（`cos_review_input_post`）は Rust | TS 移行（文面組み立て）。ただし `review_text` は中で `store.project_brief_list()` を読むので、読み出しを呼び出し側へ出す前段の Rust 変更が要る | 入力の `ProjectBrief`・`StagnationFinding` は既に Serialize。LLM も状態遷移も持たない |

### 2.2 判定のみ切出し（純粋な判定部分だけを TS に出し、I/O・transaction・安全条件は Rust）

| 対象 | file:関数 | 変更頻度 | 逆依存 crate | 試験 | 結合する状態/安全条件 | 分類 | 根拠 |
|---|---|---|---|---|---|---|---|
| execution gate の得点 | `task-core/src/execution_gate.rs`: `decide_at`（`Scorer`・`PROCESS_WORD_GROUPS`・`count_process_stages`） | 14 / +1665-578 / 7 | 11 | 12（`execution_gate/tests.rs`） | `out_of_scope_rule`（対話・support・Shared workspace・cron 知識整理）、`human_execution`（人の明示）の優先、`GateThreshold::at_depth`（ADR-0079 の深さ上限） | 判定のみ切出し: 得点と信号の表を TS、対象外規則・人の明示・深さの閾値は Rust が先に当てる | 得点表は方針で頻繁に動く（30 日で 7）。対象外規則と深さは木の安全条件なので TS に渡さない |
| source/model の順位付け | `RS/policy.rs`: `select`、`RS/lane.rs`: `decide_lane`・`clamp_to_limits`、`RS/balance.rs`: `order_harnesses`、`RS/assist.rs`・`stage.rs`・`task_override.rs`・`trace.rs` | 16 / +2277-54 / 16 | 11 | 46（各 file と `assist_tests.rs`・`mlsys_policy_tests.rs`） | lane の上限（`LaneLimits`）、人・task の override（`resolve_routing`）、trace は event に残る | 判定のみ切出し: 順位付けと lane 選択を TS、`clamp_to_limits` と override の解決は Rust | `RS/mod.rs` が「Pure source/model selection rules」と明記。変更頻度が最も高い群（30 日で 16） |
| dispatcher の route 選択 | `D/route_select.rs`: `route_select`・`route_select_one`・`order_by_headroom`。I/O 側は `Dispatcher::select_route_with`・`record_route_selection` | 16 / +1170-89 / 16 | 4 | 26（`route_select.rs`・`D/tests/route_select.rs`） | accounts・quota book（残量）・`route_selected` event。CoS route は「該当なしなら fail-closed」（試験 `cos_route_candidates_keep_priority_and_fail_closed_when_none_is_eligible`） | 判定のみ切出し: `route_select`（コメントに「no I/O, no clock」）を TS、`select_route_with`・`record_route_selection` は Rust | 純粋関数と I/O 関数が既に別。fail-closed の結果は Rust 側で再確認できる |
| 受信箱取り込みの分類 | `D/cos_chat/triage.rs`: `is_noise`・`touched_tasks`・`sources_for_touched`・`inbox_source`・`is_direct`・`direct_body`。store 側 `task-core/src/chat/triage.rs`: `cos_triage_ingest_batch`・`cos_triage_reconcile` | dispatcher 16 / +879-163 / 16、store 16 / +1448-120 / 16 | 4（dispatcher）、11（store） | 25（`D/tests/cos_chat_triage*.rs`）＋10（`chat/triage_tests.rs`） | `feed_cursor`（取り込み位置）、`cos_inbox_items` の挿入 transaction、`policy_version`、route の版（`cos_triage_route_version`） | 判定のみ切出し: noise 判定・source_kind の分類・対象 task の抽出を TS、cursor と transaction は Rust | dispatcher は「人が要るかを決めない」（ADR 2026-10-05-cos-chat-home D3）。分類規則は頻繁に変わるが、取り込みの冪等性は store の責務 |
| CoS 不在時の退避 | `D/cos_chat/fallback.rs`: `fallback_body`・`notice_text`・`clip_with_link`（文面）、`unavailable_reason`・`candidates`（判定と読み出し） | 8 / +726-62 / 8 | 4 | 27（`D/tests/cos_chat_triage_fallback*.rs`・`fallback.rs`） | 人への通知の安全網（認可は `authorization_unavailable_after_secs`）、notice store、`handoff_key` の冪等性 | 判定のみ切出し: 文面だけ TS。`unavailable_reason` と期限は Rust に残す | `unavailable_reason` は「人に届かない」を防ぐ安全条件。文面は安全条件を含まない |
| 中継指示の再判定 | `D/cos_chat/relay_rejudge.rs`: `covers`・`relay_rejudge`・`plan_rejudge`、`task-core/src/chat/plan_rejudge.rs` | 2 / +132-0 / 2、store 側 3 / +435-0 / 3 | 4、11 | 4（`chat/relay_rejudge_tests.rs`・`plan_rejudge_tests.rs`） | `cos_inbox_items.message_id`・`chat_messages.seq` の組（store が選ぶ） | 判定のみ切出し（`covers`） | 対の選択は store、`covers` は task 集合の包含判定だけ。変更頻度が低いので優先度は低い |
| root task 完了の要約 | `D/cos_chat/root_done.rs`: `root_done_tick`・`progress_markdown`・`truncate`、`task-ops/src/root_done.rs`: `is_root_done` | 3 / +469-26 / 3、ops 側 1 / +423-0 / 1 | 4、7（task-ops） | 5（`D/tests/cos_chat_root_done.rs`・`root_done.rs`）、ops 側 `root_done/tests.rs` | `feed_cursor` `root_done_summary`、`root_done_summaries` の 1 行、Discord 送信 | 判定のみ切出し: 要約文面（`progress_markdown`）を TS。検知と記録は Rust | 検知は events の cursor 読みで、送信は外部 I/O |
| replan の理由文 | `D/planner_flow.rs`: `replan_trigger_reason`・`plan_invalid_replan_note` | 10 / +1261-26 / 4 | 4 | 12（`D/tests/tree_replan.rs`・`stalled_replan.rs`・`replan_rebase.rs`） | 計画の版、replan の上限と stall 検出（下の `replan_gate`） | 判定のみ切出し（理由文の組み立て） | 文面は planner への入力で、上限の判定を含まない |

### 2.3 Rust 維持

| 対象 | file:関数 | 変更頻度 | 逆依存 crate | 試験 | 結合する状態/安全条件 | 分類 | 根拠 |
|---|---|---|---|---|---|---|---|
| 直行経路の判定 | `task-core/src/direct_route.rs`: `evaluate` | 6 / +441-2 / 5 | 11 | 11（`direct_route/tests.rs`）＋8（`D/tests/direct_route.rs`） | `pending_approval`（承認待ち）、`cross_department`（認可）、Check の種類（human check）、gate の `out_of_scope_rule` | Rust 維持 | 全条件の AND で、条件の多くが承認・認可。変更頻度も低い（90 日で 6） |
| CoS 定型代答の共通検査 | `C/basis.rs`: `validate`・`missing`・`reason_prefix`、`BasisCategory`（7 種） | 5 / +501-10 / 5 | 2（task-api） | 0（`basis.rs` 内）＋5（`task-api/tests/cos_routine_answers.rs`） | refs の検証、`policy_version` の版違え 422、人への回付 | Rust 維持 | 親 task の制約「basis 検査を TS へ移して削除・緩和しない」 |
| plan gate の自動承認 | `C/basis/plan_conforms.rs`: `validate`・`needs_human`・`gate_needs_human`・`check_guards`・`recent_versions` | 2 / +400-4 / 2 | 2 | 11（`task-api/tests/cos_plan_conforms.rs`・`cos_plan_conforms_guards.rs`）＋1 | human check 403、near_limit 等 422、承認後の objective/acceptance の Edited 422（版整合）、CoS 承認回数と版の上限 | Rust 維持 | 同上（human_required・版整合の拒否） |
| job 待ちの延長 | `C/basis/job_wait_extend.rs`: `validate`・`timed_out_wait`・`timeouts_of_same_jobs` | 1 / +326-0 / 1 | 2 | 3（`task-api/tests/cos_job_wait_extend.rs`）＋3 | cluster job の状態、同じ job の延長回数 | Rust 維持 | 同上 |
| review 差し戻しの自動再開 | `C/basis/review_rework.rs`: `validate`・`rejection`・`reviewer_verdict` | 2 / +230-0 / 2 | 2 | 4（`task-api/tests/cos_review_rework.rs`）＋2 | 上限 2 回、同文の繰り返し・human check の拒否 | Rust 維持 | 同上 |
| CoS operations | `C/operations.rs`（registry: ALLOWED/EXCLUDED/PENDING）、`C/ops/{admin,decisions,projects,surface,tasks}.rs` | 70 / +4570-970 / 70 | 2 | 58（`task-api/tests/cos_op*.rs`・`C/ops/tests.rs`） | 認可（actor=cos、human 詐称の拒否）、domain 書き込みと監査の同一 transaction、EXCLUDED の 422 | Rust 維持 | 認可と原子的な監査。変更頻度は最も高いが、多くは endpoint の追加で方針の表ではない |
| CoS 受信箱 API | `C/inbox.rs` | 14 / +775-56 / 14 | 2 | 27（`task-api/tests/cos_triage.rs`・`cos_triage_override.rs`・`cos_chat_triage_scenarios.rs`） | `cos_inbox_items` の状態遷移（resolve・escalate） | Rust 維持 | 永続状態の遷移 |
| CoS run の起動 | `D/cos_chat/launch.rs`（`launch/`） | 34 / +1732-534 / 34 | 4 | 25（`D/tests/cos_chat_launch.rs`・`cos_chat/harness_tests.rs`） | session・account・worker process・sandbox | Rust 維持 | worker orchestration と process 起動。TS 層から process を起こさない（親 task の制約） |
| replan の上限 | `D/work_units.rs`: `replan_gate`、`task-core/src/execution_plan/plan_limits.rs` | 25 / +2301-119 / 17 | 4（plan_limits は 11） | 47（`D/tests/work_units.rs`）＋12（replan 系） | replan 回数の上限、stall 検出、計画の版 | Rust 維持 | 予算・上限の安全条件 |
| 失敗の escalation | `D/child_tasks.rs`: `escalate_failed_children`、`RS/escalation.rs`: `escalation_decision`、`RS/availability.rs`: `availability` | child 4 / +448-0 / 3、RS 側 1 / +197-0 / 1 | 4、11 | 5（`D/tests/route_failure_escalation.rs`。`availability.rs`・`escalation.rs` の file 内は 0） | dependency_failed の自動 cancel、escalation の上限、auth 失敗アカウントの除外・quota 残量 | Rust 維持 | 上限・認証失敗の除外は安全条件。変更頻度も低い |
| worker の起動と終了処理 | `D/dispatch_run.rs`: `dispatch_one`、`D/worker_finish.rs` | 65 / +2217-526 / 56、29 / +2431-317 / 21 | 4 | 84（`D/tests/tick_and_dispatch.rs`・`orphan_takeover.rs`・`ownerless_runs.rs`・`work_units.rs`） | lease、runs 行、events 追記、sandbox、process | Rust 維持 | 「真実は SQLite、worker はステートレス」と lease の不変条件 |
| WorkUnit の状態機械 | `task-dispatch/src/execution_scheduler.rs`: `decide`・`complete`・`continuation`・`blocked` | 15 / +1724-860 / 3 | 4 | 23（`execution_scheduler/tests.rs`） | WorkUnit・Task の状態遷移、continuation 上限 | Rust 維持 | 純粋関数だが scheduler の状態機械そのもの（親 task の制約で Rust が主体） |
| ready の並び順 | `task-core/src/store/tasks.rs`: `ready_tasks_impl`（`ORDER BY priority DESC, created_at ASC`）、`store/query.rs` の `ListOrder::Dispatch` | 15 / +977-76 / 11 | 11 | 2（`store/tests.rs` の `ready_tasks_*`） | SQLite の query、承認待ちの親・未完了の依存の除外 | Rust 維持 | scheduler の優先順位で、SQL の中にある |
| review 同期の順位 | `D/stale_priority.rs`: `stale_priority_cmp`・`order_review_sync_queue` | 2 / +164-0 / 1 | 4 | 6（`D/tests/stale_priority.rs`・`phase_effect_ab/stale_priority.rs`） | pre-review sync queue、FIFO の救済 | Rust 維持 | scheduler の優先順位。変更頻度が低い（90 日で 2） |
| WU の ready 判定 | `task-core/src/execution_plan/scheduling.rs`: `newly_ready_with` | 12 / +1103-60 / 8 | 11 | 2（`execution_plan/tests.rs` の `newly_ready*` を含む試験） | WU の依存、外部の done | Rust 維持 | scheduler の依存解決 |

### 2.4 表から除いたもの（削除済みの機構）
- 旧 `[model_routing]`: config の読み込みで拒否される（`celeris/src/config/mod.rs`「[model_routing] is no longer supported」）ので、移す対象が無い。
- tier binding: main のコードに定義が無い（`grep -rn "tier_binding\|TierBinding" crates` が 0 件）。
- routing_shadow 表: migration 0075 で `routing_shadow_reservations` を消した。残るのは追記専用 events を読むための `RoutingShadowRecorded` の event kind だけで、判定ロジックではない。

## 3. 再ビルド範囲（逆依存）
| 変更する crate | 通常依存の逆依存（推移的、自身を除く） | 数 |
|---|---|---|
| task-core | celeris, celeris-campaign, celeris-credentiald, celeris-mcp, celerisctl, llm-proxy, routing-eval, task-api, task-dispatch, task-ops, task-worker | 11 |
| task-ops | celeris, celeris-campaign, celeris-mcp, celerisctl, llm-proxy, task-api, task-dispatch | 7 |
| task-worker | celeris, celeris-campaign, celerisctl, llm-proxy, task-dispatch | 5 |
| task-dispatch | celeris, celeris-campaign, celerisctl, llm-proxy | 4 |
| task-api | celeris, celerisctl | 2 |

TS 移行の候補で最も変更頻度が高い lane 規則表・route 順位付けは task-core にあり、1 行の方針変更でも 11 crate の再ビルドと
`celeris`・`celerisctl` の link になる。compile/link の秒数は baseline-tool・baseline-cos の WorkUnit が測る（この表には書かない）。

## 4. CoS のまとまった非同期経路の候補と S4 の推し

| 候補 | 経路（起点 → Rust の I/O → 判定 → 出力） | TS に出す部分 | Rust に残す部分 | 試験 | 前段の Rust 変更 |
|---|---|---|---|---|---|
| A. 受信箱一次対応の digest | triage run の終了 → `Dispatcher::triage_digest` が `cos_inbox_items` と run 状態を読む → `digest_message` → inbox thread の assistant message | `digest_message`（文面と cards） | 読み出し、`running`→`pending` の戻し、message の書き込み | 11 | `CosTriageItemReport` に `Serialize` を付ける（今は `Debug, Clone, PartialEq, Eq` だけ） |
| B. 定期見直し（cos_review cron）の入力 | cron の予定時刻 → `post_cos_review` が冪等 key を確かめる → `review_text` → back thread に 1 件投入 | 文面組み立て | 冪等 key、`project_brief_list` の読み出し、投入 | 7 | `review_text` から store 読み出しを外へ出す |
| C. 受信箱の取り込み分類 | events の cursor 読み → `touched_tasks`・`sources_for_touched`・`is_noise` → `cos_triage_ingest_batch` | noise 判定・分類・対象 task 抽出 | cursor、挿入 transaction、policy_version | 35 | 入力（`EventRow`・`InboxItem`・`Notice`）の直列化 |
| D. CoS 不在時の退避の文面 | tick → `candidates` → `unavailable_reason` → `fallback_body`・`notice_text` → notice | 文面 | 期限判定（安全網）、冪等 key | 27 | なし |

S4 の第一候補は A（digest）。理由は次の 3 点。
1. 境界が既にある: `digest_message(run_id, run_state, reason, items) -> (String, Vec<ChatCard>)` は I/O も時計も持たない。
   前段の変更は derive 1 つで、S4 の差分を「言語移行だけ」にできる（親 task の制約「言語移行と挙動変更を混ぜない」）。
2. 安全条件と結合しない: 出力は message 1 件で、状態遷移（`running`→`pending`）は呼び出し側の Rust に残る。認可・予算・human_required に触れない。
3. 比較が決定的: 同じ入力で文字列と cards が byte 一致するかで Rust 版と TS 版を比べられる（S5 の差分比較モードにそのまま使える）。

B は次点。文面の変更頻度は同程度だが、`review_text` の store 読み出しを外へ出す Rust 側の変更が先に要る。
C は変更頻度が最も高い（dispatcher 16・store 16）ので効果は大きいが、cursor と transaction に近く、S4 の最初には重い。

## 5. 調べ方（再現の command）
repo の root で実行する。期間は固定日付で書いたので、日が変わっても同じ結果になる（main が進めば変わる）。

```sh
# 基準
git rev-list --count "$CELERIS_WU_BASE"..main          # 0 なら base = main
cargo --version

# 変更頻度: commit 数（90 日・30 日）と追加/削除行
cat > "$TMPDIR/churn.sh" <<'EOF'
label=$1; shift
c=$(git log main --no-merges --since=2026-08-04 --format=%H -- "$@" | wc -l)
l=$(git log main --no-merges --since=2026-08-04 --numstat --format= -- "$@" | awk '$1!="-"{a+=$1;d+=$2} END{printf "+%d/-%d", a, d}')
c30=$(git log main --no-merges --since=2026-10-03 --format=%H -- "$@" | wc -l)
echo "$label | $c | $l | $c30"
EOF
sh "$TMPDIR/churn.sh" model_policy crates/task-core/src/model_policy.rs crates/task-core/src/model_policy
sh "$TMPDIR/churn.sh" execution_gate crates/task-core/src/execution_gate.rs crates/task-core/src/execution_gate
sh "$TMPDIR/churn.sh" digest crates/task-dispatch/src/dispatcher/cos_chat/digest.rs
# 他の行も表の file 列の path を渡す

# 逆依存 crate 数（通常依存、推移的）
for c in task-core task-ops task-dispatch task-api task-worker; do
  cargo tree -i "$c" --workspace -e normal --prefix none --offline \
    | awk '{print $1}' | sort -u | grep -v "^$c$" | wc -l
done

# 試験の数（file 内の #[test] と #[tokio::test]）
grep -cE '#\[(tokio::)?test' crates/task-core/src/model_policy/tests.rs
grep -cE '#\[(tokio::)?test' crates/task-dispatch/src/dispatcher/tests/cos_chat_triage*.rs   # file ごとに出る。合計は cat してから数える

# 入口の関数の位置
grep -n "const RULES\|pub fn decide_for_task" crates/task-core/src/model_policy.rs
grep -n "pub fn decide_at\|pub fn out_of_scope_rule" crates/task-core/src/execution_gate.rs
grep -n "pub fn evaluate" crates/task-core/src/direct_route.rs
grep -n "pub fn route_select\|pub fn select_route_with\|pub fn record_route_selection" crates/task-dispatch/src/dispatcher/route_select.rs
grep -n "pub(crate) fn digest_message\|pub(crate) fn review_text" crates/task-dispatch/src/dispatcher/cos_chat/*.rs
grep -n "ORDER BY priority DESC" crates/task-core/src/store/tasks.rs

# 削除済み機構の確認
grep -rn "model_routing" crates/celeris/src/config/mod.rs
grep -rn "tier_binding\|TierBinding" crates --include=*.rs | wc -l      # 0
ls crates/task-core/migrations | grep routing_shadow                     # 0049 作成、0075 削除
```

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `git diff --stat "$CELERIS_WU_BASE" -- crates web` → 出力なし（crates/・web/ に差分なし）
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`、exit 0
- `bash scripts/dev/test-affected.sh` → 「試験対象の変更なし（docs のみ等）」、docs 検査の self-test は ok、exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（46.9 秒）
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 変更頻度は `--follow` なしの数。分割で生まれた file の古い履歴は数えていない。
- 「判定のみ切出し」の行の TS 側の型（入力・出力の JSON schema）は adr の WorkUnit で決める。

## 提案
- S4 の前段として、`CosTriageItemReport` に `Serialize`/`Deserialize`/`JsonSchema` を付ける Rust の変更を、言語移行とは別の差分で入れる。
