---
tasks: [01M6JW1EFF381XRN953BX8PG81]
---
# ADR 2026-11-04: Project Controller — 案件を長期に自律運営する仕組みの境界と A1〜A9 の契約

- 状態: 採用（設計）。この ADR ではコードを変えない。実装は親 task 01M6JW13XAQ3169H3TTB3H43ZY の子 A1〜A9。
- 基準: 最新 main（`SCHEMA_VERSION` 110、最新 migration は `0110_script_exhaustive_runs.sql`）を読んで書いた。
- 関係: [ADR-0079](0079-recursive-task-decomposition.md) D12・D13、[project brief](2026-10-27-project-brief.md)、
  [cron cos-review](2026-10-27-cron-cos-review.md)、[review back thread](2026-10-28-cos-review-back-thread.md)、
  [停滞の検出](2026-10-27-project-stagnation-inbox.md)、[窓口 1 本](2026-10-28-single-human-window-general-cos.md)、
  [固定 thread](2026-10-22-cos-fixed-threads.md)、[親は子を待つ](2026-10-18-parent-waits-for-children.md)、
  [continuation 上限](2026-10-20-continuation-limit-by-progress.md)、[TS 制御層の境界](2026-11-02-ts-control-layer-boundary.md)、
  [TS 管理 CLI](2026-11-03-ts-control-ops-cli-observe.md)、[方針改善の分離](2026-11-03-ts-intended-behavior-change.md)、
  [execution_gate 領域](2026-11-04-ts-execution-gate-domain.md)、[CoS operations](2026-10-09-cos-operations-all-mutations.md)、
  [破壊的変更](2026-10-11-breaking-changes-by-default.md)、[全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)。

## 結論
- 長期の方針の入れ物は既存の **Project** のままにする。独立の Goal は足さない。中期の完了可能な仕事は複数の **root task**、実行は Task tree / ExecutionPlan / WorkUnit のまま。
- 足すのは案件ごとの **controller の状態 1 行**と、**wake（再評価のきっかけ）**、**判断の記録**の 3 表だけ。task の状態機械は触らない。
- root は普通に done / failed で終わる。続きは Project 側の controller が新しい root として起票する。root を running のまま持たない。
- 流れは「events 等 → wake → snapshot → TS の proposal → Rust の検証 → 1 transaction で記録 → 冪等に実行」。判定は TS、状態遷移・store・認可・予算・安全は Rust。LLM は `replan` が選ばれたときに既存の案件 CoS run を 1 回起こすだけで、常駐しない。
- `Project.request` が正本、`ProjectBrief` は派生の要約。CoS は人の依頼文・目的・受け入れ条件を変えられない（現在は変えられるので A3 が塞ぐ）。
- 既定は全案件で無効。有効にするのは人の API token だけで、許可リストに載せた案件だけ。無効へ戻すのは誰でもできる。本番の変更はこの依頼の範囲外で、人の手順として書く。

## 背景
人の依頼（chat message 01M6JVJVV89N6N1N4AP6PZ2RKN）の核心は「完了した root の後に、人が『次のタスクを起票して』と頼み直さないと進まない」こと。CoS が起きるきっかけは人の発言・受信箱の新着・root の完了・定期見直し（裏 thread）だけで、案件の目的に照らして次の root を決める仕組みが無い。

## 決定

### D1 独立 Goal を基本不採用にする
理由:
1. 目的の入れ物は既にある。`Project.title`・`request`（`crates/task-core/src/org.rs`）、`standing_rules`、案件の pause・cancel。ADR-0079 D13 が「案件が持ち続けるもの」と定めた。
2. Goal を足すと「案件と Goal のどちらが正本か」「Goal の状態と task の状態の二重管理」が生まれる。ADR-0074 の案件計画と途中目標は、まさにこの二重で一度も使われず撤去された（ADR-0079 §背景 T3・T4）。
3. 依頼が必要とするのは目的の入れ物でなく、**再評価のきっかけと判定**である。これは wake と TS の proposal で足りる。

進捗の台帳は Goal を持たずに作れる: task ledger は `Project.request`＋Outcome（D6）＋root の acceptance、progress ledger は root の状態・evidence・events から毎回生成する。A5 の Roadmap も Project に紐づく計画の版で、独立の Goal ではない。

例外の条件: 1 案件が互いに無関係な複数の長期目標を持ち、案件を分けると `project_repos`・standing rule が破綻する実例が出たら、この決定を見直す。この ADR を書く時点でその実例は観測していない。

### D2 旧 milestones API と project_plan の 410 を維持する
`crates/task-api/src/milestones.rs`（`MILESTONE_GONE`）と `crates/task-api/src/project_plan.rs`（`PROJECT_PLAN_GONE`）の 410、registry の `removed_by_adr_0079`（`crates/task-api/src/cos/ops/projects.rs` の `EXCLUDED`）はそのまま。理由:
1. 旧 milestones の意味（人の `ok` / `discuss` / `ng`、Go、`auto_advance`、dispatch での `in_progress`）を生かすと ADR-0079 で撤去した分岐が戻る。本番の 25 行は凍結の履歴で、書き換えない。
2. 依頼が欲しいのは「達成したい状態」で、旧 milestone の「タスクの束」とは意味が違う。同じ URL を別の意味で再開すると、生成型・web・既存の試験（`crates/task-api/tests/project_plan.rs`）の読み手を誤らせる。
3. A5 の Roadmap は別の名詞（`/projects/{id}/roadmap`・`outcome`）で足す。`Task.milestone_id`・`milestones` 表・`projects.auto_advance` を読まない。
4. 読み取りの `GET /projects/{id}?include_frozen=true` は履歴として残す。

### D3 root の長期 running を避ける方式
- 長期の継続は `project_controller_state`（Project 側）が持つ。root は通常の状態遷移で終端になる。
- controller は `transition`・`Trigger`・lease を呼ばない。task に対して行うのは `task_ops::add` 経由の新規作成だけ。既存 root の done・failed を書き換えない。続きの root は `continues_root_ids`（D13）で履歴を結ぶ。
- 「常設の監視 root」「永久に子を待つ合成の親」を作らない。`aggregate=true` の親を controller が作らない。
- 試験（A1、A9）: 全シナリオの後で、controller が作った task に run も lease も無いまま `running` の行が 0、controller の module から `Trigger`・`transition` を import していない（grep の check）。

### D4 Project.request と ProjectBrief の正本・派生
| 欄 | 役割 | 書ける主体 | controller の読み方 |
|---|---|---|---|
| `Project.request` | **正本**。人の依頼文そのまま（`org.rs` のコメントどおり） | 人。CoS は人の発言を `instructed_by` で引いたときだけ（A3 が今の抜けを塞ぐ） | 目的の判定の根拠。`request_sha256` を判断に記録する |
| `project_request_revisions`（A3 の新表） | request の版の履歴。追記専用（trigger） | request を書く経路が同じ transaction で書く | 変わったら wake `human_changed`（D12） |
| `ProjectBrief`（`objective`・`state`・`open_issues`・`next_actions`・`waiting_on`・`evidence_refs`） | **派生**の動的な要約。全置換 | 案件 CoS（自分の案件だけ）と人 | 表示と CoS の文脈。**判定の根拠にしない**。`evidence_refs` の `outcome:<id>:task:<root id>` は主張で、Rust が root の状態で確かめる |
| Outcome（A5 までは request の中の block、以後 Roadmap） | 途中目標の定義 | request の block は人、Roadmap の版は人の承認 | gap の分母 |

`brief.objective` が request と食い違っても自動で直さない。controller は `open_issues` に書くよう CoS に頼むだけ。

### D5 人の original objective / acceptance の保全
- 正本は追記専用の events: task の最初の `Event::Created` の `objective`・`acceptance`。人の発言は chat の `role=user`（`author = human:<name>`）。
- **今の抜け 2 つを A3 が塞ぐ**（読んで確かめた現状）:
  1. CoS op `project.update`（`cos_patch_project`）は根拠なしで `request` を書き換えられる。→ `instructed_by` が無ければ 422 `project_request_human_only`。
  2. CoS op `task.update`（`patch_task_op` → `task_ops::edit`）は非終端の task の `objective`・`acceptance` を書き換えられる（draft 以外も）。今は `plan_conforms` の G2(4) が「Edited の後は自動承認しない」だけ。→ draft を除き `instructed_by` が無ければ 422 `task_goal_human_only`。
- controller が起票する root の acceptance は Outcome の acceptance を**文字どおり含む**。少ない・言い換えたら Rust が `root_acceptance_narrowed` で拒否する（I8）。
- `task_ops::original_goal(events)`（A3）が Created の値を返し、reviewer の prompt と `GET /tasks/{id}` に `original_goal` として出す。Edited があるときは人の根拠（`instructed_by`）の無い差分を final review が不合格の材料にできる。
- 人の決定で受け入れ条件が書き換わらない方針（KB `projects/agent-platform/decision-acceptance-immutable.md`）は変えない。

### D6 進捗の定義
層ごとに分ける。下の層の進捗を上の層の進捗に数えない。
| 層 | 定義 | 実装 |
|---|---|---|
| run | checkpoint の差（files・HEAD・completed・remaining） | 既存 `checkpoint_shows_progress` |
| task / root | 終端の判定は reviewer か決定的 check。root の「成果」は `done` ∧ `ReviewerPassed`（または check のみの合格）∧（repo があれば取り込み済み〈`task_integration_state`〉か `auto_integrate` 無効で merge candidate あり） | 既存 |
| Outcome | 上の成果を持つ root が、その Outcome の acceptance を文字どおり含む、または `evidence_refs` の主張を Rust が確かめた。worker と CoS の自己申告だけでは reached にしない | A2、A5 |
| 案件 | `reached` の Outcome の集合が増えた、または open な root の成果が増えた | A2。`progress_epoch` を判断に記録 |

進捗に数えないもの: brief の更新、判断の件数、task の作成、CoS の発言、cancel された root、失敗の再試行。進捗なしの判断が `max_no_progress_decisions`（既定 5）続いたら `blocked`＋escalate 1 回（A6）。

### D7 TS / Rust の境界（[境界 ADR](2026-11-02-ts-control-layer-boundary.md) (a) をそのまま適用）
| Rust | TS（`control-ts/src/domains/project_controller.ts`） |
|---|---|
| events の取り込み・wake・cursor・lease・CAS・backoff・backpressure | snapshot → gap の計算 |
| snapshot の組み立て、capability の照合 | 次の行動の選択（`rule_id`・`signals` 付き） |
| proposal の検証（D13）、authority の判定（A7）、予算の判定（A6） | 失敗の分類の方針、質問の要否、次の wake までの秒 |
| task の作成、CoS 入力の投入、受信箱への項目 | root の題・本文の組み立て（Outcome の文から） |
| mode の切り替え（人だけ）、監査の記録 | 文面の雛形 |

- TS は「入力 DTO → proposal DTO」。書き込みの native op を作らない。read の op は `project_controller:read_root_evidence` の 1 個だけで、要求の scope（案件 id）の外は `Denied`。
- **参照実装の無い領域**（新しい種類）: controller には旧 Rust の判定が無いので byte 比較ができない。`CompareOutcome::ProposalOnly` を足し、script-guard は成功として数える（不一致は存在しない）。invalid_output・安全違反・timeout・失敗の停止基準はそのまま効く。fail-closed の「Rust の参照」は `wait(script_unavailable)` と backoff で、起票も LLM も起こさない。
- 領域 mode（`script_domain_modes`）と案件 mode（D11）は独立の 2 段: 案件 `shadow` は領域 `compare` 以上、案件 `enabled` は領域 `ts`（または `canary` で選ばれた案件。単位は案件 id）を要する。どちらも上げるのは人だけ。
- golden: `control-ts/fixtures/project_controller/invariants.jsonl` は Rust の dump が書く**不変条件**（`must_not_launch_root` など）で、規則の変更では変わらない。方針の期待値は `control-ts/src/domains/project_controller.test.ts` にあり、これは `control-ts/src/` の中なので ts-only の経路（cargo 0 回）で変えられる。
- **TS-only の cargo 0 回を回帰させない**: 領域の file は 1 つにする（compare.mjs の loader は領域間の相対 import を解決できない）。方針の変更で `crates/`・`control-ts/fixtures/`・host schema を触らなくて済む形に保つ。A2 が `ts-verify.sh --record` で `cargo_invocations = 0` を raw に残し、以後の子は TS-only の変更を持つとき同じ確認をする。

### D8 CoS の権限
| 操作 | 案件 CoS | 統括 CoS（general・review） | 人 | daemon（controller） |
|---|---|---|---|---|
| brief を書く | 自分の案件だけ（既存） | 不可（既存 403） | 可 | 不可 |
| `request` を変える | `instructed_by` 必須（A3） | `instructed_by` 必須 | 可 | 不可 |
| 他者が与えた `objective`・`acceptance` を変える | `instructed_by` 必須（A3） | 同左 | 可 | 不可 |
| 続きの root を起票 | `project.controller_report` の `launch_root` で。enabled の案件で `controller_decision_id` も `instructed_by` も無い `task.create` は 422 `project_controller_owns_launch` | 案件の依頼は人の原文の取り次ぎか A4 の proposal まで | 可（常に優先） | proposal の検証後に実行 |
| 修正 task・replan | 既存 op と triage 7 類型の範囲 | 不可（`relay_forward`） | 可 | 不可 |
| 受信箱の代答 | 既存 7 類型（basis 必須）。controller 用の新しい代答の種類は足さない | 既存（案件の件は不可） | 可 | 不可 |
| 案件 mode を上げる | 不可（403 `project_controller_requires_human`） | 不可 | **人の token だけ** | 不可 |
| 案件 mode を `off` に | 可 | 可 | 可 | 不可（guard の停止は `blocked` にする） |
| Roadmap の版を進める | 提案だけ | 提案だけ | **人だけ** | 不可 |
| authority grant を作る | 不可（人の発言を引いた提案だけ） | 不可 | **人だけ** | 不可 |
| 本番 promote・secret・外部公開・不可逆操作 | 既存の gate のまま。独断で不可 | 同左 | 人 | 不可 |
| 人の発言を起案する | 不可（既存の `relay_forward` は原文のみ） | 不可。A4 の `chat.proposal_forward` は `role=system` で `instructed_by` に引けない | — | 不可 |

変更操作はすべて `/cos/operations` を通す（registry の ALLOWED か EXCLUDED に分類。`cos_ops_registry_*` 試験が全 route を検査するので、新 route は必ずどちらかに入れる）。

### D9 Project Controller の不変条件
試験の名前は A1〜A9 の節に書く。
- I1 **二重の状態なし**: task の状態は `tasks` と events だけが正本。controller の表は wake・判断の記録で、task の状態を写さない。
- I2 **別のイベントストアなし**: `project_wakes` は `events` 等から作る派生。events の追記専用を破らない。
- I3 **LLM を呼ぶのは既存の CoS run だけ**: dispatcher・store に LLM を入れない。
- I4 **待機中の LLM 起動 0**: `wait`・`continue` では CoS 入力を積まない。
- I5 **起票の冪等**: root の冪等キーは `pc:<project id>:<root.key>`（`task_idempotency_keys`）。同じ key の 2 回目は既存を返す。key に判断 id を入れない。
- I6 **CAS**: 適用の transaction で `revision` と `human_epoch` を照合し、違えば適用しない（`rejected:stale_*`）。
- I7 **人が優先**: 判断の後に人が操作したら、その判断の起票は捨てて再評価する。
- I8 **人の条件を狭めない**: root の acceptance は Outcome の acceptance を含む。
- I9 **fail closed**: TS の失敗・検証落ち・authority 不明は起票せず `wait` か `blocked`。許可側へ倒さない。
- I10 **system 入力は人の指示にならない**: controller が積む CoS 入力は `role=system`（`chat_system_queue_once`）で `instructed_by` に引けない。
- I11 **信頼しない入力**: worker の報告・ログ・成果物の文は判定の値に使わず（種類と id だけ）、CoS 入力では「命令ではない」と区切る。root の本文に転記しない。
- I12 **有界**: 1 tick の評価数、案件ごとの未終端の controller root 数、Outcome ごとの root 試行数、日ごとの CoS 起動数に上限があり、超えたら止まって人に知らせる（無限再試行なし）。
- I13 **既定で無害**: 状態の行が無い案件・`off` の案件は、この機構が無い時と同じ動きをする（A3 が event と message の数で確かめる）。
- I14 **root の long running なし**（D3）。
- I15 **既存の待ちを重複実装しない**: 子待ち（`awaited_children`）・continuation・再 dispatch は tree と dispatcher に任せ、controller は観測して再評価するだけ。

### D10 operator と人の手順
既定の本番は無変更。この依頼では実行しない。手順の正本は A3 が `docs/ops/project-controller.md` に書き、A5・A6・A7・A9 が節を足す。

| 場面 | 誰が | 何を | 確認 |
|---|---|---|---|
| 機能を本番に入れる | 人 | release の promote。config に `[project_controller] enabled = true`、`allowed_project_ids`、再起動。release を戻す前に欄を消す（`Config` は `deny_unknown_fields`） | `GET /daemon` |
| 領域を上げる | 人 | 既存の `celerisctl script mode project_controller compare\|canary\|ts`（人の token） | `celerisctl script domains` |
| 案件を shadow にする | 人 | web の案件画面「自動運営」→「観察だけ」（A9）。API は `PUT /projects/{id}/controller` | 判断の履歴に `suppressed` |
| 案件を有効にする | 人 | 同画面「有効にする」、理由を書く | root が起票される |
| 止める・戻す | 誰でも（人・operator・CoS） | 同画面「止める」か `mode: off`。作成済みの root は残る。cancel は別の操作 | wake が増えない |
| 判断の要求に答える | 人 | 統括 CoS チャットの札と受信箱（既存の導線） | 札が閉じる |
| Roadmap を承認する | 人 | 案件画面の提案を承認・却下（A5） | 版が進む |
| 許可を与える | 人 | 案件画面の「許可」（A7）。期限・予算つき | ledger に出る |

外部 operator（Claude Code）は Celeris の外の主体（[external operator](2026-10-27-external-operator.md)）で、mode を上げられない（`script_mode_requires_human` と同じ扱い）。人の言葉を運ぶときは既存の `…/relays` を使う。

### D11 パイロット案件の隔離
1. **案件 mode**: `off`（既定）→ `dry_run` → `shadow` → `enabled`。
   | mode | wake を積む | TS を呼ぶ | 判断を記録 | 効果（起票・CoS 入力） |
   |---|---|---|---|---|
   | `off` | しない | しない | しない | なし |
   | `dry_run` | しない | API で頼まれたときだけ | 応答で返すだけ | なし |
   | `shadow` | する | する | する（`effect_state = suppressed`） | なし。LLM 0 |
   | `enabled` | する | する | する | あり |
2. **許可リスト**: `[project_controller] allowed_project_ids`（既定は空）。無い案件を `shadow` 以上へ上げると 403 `project_controller_not_allowed`。全体の `enabled = false`（既定）なら tick の段は何もしない。
3. **試験と pilot は本番の外**: `scripts/dev/script-compare-rehearsal.sh` と同じく一時 DB・一時 state dir・loopback の port で daemon を起こし、`CELERIS_RUN_DB`・`CELERIS_DB`・`CELERIS_FOLLOWUPS_FILE`・`CELERIS_RUN_ID`・`CELERIS_TASK_ID`・`CELERIS_COS_RUN_CREDENTIAL`・`CELERIS_API_PRINCIPAL` を unset する。script は DB の path が `$CELERIS_STATE_DIR`・`/local/celeris/data/db` の下なら失敗する。
4. **対象案件**: pilot の案件は題名 `pilot:` で始まる使い捨てで、repo は `$TMPDIR` の使い捨ての git（agent-platform を含まない）。K2・K4・IEEE Access の案件・測定 config（`measure/*`・`ops-r2-*`）を対象にしない。測定 config は `[project_controller]` を持たないので既定の無効で動く。
5. **実 LLM**: A9 の pilot だけ。費用の上限を config で固定し、認証が使えなければ手順を書いて人に頼む（ADR-0009 P-34）。24〜72h の実稼働は人の別途の承認の下でだけ。

### D12 controller の流れ
```
[events 等] ─ingest→ project_wakes ─coalesce→ lease → snapshot → decide(TS) → 検証
                                                           │
   tx1: decision 行・wake の消費・cursor・revision+1・effect=pending（CAS 照合）
   tx2: effect を冪等に実行（root 作成 / CoS 入力 / 受信箱）→ effect=applied
```
**wake の種類**（`ProjectWakeKind`）。依頼の 8 種に `human_changed` を足す。
| kind | 元 | wake_key |
|---|---|---|
| `root_done` | `Transitioned{to:Done}` で `task_ops::root_done::is_root_done` が真 | `root_done:<task>:<event id>` |
| `root_failed` | `Transitioned{to:Failed}`、`task_core::is_root_task`、reason が `review_fail` でない | `root_failed:<task>:<event id>` |
| `review_failed` | 同じ遷移で reason `review_fail` | `review_failed:<task>:<event id>` |
| `children_settled` | 子の終端の遷移で `child_wait::awaited_children(親)` が空になった | `children_settled:<親>:<event id>` |
| `decision_answered` | `DecisionAnswered`・`Answered`・`ApprovalDecided`・`PlanGateDecided` | `decision_answered:<task>:<event id>` |
| `external_released` | `ClusterJobWaitFinished`・`BrowserWaitResolved`・`BrowserPrerequisiteResumed`、または wake API | `external:<task>:<event id>`、`external:<ref>:<idempotency_key>` |
| `stagnation` | `DaemonSnapshot.stagnation` の `StagnationFinding`（`stagnation_known` が真のときだけ） | `stagnation:<論理キー>:<since>` |
| `cron` | cron の新しい雛形 `extra.kind = "project_reconcile"` | `cron:<job>:<予定時刻 unix 秒>` |
| `human_changed` | `human_epoch` の増加（下） | `human:<epoch>` |

- 全体の取り込み cursor は `feed_cursor` の `project_wake`（`root_done_summary` と同じ）。初回は末尾から。案件別の cursor は `project_controller_state.event_cursor`（判断に畳んだ events.id の最大）。
- **coalesce**: 最初の wake から `coalesce_secs`（既定 30）待って 1 回の評価に畳む。
- **lease**: instance id と期限（`lease_secs` 既定 60）。draining の instance は新しく取らない（`accepting_new_work`）。切れたら他が引き継ぐ。
- **human_epoch**: 次の経路が**同じ transaction で**増やす（`SqliteStore::project_controller_human_bump_tx`）: `patch_project`、`project_action`、人の案件 thread への発言と `…/relays`・`…/forwards`、人の task の作成・cancel・retry・reopen、人の受信箱の回答。
- **retry/backoff**: decide の失敗で `retry_base_secs`（60）× 2^n、上限 `retry_max_secs`（3600）。`max_retries`（5）で `phase = blocked`。
- **backpressure**: `max_evaluations_per_tick`（2）、古い wake の順。CoS の入力は `max_cos_inputs_open_per_project`（1、未処理があれば次を積まない）。
- **crash の境界**（A1 が全部試験する）: B1 ingest 前、B2 ingest 後・評価前、B3 lease 後・判断前、B4 tx1 の後・効果の前、B5 効果の後・`applied` の前、B6 `applied` の後。tx1 の effect を読み直して同じ key で実行するので、どこで落ちても効果は 1 回。
- **phase**（A1 が列を持ち、A6 が遷移を完成させる。`ControllerPhase`）: `active`・`waiting`・`blocked`・`paused`・`achieved`・`cancelled`。Task の running とは独立。`paused`・`cancelled` は `Project.status` に従う。`achieved` は全 Outcome が reached で、`Project.status` は変えない（案件の終わりは人が決める。SPEC §7）。

## 依頼が挙げた既存機構の棚卸し（最新 main で読んだ。1 件ずつ）
| 機構 | 読んだ所 | 判定 | 使い方・不足 |
|---|---|---|---|
| ADR-0079 D12 | §D12 | **再利用**＋付記 | 「1 依頼 = 1 root」「CoS は範囲を狭めない」を保つ。不足: 人の依頼が無いときに続きの root を作る経路が無い（D8 で足す）。A4 が「相談を全て 1 root」の読み方を直す（preamble の `create_task` の節、`crates/task-worker/src/preamble.rs`） |
| ADR-0079 D13 | §D13 | **再利用** | 案件が持つ物の表をそのまま土台にする。不足: 途中目標の代わりが無い（A5、別名詞）。D14 の「途中目標という語を新しい画面で使わない」は A5 が付記で改める |
| `crates/task-api/src/milestones.rs` | 全文 | **再利用（凍結のまま）** | 410 を維持（D2）。足さない |
| `crates/task-api/src/project_plan.rs`・`task-ops/src/project_plan.rs`・`task-core/src/project_plan.rs` | 全文・冒頭 | **再利用（凍結のまま）** | 410 と履歴の読み取りだけ。`celeris.project-plan/1` を Roadmap に流用しない |
| `crates/task-core/src/tree.rs`・`tree/`・`task-ops/src/tree.rs`・`task-api/src/tree.rs`・`store/execution/tree.rs` | 冒頭・公開関数 | **再利用** | `max_depth` 3、`is_root_task`。phase を長期の途中目標に流用しない。controller は root だけを作り木の深さを増やさない。不足なし |
| `crates/task-core/src/project_brief.rs`・`store/project_briefs.rs`・`handlers/projects.rs::put_project_brief_op` | 全文 | **再利用**＋不足 | D4 の派生。不足: 版が無く盲目的な全置換（`expected_revision` と照合しない）、履歴は `cos_operations` の payload にしか無い。A3 が `PUT /projects/{id}/status` に `expected_updated_at` の照合を足す |
| ADR cron-cos-review（`dispatcher/cron_tick.rs`・`task_ops::cron_jobs`・`CronInputSink`） | 全文 | **再利用**＋不足 | 発火の規則、`client_message_id` の冪等、`skipped_overlap`。不足: 積み先が統括の裏 thread だけ。A1 が `project_reconcile` を足す |
| ADR cos-review-back-thread（`cos_chat/review.rs`・`chat/review.rs`・`task-api/src/cos/review.rs`） | 全文 | **再利用** | 案件横断の見直しはこのまま。`chat.review_report` は人への短報の型。案件の再評価は controller が担い、統括の裏 thread に積まない |
| ADR project-stagnation-inbox（`task_ops::stagnation`） | 全文 | **再利用** | `StagnationFinding` を wake の元にする（判定を書き直さない）。論理キーと since で冪等。不足: 知らせるだけで再評価が無い |
| ADR single-human-window-general-cos | D1〜D6 | **再利用**＋A4 | 札・`chat.relay_forward`（人の原文のみ）・統括の排他を変えない。不足: CoS 起案を案件へ渡す経路が無い。A4 が別 route に足す |
| ADR cos-fixed-threads（`cos_thread_ensure_tx`） | D1〜D5 | **再利用** | controller の CoS 入力は案件 thread へ。session はそこで継続。新しい thread 種別を足さない |
| `config/skills/cos-operator/` | SKILL.md・operations.md | **再利用**＋追加 | A3 が `project-controller.md`、A4 が `consult.md` を足す。表に 1 行ずつ |
| `config/skills/cos-inbox-triage/` | SKILL.md（policy 10） | **再利用** | 7 類型と `human_required` をそのまま判定に使う。新しい代答の種類は足さない。版を上げるのは A7（authority の refs）だけ |
| `crates/task-worker/src/cos_chat.rs` | Core・各 section | **再利用**＋A3・A4 | 1 run = 1 turn、subagent・他 LLM CLI 禁止を保つ。不足: 案件の `request` と controller の状態が文脈に無い（A3）、相談・短い読取の指針が無い（A4） |
| `crates/task-dispatch/src/dispatcher/cos_chat/review.rs`（＋`task-dispatch/src/review.rs`） | 全文 | **再利用** | sink の形を A1 が真似る。後者は reviewer の起動で別物、変えない |
| `crates/task-dispatch/src/dispatcher/cos_chat/triage.rs`・`task-core/src/chat/triage.rs` | 冒頭・関数 | **再利用** | events cursor・`touched_tasks`・`is_noise`・`cos_triage_direct_escalate`。controller の escalate は受信箱の新種類 `project_controller` から同じ経路に乗せる |
| ADR parent-waits-for-children（`task_ops::delegate::child_wait`） | 全文 | **再利用** | `awaited_children` を `children_settled` の判定に読むだけ。親を起こすのは既存（I15） |
| ADR continuation-limit-by-progress（`checkpoint_shows_progress`） | 全文 | **再利用** | run の進捗の定義（D6 の最下層）。controller は continuation を数えない |
| ADR ts-control-layer-boundary | (a)〜(d)・付記 | **再利用**＋不足 | actor・Host API・generation・guard・mode。不足: 参照実装の無い領域（D7 の `ProposalOnly`） |
| TS 移行 S6（進捗 `2026-11-02-ts-control-layer-s6.md`） | 結論・表 | **再利用（基準値）** | warm p50 `policy-ts` 7.519 秒・`cos-ts` 7.470 秒が A2 の比較相手。本番の標準稼働は 0 領域 |
| ts-control-ops（進捗 `2026-11-03-ts-control-ops.md`、ADR cli-observe、intended-change） | 結論・表 | **再利用** | release gate の 4 段、`celerisctl script`、`script_tick_wait`、behavior-changes、`script-compare.py`。controller も同じ道具を通す |

調べて足した関連（依頼の表には無いが再利用する）: `task_idempotency_keys`（`store/idempotency.rs`、I5）、`chat_system_queue_once`（`chat/system_request.rs`、I10）、root 完了のまとめ（`cos_chat/root_done.rs`。残すが、enabled の案件では「次の root は起票しない」と入力に書く。A3）、`feed_cursor`、`decision_log`（campaign 用で案件の判断には使わない。主キーが run 単位で合わない）、root 同士の `depends_on`（[ADR](2026-10-17-root-dependency-waits-for-integration.md)。続きの root は成果の取り込みを待つときに使う）。

## 実ファイル・関数の責任表（Project Controller の経路）
| 責任 | 既存 file・関数 | 新しい場所（担当） |
|---|---|---|
| wake の種類・状態の型 | — | `crates/task-core/src/project_wake.rs`（A1） |
| 表の読み書き・CAS | `store/idempotency.rs`・`store/root_done.rs` を手本 | `crates/task-core/src/store/project_wake.rs`（A1） |
| ingest・lease・適用の tick | `dispatcher/cron_tick.rs`（`tick_cron_jobs`）から呼ぶ | `crates/task-dispatch/src/dispatcher/project_wake.rs`（A1） |
| 判定の口 | `script_policy.rs::PolicyScript`（既定実装つきの method） | A1: trait `ProjectDecider`。A2: `PolicyScript::project_controller`。A3: 両者の接続 `dispatcher/project_decider_ts.rs` |
| snapshot の組み立て | `task_ops::view`・`derive`・`human_inbox` | `crates/task-ops/src/project_snapshot.rs`（A2） |
| proposal の DTO・検証・Outcome の解析 | `execution_gate` の検証の形 | `crates/task-core/src/project_proposal.rs`（A2） |
| TS の判定 | `control-ts/src/domains/execution_gate.ts` を手本 | `control-ts/src/domains/project_controller.ts`（A2） |
| root の起票 | `task_ops::add`、`idempotency_record_tx` | 新規関数なし（A1 の効果、A2 の試験の両方が既存を呼ぶ） |
| CoS を起こす | `chat_system_queue_once`、`chat_run_claim_next` | 新規関数なし |
| CoS からの報告 | `cos/ops/projects.rs`、`handlers/projects.rs` | `crates/task-api/src/handlers/project_controller.rs`（A1 が作る。A3・A7 は別 file から route を足す） |
| 許可の判定 | `cos/basis.rs`、`standing_rules` | `crates/task-core/src/authority.rs`・`store/authority.rs`（A7） |
| Roadmap | — | `crates/task-core/src/roadmap.rs`・`store/roadmap.rs`・`crates/task-api/src/handlers/roadmap.rs`（A5） |
| 画面 | `web/features/projects/project-detail-view.tsx` | `project-controller-panel.tsx`（A9）、`project-roadmap-panel.tsx`（A5）、`project-authority-panel.tsx`（A7） |

## Hermes / Magentic-One に対応する不足
Hermes の persistent goals・heartbeat・cron は人の依頼の語で対応づけた。外部資料はこの run で読んでいない（使える外部の道具に無い）ので、**機能名の対応で、製品の細部は未確認**。Magentic-One は著者の知識（task ledger＝facts/guesses/plan の Outer Loop、progress ledger＝完了か・進んでいるか・ループか・次は誰かの Inner Loop、停滞が閾値で Outer へ戻る）による。

| 参照の機能 | Celeris の今 | 不足 | 埋め方 |
|---|---|---|---|
| Hermes persistent goals | `Project.request`、root 一覧、brief | 目的に照らして次を決める者がいない | Outcome（request block → Roadmap）と controller。Goal は足さない（D1）。A2・A5 |
| Hermes heartbeat | 定期見直し（統括のみ）、生存確認（task 単位） | 案件ごとの定期の照合が無い | `project_reconcile` の cron と `next_wake_at`。A1・A6 |
| Hermes cron | ADR-0131 cron（task か cos_review） | 案件の再評価の種類が無い | `CronJobKind::ProjectReconcile`。A1 |
| Magentic task ledger | brief（CoS 手書き）、root の acceptance | 機械が読める「確かな事実と計画」が無い | snapshot の `outcomes`・`roots`、`gap`。A2 |
| Magentic progress ledger | checkpoint、tree の roll-up、stagnation | 案件単位の「完了か・進んでいるか・ループか」が無い | proposal の `gap`＋`progress_epoch`（D6）、判断の記録。A2・A6 |
| Magentic Inner Loop | Task tree・ExecutionPlan・WU・continuation・repair | **不足なし**（重複させない） | I15 |
| Magentic Outer Loop | 人が頼み直す。tree の replan は root の中まで | 案件をまたぐ replan が無い | `replan` → 案件 CoS run → `project.controller_report`。A3 |
| 停滞で replan | `max_no_progress` は run・WU 単位 | 案件単位の guard が無い | `max_no_progress_decisions`。A6 |

## 契約: 共通 DTO
正本は Rust の型（schemars で schema を出す）。ここが固定する形を変えるときは ADR に付記する。

```jsonc
// ProjectSnapshot（Rust → TS。schema celeris.project-snapshot/1。上限 1 MiB、超える分は古い done の root から落とし truncated を真に）
{
  "now": "RFC3339", "truncated": false,
  "project": {"id":"…","title":"…","status":"active","request":"…","request_sha256":"…","auto_integrate":false},
  "brief": null,                     // ProjectBrief そのまま。判定の根拠にしない
  "outcomes": [{"id":"o1","title":"…","acceptance":["…"],"depends_on":[],"priority":1,"source":"request_block"}],
  "roots": [{"id":"…","status":"done","title":"…","controller_key":"outcome:o1:1","outcome_ids":["o1"],
             "acceptance":[{"text":"…","kind":"command","result":"pass"}],
             "reviewer_passed":true,"integrated":true,"failure_class":null,"attempts":0,
             "open_waits":["decision"],"leaf_done":3,"leaf_total":3,"continues_root_ids":[]}],
  "waits": [{"kind":"decision","id":"…","task_id":"…","age_secs":120}],
  "stagnation": [{"kind":"project_stalled","key":"…","since":"RFC3339"}],
  "wakes": [{"kind":"root_done","ref":"…","event_id":123}],
  "decisions": [{"id":"…","kind":"launch_root","rule_id":"…","effect_state":"applied","root_key":"outcome:o1:1","progress_epoch":1,"at":"RFC3339"}],
  "controller": {"mode":"enabled","phase":"active","revision":4,"human_epoch":1,"retry_count":0,"progress_epoch":1},
  "limits": {"max_open_roots":2,"max_root_attempts_per_outcome":2,"max_no_progress_decisions":5,"cos_inputs_remaining_today":10,"roots_remaining_today":5},
  "roadmap": null,                   // A5 が埋める。無ければ null
  "authority": null                  // A7 が埋める。無ければ null（launch_root は authority が null なら Rust の既定規則）
}

// ProjectProposal（TS → Rust。schema celeris.project-proposal/1。deny_unknown_fields）
{
  "action": /* 下の判別 union（tag = kind）*/,
  "gap": {"reached":["o1"],"open":["o2"],"unknown":[],"progress_epoch":1},
  "explanation": {"rule_id":"pc/outcome-open","signals":[{"name":"open","detail":"o2"}]},
  "based_on": {"revision":4,"human_epoch":1,"request_sha256":"…"}
}
```
| 依頼の語 | `action.kind` | 欄 | Rust の効果（`enabled`） |
|---|---|---|---|
| continue | `continue` | `watch_root_ids`・`next_wake_after_secs` | 効果なし。`phase = active`、`next_wake_at` |
| replan | `replan` | `reason_code`・`question`・`context_refs` | 案件 thread に `role=system` の入力 1 件（key `pc-input:<decision id>`） |
| launch-root | `launch_root` | `root: RootSpec` | `POST /tasks` と同じ検証で root を作る。key `pc:<project>:<root.key>`、`created_by = daemon`、label `project-controller` |
| wait | `wait` | `waiting_on[]`（`WaitingOn` の型）・`next_wake_after_secs`・`phase`（`waiting` か `achieved`） | 効果なし |
| escalate | `escalate` | `category`（triage の `human_required` の名）・`question_key`・`summary`・`options[]`・`evidence_refs[]` | 受信箱の種類 `project_controller` に 1 件。同じ `question_key` は開いている間 2 回目を作らない |
| mark-milestone | `mark_milestone` | `outcome_id`・`evidence_refs[]` | A5 までは判断の記録だけ。A5 以後は Outcome の `status` を提案 |

`RootSpec`: `{key, title, objective, acceptance:[{text, check}], stages_hint:[], continues_root_ids:[], outcome_ids:[]}`。`key` は `outcome:<id>:<n>` の形で `n` は試行番号。

**検証**（`task_core::project_proposal::validate`。落ちたら起票せず `invalid_output`、下の `*` は安全違反で guard が領域を `rust` へ戻す）:
`based_on` が snapshot と一致 / 未知の欄なし・文字数の上限（`objective` 8,000、`title` 200、`signals` 16）/ `*root_acceptance_narrowed` / `*root_key_not_owned`（`outcome_ids` が snapshot に無い）/ `root_attempts_exceeded` / `open_roots_exceeded` / `*escalation_category_unknown` / `*launch_on_paused`（案件が active でない）/ `mark_milestone` の `evidence_refs` が実在の root を指す。

**request の Outcome block**（A5 より前の PoC。無ければ `outcomes: []` で、TS は `replan(no_outcomes)` を request の版ごとに 1 回だけ返す）:
````
```celeris-outcomes
[{"id":"o1","title":"…","acceptance":["…"],"depends_on":[],"priority":1}]
```
````
解析は Rust の `project_proposal::parse_outcomes`。id の重複・循環・形の誤りは `outcomes: []`・`outcomes_invalid` の wait＋escalate。

**表**（A1 の migration。番号は着地時の最新 main の次）:
```sql
project_controller_state(project_id PK, mode, phase, revision, event_cursor, next_wake_at, coalesce_until,
  lease_owner, lease_expires_at, retry_count, backoff_until, last_decision_id, human_epoch, progress_epoch, updated_at, updated_by)
project_wakes(id PK, project_id, wake_key, kind, source_event_id, source_ref, created_at, consumed_by, UNIQUE(project_id, wake_key))
project_controller_decisions(id PK, project_id, based_on_revision, human_epoch, snapshot_sha256, request_sha256, generation_id,
  mode, proposal_json, validation, effect_json, effect_state, effect_ref, progress_epoch, created_at, applied_at)
```
`project_controller_decisions` は `effect_state`・`effect_ref`・`applied_at` の前進だけ UPDATE でき（`effect_state` は `none|pending|applied|suppressed|failed`）、他の欄の UPDATE と DELETE は trigger で拒む。

## 契約: 子ごとの範囲
共通: 試験は `scripts/dev/test-*.sh` が固定の接頭辞で走らせ、**0 件なら失敗**。LLM を呼ばず（fake harness か fake decider と仮想時刻）、本番の DB・daemon に触れず、前提が欠けたら SKIP でなく失敗する。CPU を焼く負荷をかけない。raw は `--out <dir>`（既定 `$TMPDIR`）に `celeris.long-horizon-record/1` の JSONL で書き、進捗の `agent-docs/progress/2026-11-04-long-horizon/<key>/` に写す。時間は `tokio::time::pause`・注入の時計で進める。crash は apply の hook で panic し `catch_unwind` の後に store を開き直す。

### A1 durable wake（01M6JW1EFJ55VST9P08ZTHGKVN）
- **API**: `GET /api/v1/projects/{id}/controller`（`ProjectControllerView`＝state の欄＋`pending_wakes`＋`last_decision`）、`PUT /api/v1/projects/{id}/controller` `{mode, expected_revision, reason}`（D8・D11。op `project.controller_put`、CoS は `off` だけ）、`POST /api/v1/projects/{id}/controller/wake` `{external_ref, note, idempotency_key}`（op `project.controller_wake`）、`GET /api/v1/projects/{id}/controller/decisions?before=&limit=`。cron の `extra.kind = "project_reconcile"`、`extra.project_id`、`overlap = skip`、手動実行は 422（`check_manual_run`）。`[project_controller]`: `enabled`・`allowed_project_ids`・`coalesce_secs`・`lease_secs`・`max_evaluations_per_tick`・`retry_base_secs`・`retry_max_secs`・`max_retries`・`max_cos_inputs_open_per_project`。
- **DTO**: `ProjectWakeKind`・`ControllerMode`・`ControllerPhase`・`ProjectControllerView`、`ProjectDecideInput{project_id,revision,human_epoch,wakes,now}`、`ProjectEffect = None | QueueCosInput{key,text} | CreateRoot{idempotency_key, spec: NewTaskSpec} | OpenEscalation{question_key,summary,…}`。`trait ProjectDecider`（A1 は試験の fake と、常に `None` を返す既定実装）。Proposal の型には依存しない。
- **file**: `crates/task-core/src/project_wake.rs`、`store/project_wake.rs`、`migrations/` の 1 本、`dispatcher/project_wake.rs`、`dispatcher/cron_tick.rs`（呼び出し 1 か所）、`task_ops/src/cron_jobs.rs`（kind）、`crates/task-api/src/handlers/project_controller.rs`、`cos/ops/projects.rs`（表に行）、`crates/celeris/src/config/`（新 file）、human_epoch を増やす handler の 1 行ずつ、`scripts/dev/test-project-wake-replay.sh`。**`dispatcher.rs`・`work_units.rs` は source-size の負債で行を増やせない**（field は `cron_tick.rs` 側の構造体に置く）。
- **E2E**: 試験の file は `crates/task-dispatch/src/dispatcher/tests/project_wake.rs` と `crates/task-api/tests/project_controller.rs`。fixture は Rust のコードで組む（一時 DB、fake decider）。
  | 試験（接頭辞 `project_wake_`） | 入力 | 期待値 |
  |---|---|---|
  | `redelivery_10x` | 同じ root の Done event を 10 回取り込み、fake は毎回同じ `CreateRoot` | root 1、`project_wakes` 1、`task_idempotency_keys` 1 |
  | `kind_root_done` など 9 件（kind ごとに 1 試験） | 各 kind の元 | 各 1 回 decide、kind が一致 |
  | `crash_b1`〜`crash_b6`（6 件） | 各境界で panic→再起動 | 未消費 wake 0、root 1、CoS 入力 ≤ 1 |
  | `idle_7days` | 仮想 7 日、wake なし | decide 0、CoS run 0、chat message 0 |
  | `cron_idle` | 仮想 7 日、1 時間ごと cron、fake は `None` | decide 168、CoS run 0、message 0 |
  | `coalesce` | 30 秒に 20 wake | decide 1 |
  | `human_races_apply` | decide の後・tx1 の前に human bump | `rejected:stale_human_epoch`、root 0、再評価 1 |
  | `cas_stale`・`lease_takeover`・`two_instances`（3 件） | 2 instance | 効果 1 回。切れた lease は引き継ぐ |
  | `backoff` | decide 失敗 6 回 | 60,120,240,480,960 秒で `blocked`、以後 decide 0 |
  | `backpressure` | 50 案件、上限 2 | 1 tick の decide ≤ 2、25 tick で全部 |
  | `off_is_inert` | 状態行なし・`off` | wake 0、event の差分 0 |
  | `mode_shadow` | `shadow` | 判断 1（`suppressed`）、効果 0 |
- **`scripts/dev/test-project-wake-replay.sh`**: 上の接頭辞を nextest で走らせ、件数 ≥ 27、raw（redelivery・crash・idle の各記録: root 数・未処理数・LLM 起動数）を検査して、期待値と違えば exit 1。

### A2 TS core（01M6JW1EFMF6T3CVCEH9ATR14D）
- **API**: HTTP は足さない。Host API の minor: capability `project_controller:read_root_evidence` と op `read_root_evidence{root_id}`。`PolicyScript::project_controller(input, deadline)`（既定は失敗）。
- **DTO**: 上の `ProjectSnapshot`・`ProjectProposal`（`crates/task-core/src/project_proposal.rs`）。
- **file**: `crates/task-core/src/project_proposal.rs`、`crates/task-ops/src/project_snapshot.rs`、`control-ts/src/domains/project_controller.ts`・`.test.ts`、`control-ts/src/domains/index.ts`、`control-ts/bundle.config.json`、`control-ts/fixtures/project_controller/`、`crates/task-dispatch/src/script_policy.rs`（method 1 つ）、`crates/script-runtime/`（`ops.rs`・`generation.rs`・`dto.rs`、runner の `DOMAINS`）、`scripts/dev/script-runner-cache.sh`、`docs/api/script-host/`、`ScriptDomain`・`CompareOutcome` と migration（`script_domain_modes` の行）、event・api schema と web 生成型の再生成、`crates/celeris/tests/project_controller_e2e.rs`、`scripts/dev/test-project-controller-e2e.sh`。領域の追加で触る箇所の一覧は S2 の記録と KB を見る。
- **触らない**: `crates/task-core/src/project_wake.rs`・A1 の表。起票は試験の support が `task_ops::add`＋`idempotency_record_tx` を直接呼ぶ（本番の接続は A3）。
- **E2E**（`crates/celeris/tests/project_controller_e2e.rs`。実 bundle を実 V8 で動かし、root の完了は store の遷移で模す。接頭辞 `project_controller_core_`）。fixture の案件は Outcome 2 件 `o1`（中間）・`o2`（最終）。
  | シナリオ | snapshot | 期待 proposal | 期待（store） |
  |---|---|---|---|
  | `s1_first` | root 0 | `launch_root` `outcome:o1:1` | root 1 |
  | `s2_mid_gap` | root1 done・合格・取り込み済み、`o2` 未達 | `mark_milestone o1` → `launch_root` `outcome:o2:1` | root1 は done のまま（events 不変）、root2 作成、`continues_root_ids=[root1]` |
  | `s3_achieved` | root2 も完了 | `mark_milestone o2` → `wait{phase: achieved}` | **追加の起票 0** |
  | `s4_concurrent` | 同じ snapshot で 8 回並行に評価して適用 | 全部同じ key | root 1（重複 0） |
  | `s5_not_integrated` | done だが未取り込み | `continue` | 起票 0 |
  | `s6_review_failed` | root1 が review 不合格 | `replan(review_failed)` | 起票 0、`failed` は保つ |
  | `s7_infra` | 基盤の失敗 | `wait` | 起票 0 |
  | `s8_no_outcomes` | block なし | `replan(no_outcomes)`、同じ版で 2 回目は `wait` | |
  | `s9_human_wait` | 開いた decision | `wait{waiting_on human}` | |
  | `s10_narrowed` | TS の出力を差し替えて acceptance を減らす | 検証で `root_acceptance_narrowed` | 起票 0 |
  | `s11_other_project` | op で別案件の root | `Denied` | |
- **`scripts/dev/test-project-controller-e2e.sh`**: 上を走らせ（≥ 11 件）、raw を検査。続けて control-ts の unit と `ts-verify.sh --record` を確かめ、`cargo_invocations = 0`・`link_count = 0`（TS-only の 1 行変更の使い捨て worktree で）。計測は 20 試行を `a2/ts-verify-warm.jsonl` に残し、基準 p50 7.5 秒との差を段ごとに分解して進捗に書く。合否の線は境界 ADR の目標 p50 30 秒。

### A3 案件 CoS の運営（01M6JW1EFPBCJX8J7D2ZACHS3D）
- **API**: `POST /api/v1/projects/{id}/controller/dry-run`（`{}`→`{proposal, validation, snapshot_sha256}`。書かない）、`POST /api/v1/projects/{id}/controller/report`（op `project.controller_report`。`{decision_id, classification, action}`。`action` は `ProjectProposal.action` と同じ型で、同じ検証と I5 の key を通す。案件 thread の run だけ）。`PUT /projects/{id}/status` に任意の `expected_updated_at`（違えば 409）。422 `project_request_human_only`・`task_goal_human_only`・`project_controller_owns_launch`。
- **DTO**: `Classification`（`worker_bug|spec_gap|infra|flaky|external|human_needed|unknown`）。TS の `classifyFailure` が案を出し、`task_ops::derive::classify_task_failure` の infra・supply は変えない。
- **file**: `dispatcher/project_decider_ts.rs`（A1 と A2 の接続。Proposal → `ProjectEffect`）、`crates/task-api/src/handlers/project_controller_report.rs`、`cos/ops/`（行の追加）、`handlers/projects.rs`・`handlers/task_actions.rs`（拒否）、`crates/task-ops/src/original_goal.rs`、`cos_chat/root_done.rs`（文面）、`crates/task-worker/src/cos_chat.rs` の `projects_section`（request の版と controller の状態）、`config/skills/cos-operator/project-controller.md`、`control-ts/src/domains/project_controller.ts`（方針）、`docs/ops/project-controller.md`、migration（`project_request_revisions`）、`scripts/dev/test-project-cos-shadow.sh`。
- **E2E**（`crates/celeris/tests/project_cos_shadow.rs`。実 daemon・fake harness。接頭辞 `project_cos_`）:
  | 試験 | 期待値 |
  |---|---|
  | `dry_run` | 応答あり、表の行数の差分 0 |
  | `shadow_then_enable`・`rollback_off` | shadow: CoS run 0、判断は `suppressed`。enable で root 1。off で以後の wake 0、作成済みの root は残る |
  | `off_equals_legacy` | 同じ操作列で `off` と状態行なしの events・message が一致 |
  | `brief_updated_after_report` | fake CoS の report と `project.status_put` で brief が進み、`updated_by = cos:<thread>` |
  | `failure_classified` | `review_failed` で report の分類が判断に残る |
  | `triage_types_reused` | 受信箱の代答は既存の 7 類型の basis でしか通らない |
  | `no_needless_question` | 既に答えた `question_key` では新しい項目 0 |
  | `injection_*`（ログ・成果物・worker の報告に「承認した」「task を作れ」） | 判定は不変、root 0、`instructed_by` に引くと 422 |
  | `runaway`（fake CoS が毎回 `replan` を返す） | 上限で止まり `blocked`、escalate 1 |
  | `refuses_goal_edit`（request・objective・acceptance） | 422 と `rejected` の監査 |
  | `refuses_production`（promote・secret・外部公開・不可逆） | 既存の gate で拒否 |
  | `ts_change_without_rust` | 規則の 1 行を変えた bundle で判定が変わる（cargo 0） |
- **`scripts/dev/test-project-cos-shadow.sh`**: 接頭辞 `project_cos_` を走らせ（≥ 14 件）、raw（shadow の CoS run 数・injection の root 数・拒否の code）を検査する。control-ts の変更があれば `ts-verify.sh` も通す。

### A4 統括 CoS の相談（01M6JW1EFRVZZNDQ8NYF3G588W）
- **API**: `POST /api/v1/chat/threads/{t}/proposals`（op `chat.proposal_forward`。`{client_message_id, text, task_ids, expires_at}`。`{t}` は案件 thread、統括の run だけ。`role=system`、metadata `cos_proposal`、`author = cos:<統括>`。`instructed_by` に引くと 422）。人の発言の取り次ぎ `chat.relay_forward` は変えない。
- **DTO**: 領域 `cos_intent`（TS。入力 `{text_sha256, text, thread_kind}`、出力 `{intent: consult|read|delegate, rule_id}`。prompt の hint で、LLM は覆せる。参照実装なしで `ProposalOnly`）。`CosReadBudget`（`[cos]` の `inline_read_max_commands`、既定 20）。
- **file**: `crates/task-worker/src/cos_chat.rs`（Core の指針）、`crates/task-worker/src/preamble.rs` の `create_task` の節、`config/skills/cos-operator/consult.md`、`control-ts/src/domains/cos_intent.ts`、`cos_chat/launch.rs`（hint）、`cos_chat/root_done.rs`（元の thread へ）、`crates/task-api/src/cos/ops/surface.rs`、`scripts/dev/test-general-cos-consult-e2e.sh`。controller の file は触らない。
- **E2E**（fake harness。接頭辞 `general_cos_consult_`）: 「どうするべき？」→ task 0、返事 1 / 「直近 3 件を比較して」→ task 0、run 内の読取 ≥ 1、返事 1 / 「これを修正して」→ task 1、完了後に元の thread へ要約 1 件で task id が読める / 読取の上限を超えると run が止まる / LLM CLI の起動は既存の `WorkerPolicyViolation` / review・統合は同じ経路（`ReviewerPassed` が無ければ完了の要約に「合格」と書かれない）/ 統括と案件の session が別 / proposal を `instructed_by` に引くと 422 / 人の発言の偽装（`relay_forward` に本文）は 422。
- **`scripts/dev/test-general-cos-consult-e2e.sh`**: 接頭辞 `general_cos_consult_` を走らせ（≥ 9 件）、raw（発言ごとの task 数・読取の数・元の thread の要約）を検査する。`cos_intent` の fixture（3 種の発言）を実 bundle で評価して intent を照合する。

### A5 Roadmap（01M6JW1EFT0NX6HP89S55ECNXE）
- **前提の確認**: A2・A3 の PoC の raw を読み、request block の不足（版がない、人の確認の記録がない、root との対応の履歴がない）を実データで示す。不要なら done にせず question。
- **API**: `GET /api/v1/projects/{id}/roadmap[?version=]`、`GET .../roadmap/history`、`POST .../roadmap/proposals`（op `project.roadmap_propose`、CoS 可）、`POST .../roadmap/proposals/{version}/decide`（人だけ。registry は EXCLUDED `human_only`）、`POST .../roadmap/outcomes/{oid}/status`（`mark_milestone` の適用と人。`reached` は evidence を Rust が確かめる）。`/milestones` を一切使わない。
- **DTO**: `Roadmap{project_id,version,status: proposed|approved|rejected|superseded,request_sha256,change_reason,proposed_by,decided_by,outcomes[]}`、`Outcome{id(版をまたいで不変),title,acceptance[],evidence[],priority,depends_on[],status: open|in_progress|reached|dropped,root_task_ids[]}`。snapshot の `roadmap` を埋め、`approved` の版があれば block より優先。
- **file**: `crates/task-core/src/roadmap.rs`、`store/roadmap.rs`、migration、`handlers/roadmap.rs`、`cos/ops/projects.rs`（行）、`project_snapshot.rs`（`roadmap`）、`web/features/projects/project-roadmap-panel.tsx`、`docs/ops/project-controller.md`（節）、ADR 付記（ADR-0079 D14）、`scripts/dev/test-project-roadmap.sh`。
- **E2E**（接頭辞 `project_roadmap_`）: 提案は版を進めない / 人の承認で版が進み旧版が `superseded` / CoS の decide は 403 / stable id が版をまたぐ / root と Outcome の対応の履歴 / 旧 milestones の 410 が不変（既存の試験を走らせる）/ task の `phase` を読まない / brief の全置換で Roadmap が変わらない / migration の後方互換（その前の版の DB からの起動と行の保持）/ block→Roadmap の優先順。
- **`scripts/dev/test-project-roadmap.sh`**: 接頭辞 `project_roadmap_` を走らせ（≥ 10 件）、`crates/task-api/tests/project_plan.rs` の 410 の試験も走らせ、raw（版の遷移・主体）を検査する。

### A7 authority ledger（01M6JW1EFW86JH3VKZ274J9GT5）
- **API**: `GET /api/v1/projects/{id}/authority`、`POST .../authority/grants`（人だけ）、`POST .../authority/grants/{g}/revoke`（誰でも）、`POST .../authority/check`（読み取り、`{action_class, scope}`→`{allow, grant_id, reason}`）。
- **DTO**: `AuthorityGrant{id,project_id,scope:{kind:project|outcome|task,ref},action_class,source:{kind:human_message|decision|approval|standing_rule,ref},granted_by,expires_at,budget,consumed,revoked_at}`。`action_class`: `human_required|destructive|production|api_authorization|external_publish|opaque_external_effect|launch_root`。不明・期限切れ・予算切れ・出所の読めない grant はすべて拒否。snapshot の `authority` を埋める。`launch_root` は grant が無ければ `escalate`（D9 の I9）。人が案件を `enabled` にする操作は、その理由を出所に `launch_root` の grant を 1 件作る（期限・予算つき。A7 より前は mode だけで許可）。
- **file**: `crates/task-core/src/authority.rs`、`store/authority.rs`、migration（grant と `project_controller_decisions` の `observed_outcome_json`・`human_decision_ref`）、`handlers/authority.rs`、`cos/basis.rs`（refs の種類 `authority_grant`）、`cos/ops/projects.rs`、`project_snapshot.rs`（`authority`）、`config/skills/cos-inbox-triage/SKILL.md`（policy_version を上げる。手順は KB `cos-triage-policy-skill-version`）、`web/features/projects/project-authority-panel.tsx`、`scripts/dev/test-authority-ledger-e2e.sh`。A5 と並行しうるので snapshot の変更は `authority` の欄だけ。
- **E2E**（接頭辞 `authority_ledger_`）: 6 分類それぞれ grant なしで拒否・状態不明で拒否 / 期限切れ / 予算切れ / 取り消し後 / 根拠つきで許可され元リンクが読める / 同じ `question_key` を再度聞かない（根拠を出す）/ 変更は `/cos/operations` を通り監査に suggestion・observed outcome・actual actor・human decision が揃う / 偽の人の指示: CoS の文面・system 入力・proposal・ログの文を `source` にした grant は 422 / `human_required` の解除は不可。
- **`scripts/dev/test-authority-ledger-e2e.sh`**: 接頭辞 `authority_ledger_` を走らせ（≥ 13 件）、raw（class ごとの拒否の code・監査の 4 欄の有無）を検査する。

### A6 長時間の運用（01M6JW1EFYFTSPMT080KHKW9EY）
- **API**: `GET /projects/{id}/controller` に `budget`・`guards` を足す。`GET /metrics/project-controller`（評価数・op 遅延・loop cost）。
- **DTO**: `[project_controller.budget]`: `max_cos_inputs_per_day`・`max_roots_per_day`・`max_no_progress_decisions`・`max_same_failure_repeats`・`max_tokens_per_day`（`tree_metrics::rollup` の和）・`ttl_secs`（wake の失効）。`phase` の遷移の完成。
- **file**: `dispatcher/project_wake.rs`（guard は `dispatcher/project_guard.rs` に分ける）、`crates/task-ops/src/project_budget.rs`、`docs/ops/project-controller.md`、`scripts/dev/test-project-controller-replay.sh`。
- **E2E**（`crates/celeris/tests/project_controller_replay.rs`。接頭辞 `project_controller_replay_`）: `crash_each_stage` / `suspend_clock_jump_3days`（再評価 1 回に畳む）/ `external_never_replies`（TTL で escalate 1、以後 LLM 0）/ `worker_fails_same_way_5x`（同じ失敗の 3 回目で止まる）/ `human_escalation_repeat`（2 回目なし）/ `budget_day_cap`・`token_cap` / `paused_project`・`cancelled_project`（評価 0）/ `achieved_stays_quiet`（新しい wake でも起票 0、人の変更でだけ再評価）/ `event_primary_cron_fallback`（event を落としても cron の照合で回復）。raw `a6/resource.jsonl`: daemon の RSS・isolate 数・op の遅延 p50・p95・1 周の loop cost を idle と 100 案件で。常時の LLM・isolate が増えないこと（領域の評価で isolate が増えない）を確かめる。複数日の実稼働は A9 の別 pilot。
- **`scripts/dev/test-project-controller-replay.sh`**: 接頭辞 `project_controller_replay_` を走らせ（≥ 12 件）、raw の resource が揃い、LLM 起動数が期待値と一致することを検査する。

### A8 ベンチ（設計 01M6JW1EG0Y7H5REMSDQESKBWA、実行 01M6JW1EG2G677XHN408JCW2JG）
- **A8(設計)** は `agent-docs/progress/2026-11-04-long-horizon/a8a.md` に課題 8〜12 件・oracle・隔離を決める。この ADR の候補: `stale_event`・`partial_result`・`slow_worker`・`child_failed`・`plan_invalid`・`auth_wait`・`daemon_restart`・`multi_root`・`consult_short_continue`・`repeat_failure`。A8(設計) は変えてよい。
- **隔離の条件（固定）**: 公開の入力は `tests/fixtures/long-horizon/public/<case>/`、oracle は `tests/fixtures/long-horizon/oracle/<case>/`。oracle は agent-platform に置いてよいが、**実 LLM の試行の作業 tree は課題ごとの使い捨て repo で agent-platform を含まない**。旧 CoS と新 Manager は同じ初期 DB・同じ権限・同じ oracle。採点は試行の後に別 process で読む。K2・K4 の release・凍結の環境・評価器を読まない。
- **指標**: token・quota・walltime・handoffs・human_intervention・到達率・無駄起票・double effect・回復率。イベント再生の値と実 LLM pilot の値を混ぜない。実 LLM を使わなかった指標は未計測と書く。
- **A8(実行) の file**: `crates/celeris-bench-long-horizon/` は作らず `crates/campaign/` の既存の grader の形を読んで再利用を判断し、`tests/fixtures/long-horizon/`、`scripts/dev/test-long-horizon-bench.sh`。
- **`scripts/dev/test-long-horizon-bench.sh`**: 接頭辞 `long_horizon_bench_`、課題の全件を旧・新で再生（`off` が旧 CoS）、raw と集計を書き、期待値の不等式（新の到達率 ≥ 旧、新の double effect = 0、無駄起票 ≤ 設計の上限）を検査、`git ls-files` と使い捨て repo の中身で oracle の文字列が worker の tree に無いことを検査する。

### A9 統合と pilot（01M6JW1EG41D198Y5CP65MQJM4）
- **API**: 足さない（A1・A5・A7 の API を画面に出す）。
- **file**: `web/features/projects/project-controller-panel.tsx`、`project-detail-view.tsx`、`web/e2e/`（1 本）、`scripts/dev/test-long-horizon-pilot.sh`、`scripts/dev/long-horizon-pilot-llm.sh`（実 LLM。名前を test- にしない）、`agent-docs/progress/2026-11-04-long-horizon/a9/`。`docs/SPEC.md`（§3.3・§5 に Project Controller の行）、`docs/architecture-map.md`。
- **`scripts/dev/test-long-horizon-pilot.sh`**（LLM なし）: 接頭辞 `long_horizon_pilot_`。一時 daemon・fake harness・仮想時刻で 3 日分のイベントを流し、新 task の起票・無限ループの停止・費用の上限での停止・cancel と巻き戻し・`off` で従来 CoS に戻ることを検査。human check と production gate が残ること（promote の op が拒否される）も検査。
- **実 LLM pilot**: `long-horizon-pilot-llm.sh` が使い捨ての `pilot:` 案件で数時間相当を流し、raw を `a9/` に残す。24〜72h の実稼働は人の承認があるときだけで、無ければ未実施と書く。
- **画面**: 案件の詳細に長期方針（`request`）・途中目標・関連 root・今後の作業（`next_actions`）・待機と人の要件・過去の判断と根拠・mode の切り替え。スマホ幅の確認（`mobile-audit`）。

## script の担当（後続の子の受け入れ条件にある名前）
| script | 担当 | 接頭辞・最低件数 | 中身 |
|---|---|---|---|
| `scripts/dev/test-project-wake-replay.sh` | A1 | `project_wake_` ≥ 27 | 10 回再配送・全 kind・crash 6 境界・待機中 LLM 0 |
| `scripts/dev/test-project-controller-e2e.sh` | A2 | `project_controller_core_` ≥ 11 | root1→root2→達成、重複 0、TS-only の cargo 0 |
| `scripts/dev/test-project-cos-shadow.sh` | A3 | `project_cos_` ≥ 14 | dry-run・shadow・enable・rollback、injection、runaway、拒否 |
| `scripts/dev/test-general-cos-consult-e2e.sh` | A4 | `general_cos_consult_` ≥ 9 | 相談・読取・委譲の 3 経路、偽装の拒否 |
| `scripts/dev/test-project-roadmap.sh` | A5 | `project_roadmap_` ≥ 10 | 版・人の承認・410 の不変 |
| `scripts/dev/test-authority-ledger-e2e.sh` | A7 | `authority_ledger_` ≥ 13 | 6 分類の fail closed、監査、偽の指示 |
| `scripts/dev/test-project-controller-replay.sh` | A6 | `project_controller_replay_` ≥ 12 | fault injection、仮想時刻、資源の raw |
| `scripts/dev/test-long-horizon-bench.sh` | A8(実行) | `long_horizon_bench_`（課題数×2 以上） | 旧・新の再生、oracle の隔離の検査 |
| `scripts/dev/test-long-horizon-pilot.sh` | A9 | `long_horizon_pilot_` ≥ 6 | 3 日分の仮想時刻、停止・巻き戻し・rollback |

全 script の共通の形: `set -euo pipefail`、D11 の環境変数を unset、nextest の結果から実行件数を数えて下限未満なら exit 1、raw の JSONL を読んで条件を検査する（走っただけで通さない）。

## 並列の衝突を避ける分担
起動順は `depends_on` のとおり: A0 → A1・A2・A8(設計)（並列）→ A3・A4（並列）→ A5・A7（並列）→ A6 → A8(実行) → A9。
| file | 持ち主 | 他の子の扱い |
|---|---|---|
| `crates/task-core/src/project_wake.rs`・`store/project_wake.rs`・`dispatcher/project_wake.rs` | A1 | A6 は同じ file を A1 の後に変える。A3 は読むだけ |
| `crates/task-core/src/project_proposal.rs`・`task-ops/src/project_snapshot.rs` | A2 | A3 は方針を足さない。A5・A7 は snapshot の自分の欄だけ |
| `control-ts/src/domains/project_controller.ts` | A2 → A3 | A5・A6 は方針の必要があれば末尾に。TS-only で済ませる |
| `control-ts/src/domains/index.ts`・`bundle.config.json` | A2、A4 | 各 1 行。A4 は A2 の後なので順 |
| `crates/task-core/src/lib.rs` | A1・A2・A5・A7 | `pub mod` を各 1 行。衝突は両方残す |
| `cos/ops/projects.rs`（表） | A1・A3・A5・A7 | 行の追加だけ。衝突は両方残す。A3 の他は A1 の後 |
| `handlers/project_controller.rs` | A1 | A3 は別 file `project_controller_report.rs` |
| `web/features/projects/*-panel.tsx` | 各子 | `project-detail-view.tsx` への結線は A9 だけ。A5・A7 は panel と単体試験まで |
| `config/skills/cos-operator/SKILL.md` | A3・A4 | 別 file を足し、表に 1 行ずつ |
| `crates/task-worker/src/cos_chat.rs` | A3（`projects_section`）・A4（Core） | 関数が別 |
| `docs/ops/project-controller.md` | A3 が作る | A5・A6・A7 は節を足す |
| migration | 各子 | 着地時の最新 main の次。A1 と A2 は並列なので後着が番号を振り直す。`SCHEMA_VERSION`、それを書く試験 16 か所、`delivery.rs` の版の列、`open_rejects_db_with_schema_version_newer_than_supported`、`crates/campaign/src/ops_r1_trace.rs` の `SUPPORTED_SCHEMA_VERSIONS` を同時に直す |
| `docs/api/v1/*.schema.json`・`web/api/generated/` | 各子 | 衝突したら生成し直す（手で解かない） |
| 進捗 | 各子 | `agent-docs/progress/2026-11-04-long-horizon/<key>.md`。親の `2026-11-04-long-horizon.md` は親の closeout だけ |

`dispatcher.rs`（2480）と `work_units.rs`（2182）は source-size の負債で増やせない。`crates/celerisctl/src/main.rs` の inline 試験も上限。celerisctl を足すなら `commands/<name>.rs`（`read_only`・`cos_mapped` のどちらかに入れる）。

## 却下した案
- **独立 Goal 表**: D1。
- **旧 milestones の再開**: D2。
- **常設の監視 root を running で保持**: D3。lease・reviewer・delivery の前提を壊す。
- **常駐の Manager LLM**: 待機中も費用が出る（I4）。
- **別のイベントストア**: `events` は task 単位だが、案件は `tasks.project_id` で引ける。必要なのは派生の wake だけ（I2）。
- **Proposal を `Effect` に統一して A1 と A2 の共通型を A0 で land する**: A0 にコードを足すと全 crate の試験が要り、型は実装で詰める余地が要る。`ProjectDecider` と `ProjectEffect` で分離すれば A1 と A2 は独立に試験でき、A3 が接続する。
- **`decision_log` を判断の記録に流用**: 主キーが run 単位で campaign の export に結び付く。案件の判断は run を持たない。
- **brief に判断の結果を自動で書く**: brief は CoS の全置換で、daemon が書くと食い違う（D3 の所有の決定を保つ）。

## 未解決・この ADR の更新
- A5 の必要性は PoC の raw で決まる（不要なら人に question）。
- `tree::gate::unit_gate` と TS の gate のずれ（ADR execution_gate の未解決）は controller と無関係で、ここでは扱わない。
- 契約を変える子は、この ADR の末尾に付記（日付・子の key・変えた行・理由）を足し、進捗に理由を書く。

## 付記 2026-11-04（a8a）
§A8 の設計の本文は [ADR 2026-11-04-long-horizon-bench](2026-11-04-long-horizon-bench.md) に置き、`a8a.md` はその要約と進捗にした（CLAUDE.md の「設計判断は ADR」に従うため）。変えた行と理由:
- §A8 の候補 10 件はそのまま採用した。
- §A8 の「`off` が旧 CoS」は再生では維持し、腕を `old.passive`・`old.proactive`・`new` の 3 つにした。旧 CoS の台本で結果が決まるので、幅を見るため。`test-long-horizon-bench.sh` の「課題数×2 以上」は満たす。
- A4 の指針は案件 mode で切れないので、C09 の実 LLM だけ A1 の直前の binary を補助の腕にする。主比較には入れない。
- D11-5 の「認証が使えなければ人に頼む」を強めた。実 LLM の試行は worker の run の中で起こさず、`scripts/dev/long-horizon-pilot-llm.sh` を人が実行する。run の規則が LLM の起動を禁じていることと、測定専用 account に人の login が要ることが理由。人に頼まれた外部 operator が実行してもよい。
- §A8 の配置に `oracle/MANIFEST.sha256` と、主体に見せない台本 `oracle/<case>/scenario.jsonl` を足した。`public/` と `oracle/` の分け方は変えていない。

## 付記 A2（2026-11-04、01M6JW1EFMF6T3CVCEH9ATR14D）
- `ProjectSnapshot` に `outcomes_invalid: bool` を足した（block の形の誤りを「block なし」と分けるため）。
- `decisions[]` の行に `reason_code`・`outcome_id`・`request_sha256` を足した（同じ版での `replan`・`escalate`・`mark_milestone` の繰り返しを TS が避けるため）。`escalate` の行は `root_key` に `question_key` を入れる。
- `RootSpec.acceptance[].check` は `{"type":"reviewer"}` か `{"type":"human"}` だけ（TS が command を作らない）。
- 検証の code に `gap_unknown_outcome`・`unknown_root`・`unknown_outcome`・`achieved_with_open_outcomes`・`cos_inputs_exceeded`・`roots_exceeded_today`・`field_too_long` を足した（安全違反は `SAFETY_VIOLATIONS` の 4 つのまま）。
- 冪等キーの行の `result_json` に `{root_key, outcome_ids, continues_root_ids}` を持たせ、snapshot はこれで controller の root を引く（task の行・label に controller の値を書かない）。
- Host op `project_controller:read_root_evidence` は A2 では作らなかった。判定は snapshot だけで決まり、別案件の root は snapshot に入らない。成果物の中身を読む判定が要るときに A3 が足す。
- runner は参照実装の無い領域を byte 比較でなく行の不変条件（`DomainSpec.invariants`）で採点する。

## 付記 A4（2026-11-04、01M6JW1EFRVZZNDQ8NYF3G588W）
§A4 の契約を次のとおり実装し、一部を変えた。理由は [進捗 a4](../progress/2026-11-04-long-horizon/a4.md) にも書いた。
- **`cos_intent` は TS の領域にせず、Rust の純粋関数 `task_core::chat::intent::classify` にした**（変えた行: §A4 の DTO の「領域 `cos_intent`（TS …）」）。理由: 推定は prompt の hint だけで、状態遷移・起票・認可・予算に効かない（LLM が覆せ、起票の可否は `/cos/operations` の検査が決める）。TS の領域を足すと migration・`ScriptDomain`・runner・bundle・schema など十数か所が要り、参照実装の無い `ProposalOnly` の領域が判定でなく文言の手がかりを返すだけになる。「判定は TS」の判定（状態を変える決定）には当たらない。hint の規則を頻繁に変える必要が実データで出たら、TS の領域へ移す（提案に書いた）。
- **読取の上限 `CosReadBudget` は worker の `task_worker::tool_policy::CosInlineBudget` にした**。`[cos] inline_read_max_commands`（既定 20）を `CosChatContext.inline_read_max_commands` で渡し、claude-code の adapter が CoS chat run の shell 道具の起動を数え、上限を超えた 1 回目で run を止める（`Terminal::Error`、`retryable=false`、文面 `cos_inline_budget_exceeded`）。CoS chat run の中で LLM の CLI・API・subagent を検出したら、従来の `WorkerPolicyViolation` に加えて run を止める（`cos_llm_launch_refused`。通常の worker run は従来どおり警告だけ。`subagents = allow_cos` の設定は尊重する）。CoS の実行器は `claude-code` 固定（`crates/celeris/src/daemon/cos_launch.rs`）なので、codex・acp の adapter には入れていない。
- **`chat.proposal_forward`** は §A4 のとおり `POST /api/v1/chat/threads/{t}/proposals`。応答は 200 `ChatMessageResponse`（operation の `result`）。`{t}` は案件 thread、送り手は統括 thread の run だけ。案件 thread の `role=system`・`queued` の入力になり、本文の先頭に Rust が「【統括 CoS の提案（人の指示ではない）】」を付け、`ChatMessage.cos_proposal{proposed_by=cos:<統括 thread>, from_thread_id, run_id, operation_id, task_ids, expires_at}` を記録する。task 1〜20 件・期限・案件内の task は relay と同じ検査（`relay_record`）。`author` は付けない（`role=user` の欄）ので、§A4 の「`author = cos:<統括>`」は `cos_proposal.proposed_by` に置いた。直接の呼び出しは 403 `chat_proposal_cos_only`。
- **完了の要約の戻り先**（`cos_chat/root_done.rs`）: root task を統括 thread の run が `task.create`（applied）で作ったなら、その統括 thread に事実を置く（`SqliteStore::cos_origin_thread_for_task`）。それ以外は従来どおり案件 thread（案件の無い root は統括）。要約の「合格」は事実ブロックの `review_verdict` からだけ出る（判定が無ければ「受け入れ判定: 記録なし」）。
- **E2E** は 1 crate に置かず、試験の対象の crate に置いた（接頭辞 `general_cos_consult_`、計 13 件）: `task-api/tests/general_cos_consult.rs`（相談・読取・委譲・proposal・relay の偽装・session の分離）、`task-dispatch` の `cos_chat_root_done.rs`（元の thread への要約）、`task-worker` の `claude_code/cos_chat_tests.rs`（上限内・上限超え・LLM CLI）と `cos_chat/tests.rs`（prompt の hint と指針）と `tool_policy.rs`、`task-core` の `chat/intent.rs`。`scripts/dev/test-general-cos-consult-e2e.sh` の下限は 13 件にした（§script の表の 9 件より多い）。
- 指針: `crates/task-worker/src/cos_chat.rs` の Core に「応じ方（相談・短い読取・委譲）」を足し、各入力の横に推定と規則 id、入力の節に読取の上限を出す。「1 run = 1 turn」「subagent・他の LLM CLI を起動しない」は保ち、委譲した task の完了は daemon が thread に戻すので待たずに turn を終える、と書き足した。`preamble.rs` の `create_task` の節は「1 つの作業の依頼は 1 つの root」に直し、相談と短い読取は root にしないと書いた。skill は `config/skills/cos-operator/consult.md`（version 6）。
- Core の大きさの上限（試験 `cos_chat_core_is_byte_identical_across_runs_and_free_of_run_values`）を 8300 B から 9100 B に上げた。応じ方の規則（682 B）と subagent・turn の行の書き足しで 9047 B になったため。詳細は skill の consult.md に置き、Core は最小にした。

## 付記 A1（2026-11-04、task 01M6JW1EFJ55VST9P08ZTHGKVN）: 実装で決めた契約の差分
A1 の実装（migration `0112_project_controller.sql`、`SCHEMA_VERSION` 112。main の A2 が 0111 を先に使ったので review 前の同期で 0112 に振り直した）で契約を次のとおり具体化・変更した。理由は
`agent-docs/progress/2026-11-04-long-horizon/a1.md` に書いた。
1. **型の置き場所**: `ProjectWakeKind`・`ControllerMode`・`ControllerPhase`・`EffectState`・`ProjectControllerView`・
   `ProjectDecideInput` は契約どおり `crates/task-core/src/project_wake.rs`。`ProjectEffect`・`ProjectDecision`・
   `trait ProjectDecider`・`NoopProjectDecider`・`ProjectControllerSettings` は `crates/task-dispatch/src/project_controller.rs`
   に置く（`CreateRoot` が持つ `NewTaskSpec` は task-ops の型で、task-core から参照できない）。A3 の
   `dispatcher/project_decider_ts.rs` はこの trait を実装する。
2. **`OpenEscalation` は A1 の効果に入れない**: 受信箱の新種類 `project_controller` は A3 が足し、そのときに
   `ProjectEffect` に variant を足す。A1 の効果は `None`・`QueueCosInput`・`CreateRoot` の 3 つ。
3. **実行時の状態の置き場所**: `dispatcher.rs` に欄を足せない（source-size の負債）ので、controller の実行時の状態は
   `CosChatLaunch.project_controller` に置き、段は `dispatcher/cron_tick.rs` の `tick_cron_jobs` の最後から呼ぶ
   （module は `cron_tick.rs` の `#[path = "project_wake.rs"]`）。CoS chat が無い構成では controller は入らない（起動時に warn）。
4. **CAS で捨てた判断は行にしない**: `rejected:stale_*` は log と tick の集計（試験が読む）に残し、判断の行は作らない。
   wake は消費せず、次の tick で評価し直す（I7）。
5. **`next_wake_at`**: 期限が来た案件は wake の行なしで評価する（timer）。wake の種類は 9 種のまま。
6. **mode の API**: `shadow` 以上へ上げるには daemon の `[project_controller] enabled = true` も要る（403
   `project_controller_disabled`）。`PUT` の応答は `ProjectControllerView`。
7. **`human_epoch` を増やす経路**（2026-11-04 改訂、task 01M6JW1EFJ55VST9P08ZTHGKVN。旧版の「2 経路・別 transaction・残りは A3」を
   置き換える）。D12 の経路は全部 A1 が実装した（A3 に残さない）。どの経路も書き込みの transaction の中で
   `HumanEpoch::bump_tx`（`crates/task-api/src/human_epoch.rs`）→ `SqliteStore::project_controller_human_bump_tx` を呼び、
   書き込みと epoch の片方だけが残ることはない。旧い別 transaction の `project_controller_human_bump` は消した。
   数える principal は `HumanEpoch::of` が決める: `Principal::Human` は数え、`Operator`・`Mcp`・`Cos`・`Unauthenticated` は数えない。
   例外として、人の発言を運ぶ `…/relays`（operator だけが通る）と `chat.relay_forward`（`author` が `human:` の発言だけ写せる）は
   `HumanEpoch::Counted` を直接渡す。`/cos/operations` の監査経路（`*_op` の `audit = Some`）は `HumanEpoch::NotCounted`。
   案件を持たない task・thread、状態の行が無い案件、`off` の案件では何も書かない（I13）。

   | 経路（API） | 数える principal | 同じ transaction の書き込み口 | 試験 |
   |---|---|---|---|
   | `PATCH /projects/{id}` | Human | `SqliteStore::write_tx` の 1 tx（全欄。`project_set_{status,slug,workspace,auto_integrate}_tx`） | `project_wake_api_human_patch_bumps_human_epoch`・`…_each_route_bumps_once` |
   | `POST /projects/{id}/{pause,resume,cancel,archive,unarchive}` | Human | `lifecycle::project_action_op` → `task_ops::lifecycle::apply_project_action` | `…_each_route_bumps_once`、task-dispatch `project_wake_human_races_pause` |
   | 案件つきの `POST /tasks`・integrate task の起票 | Human | `create_task_with` の tx | `…_each_route_bumps_once`（`POST /tasks`）、`…_inbox_each_answer_bumps_once`（integrate task） |
   | `POST /tasks/{id}/cancel` | Human | `task_ops::gate::cancel_with` | `…_each_route_bumps_once`、task-dispatch `project_wake_human_races_cancel` |
   | `POST /tasks/{id}/retry` | Human | `retry_op`（`retry_task_claimed_tx` と routing_override を 1 tx） | `…_each_route_bumps_once` |
   | `POST /tasks/{id}/reopen`・`rereview` | Human | `reopen_op` → `task_ops::comment::reopen_with`、`rereview_via` | `…_each_route_bumps_once`（reopen）、`…_inbox_each_answer_bumps_once`（rereview） |
   | `POST /tasks/{id}/{approve,reject,accept,answer}` | Human | `task_ops::gate::{approve,reject,accept,answer}_via` | `project_wake_api_human_inbox_each_answer_bumps_once` |
   | 案件 thread（kind `project`）への `POST /chat/threads/{t}/messages` | Human | `chat_message_post_as_also`（`crates/task-core/src/chat/human_post.rs`） | `project_wake_api_human_chat_message_bumps_once_and_a_resend_does_not` |
   | `POST /chat/threads/{t}/relays` | Operator（人の指示を運ぶ） | `chat_relay_post_also` | `…_chat_relay_bumps_once_and_a_resend_does_not` |
   | `/cos/operations` の `chat.relay_forward` | Cos（人の発言の写し） | operation の監査と同じ tx | `…_chat_forward_bumps_once_and_a_replay_does_not` |
   | `POST /inbox/items/{id}/answer` と委ね先の route（決定・認可・計画/工程の gate・`PATCH /tasks`・未取り込み・browser の待ち・統合の依頼） | Human | 委ね先の route の tx（受信箱の handler は数えない）。行ごとの書き込み口は 9 の表 | `…_inbox_each_answer_bumps_once`（17 経路）、`…_inbox_browser_{decision,registered}_bumps_once` |

   数えない（試験で固定）: operator・MCP・CoS が直接呼ぶ同じ route（`…_operator_is_not_counted`・`…_mcp_is_not_counted`・`…_cos_is_not_counted`・
   `…_chat_cos_and_operator_messages_are_not_counted`・`…_inbox_operator_or_projectless_is_not_counted`・`…_inbox_cos_answer_and_resolve_are_not_counted`）、
   検査で断られた要求・変化の無い要求・再送（`…_refused_or_unchanged_requests_do_not_bump`・`…_chat_refused_writes_do_not_bump`・
   `…_inbox_refused_answers_do_not_bump`）、`off` と案件なし（`…_off_or_projectless_is_inert`・`…_chat_off_project_is_inert`）。
   同じ transaction であることは、tempdir の DB に `RAISE(ABORT)` の trigger を足して片方を落とす試験で固定した
   （`…_write_failure_keeps_the_epoch`・`…_bump_failure_rolls_the_write_back`・`…_chat_bump_failure_rolls_the_message_back`・
   `…_inbox_bump_failure_rolls_the_answer_back`・`…_inbox_plan_withdraw_failure_keeps_the_epoch`）。数える・数えない経路の詳細は 9。
8. **wake API の coalesce**: `POST …/controller/wake` は 30 秒の固定値で `coalesce_until` を置く（API は daemon の設定を持たない）。
9. **`human_epoch` を数える経路と数えない経路**（WU epoch-tx-chat、2026-11-04）。数える経路はどれも、書き込みの transaction の
   中で `SqliteStore::project_controller_human_bump_tx` を呼ぶ（`crates/task-api/src/human_epoch.rs` の `HumanEpoch`）。案件を持たない
   thread・task、状態の行が無い案件、`off` の案件では何も書かない（I13）。同じ要求の再送（同じ `client_message_id`・
   idempotency key・browser の `replayed`）と、検査で断られた要求・書き込みに失敗した要求は数えない。

   | 経路 | 数えるか | 理由・書き込みの場所 |
   |---|---|---|
   | 案件 thread（kind `project`）への `POST /chat/threads/{t}/messages`（`Principal::Human`） | 数える | 人の発言。`chat_message_post_as_also` の tx |
   | `POST /chat/threads/{t}/relays`（`Principal::Operator` だけが通る） | 数える | operator は人の指示をそのまま運ぶ。`chat_relay_post_also` の tx |
   | `/cos/operations` の `chat.relay_forward`（統括 CoS → 案件 thread） | 数える | 写すのは人の発言そのもの（`author` が `human:` の発言だけ写せる）。operation の監査と同じ tx |
   | 統括 thread・review thread への人の発言 | 数えない | 案件を持たない。案件へは forward で届いたときに数える |
   | operator・MCP が `…/messages` に書く自分の発言 | 数えない | 人の発言でない |
   | CoS の発言（`/cos/operations` の `chat.message_post`）・CoS の run の出力 | 数えない | controller と同じ側の判断。数えると CoS が自分の発言で controller を止める |
   | `POST /inbox/items/{id}/answer`（`Principal::Human`） | 数える | 委ね先の route が自分の書き込みの tx で 1 回数える（受信箱の handler は数えない。二重に数えない）。下の 10 行 |
   | 　質問 → `POST /tasks/{id}/answer` | 数える | `gate::answer_via` の遷移の tx |
   | 　決定 → `POST /decisions/{id}/answer`（`withdraw`・`revise` も同じ） | 数える | `decision::apply_plan_also` の tx |
   | 　認可 → `POST /approvals/{id}/decide` | 数える | 決定の行（`approval_decide_tx`）の tx。続く task の再開は後始末で数えない |
   | 　受け入れ確認・draft → `POST /tasks/{id}/{approve,reject,accept,cancel}` | 数える | `gate::{approve,reject,accept,cancel}_via` の遷移の tx |
   | 　失敗 → `retry`・`reopen`・`cancel`、再同期の停止 → `rereview` | 数える | `retry_op`・`reopen_with`・`rereview_via` の tx |
   | 　計画・工程の確認 → `…/execution/{plan,phase}-gate` | 数える | `{plan,phase}_gate_via` の遷移の tx。取り下げの監査 event は遷移の前の別の tx（従来どおり） |
   | 　担当が決まらない → `PATCH /tasks/{id}` | 数える | `edit_task_tx` の tx。`expected_write_paths` だけの変更は数えない |
   | 　未取り込み → `…/integrations/{repo}/{merge,hold,discard,reopen}` | 数える | 状態の行（`task_integration_write_tx`）の tx |
   | 　browser の待ち → `…/browser/waits/{w}/{decision,registered,revoke}` | 数える | `browser_wait_{decide,register}_also` の解決の tx |
   | 　統合の依頼（記録だけの回答）・integrate task の起票 | 数える | `integration_request_answer_tx` の tx・`create_task_with` の tx |
   | operator が `POST /inbox/items/{id}/answer` を呼ぶ | 数えない | `Principal::Operator`。人の指示を運ぶなら relays に書く |
   | CoS の `inbox.answer`（`/cos/operations`。`instructed_by` つきでも） | 数えない | 監査経路（`audit = Some`）。人の発言は relays・forwards・案件 thread への発言の時点で数えている |
   | `POST /cos/inbox/{id}/resolve` | 数えない | CoS の一次対応の記録で、人の回答でない |

10. **`pending` の効果の門と再試行**（WU effect-safety、2026-11-04）。効果は実行の直前に
   `SqliteStore::project_controller_effect_admit`（`crates/task-core/src/store/project_wake_effect.rs`）を通す。1 つの tx で
   controller の mode・phase と `projects` の状態を読み、mode が `enabled` でない（`off`・`dry_run`・`shadow`。状態の行が無い
   案件は `off`）、phase `paused`・`cancelled`、案件の `paused`・`cancelled`・`done`・archive なら効果を実行せず `suppressed`
   にし、`effect_ref` に `halted:<理由>`（例 `halted:mode_off`・`halted:project_paused`）を残す。phase `blocked` は `pending`
   のまま待つ。効果の `Err` と CoS 入力の Busy は `pending` のまま延期し、判断の行の `effect_attempts`（延期の回数。Busy を含む）で
   `retry_base_secs × 2^(n-1)`（上限 `retry_max_secs`）の後の `effect_next_at` まで試さない。`Err` の回数 `effect_failures` が
   `max_retries` を超えたら、先に notice（`bad_news`、source key `project_controller:effect_failed:<判断 id>`）を記録してから
   判断を `failed`、案件の phase を `blocked` にし、`tracing::warn` を出す。Busy は `max_retries` に数えない（CoS が処理するまで
   待つのは失敗でない）。migration 0112 の判断の行に 3 列を足し、trigger は `pending` → `pending`（再試行の欄の前進だけ）・
   `applied`・`failed`・`suppressed` を許す。試験は `project_wake_pending_{off,paused,cancelled,shadow}`・
   `project_wake_effect_{retry,exhausted}`・`project_wake_cos_input_*`・`project_wake_crash_cos_b{4,5}`。
   門と効果の実行の間の窓は 11 で閉じた（書き込みの tx で門をもう一度読む）。
11. **判断の `human_epoch` の照合**（WU stale-epoch-gate、2026-11-04。final review の差し戻し）。tx1 の CAS だけでは、tx1 の後に
   `pending` で残った判断（crash の B4、効果の `Err` の backoff、CoS 入力の Busy）が人の操作の後に実行され、I7 に反していた。
   次の 3 段で止める。
   - **門**: `project_controller_effect_admit` は判断の行の `human_epoch` と `project_controller_state` の今の値を比べ、違えば
     `suppressed`（`effect_ref` `halted:stale_human_epoch`）にする。見る順は mode → phase `paused`・`cancelled` → 案件の状態 →
     `human_epoch` → phase `blocked` の Hold → `effect_next_at` の待ち。backoff 中・`blocked` 中の古い判断も待たせずに止める。
     この順の変更で、phase `blocked` の間に案件が `paused`・`cancelled`・`done`・archive になった判断も Hold でなく `suppressed` になる。
   - **書き込みの tx**: `project_controller_create_root` と `project_controller_queue_cos_input` は判断の `human_epoch` を受け取り、
     挿入と同じ tx で同じ門（`effect_gate_open_tx`）を読む。閉じていれば何も書かず `Refused` を返し、dispatcher が門を
     もう一度通して判断の行に理由を記録する。10 の窓（門を通った直後の人の off）もこれで閉じた。CoS 入力は thread の用意・
     backpressure・挿入を 1 つの tx にした（`chat_system_queue_once_tx`）。
   - **効果が既に効いていた判断**: task-core は `ProjectEffect` を読めないので、dispatcher の `apply` が門より先に
     `effect_in_place` で調べる（root は `task_idempotency_keys`、CoS 入力は案件 thread の `client_message_id` と本文）。あれば
     門の結果によらず `applied` と記録し、実在する root を `halted` と記録しない（crash の B5）。同じ key を選んだ別の判断も同じ扱いで、
     新しい効果は実行しない。

   再評価は人の操作が積む既存の wake `human:<epoch>` が起こす（別の仕組みを足していない）。試験は `project_wake_stale_epoch_` の 13 件。

   **残る窓**: `off` の間の人の操作は数えない（I13）ので、`pending` の判断が残ったまま「`off` → 人の操作 → `off` 以外へ戻す」が
   controller の tick を挟まずに起きると（daemon の停止中など）、`human_epoch` が変わらず古い判断の効果が実行される。tick が 1 回でも
   挟まれば `off` の時点で `halted:mode_off` になるので、窓はこの場合に限る。mode の変更で `human_epoch` を増やす案は、人でない
   principal の扱いを含めて別の判断なのでこの WU では変えない。
12. **decide の失敗の notice と案件別 cursor の読み手**（WU decide-notice-cursor、2026-11-04。final review の指摘 (a)(b)）。
   - **decide の失敗の notice**: `project_controller_decide_failed` は、失敗が `max_retries` を超えて phase を `blocked` にするとき、
     呼び出し側（`dispatcher/project_wake.rs` の `decide_exhausted_notice`）が作った BadNews の notice を、`blocked` にする
     UPDATE と同じ書き込みの tx で記録する（`feed::record_in_tx`）。notice と `blocked` は一緒に残るか一緒に残らないので、間の
     crash で「止まったのに知らせが無い」状態は起きない。source_key は `project_controller:decide_failed:<project>:<revision>`。
     `blocked` の後は decide しないので同じ止まり方では 1 件のまま、人が mode を置き直す（`revision + 1`）と次に止まったときに
     別の notice になる。group_key は効果の失敗と同じ `bad_news:project_controller:<project>`。上限内の失敗は notice を書かない。
   - **案件別 cursor**: `project_controller_state.event_cursor`（tx1 が消費した wake の `source_event_id` の最大値。減らない）を、
     wake を入れる共通の関数 `pc_wake_insert_tx` が読み、`source_event_id` が cursor 以下の wake を入れない。全体の cursor
     （`feed_cursor` の `project_wake`）を戻した再配送で、wake の行が残っていなくても次の判断が起きない。前提は、取り込み
     （`project_wake_ingest`）が event を id の昇順に 1 tx で入れ、全体の cursor を同じ tx で進めることと、wake の消費が
     `ORDER BY id` の先頭からであること。このため cursor 以下の event の wake は、既に入れたか、入れなかった（その時 `off`）
     ものに限る。畳み切れずに残った wake は表に入ったままで、cursor の照合は挿入の時だけなので落ちない。`source_event_id` の
     無い wake（人・cron・停滞・API）は対象外。
   - 試験は `crates/task-dispatch/src/dispatcher/tests/project_wake_cursor.rs` の 6 件
     （`project_wake_decide_{exhausted_notice,retry_no_notice}`・`project_wake_cursor_{per_project,monotonic,blocks_redelivery,keeps_new_events}`）。
     `project_wake_cursor_blocks_redelivery` は照合を外すと落ちることを確かめた。replay script の task-dispatch の下限は 60。

## 付記 A3（2026-11-04、task 01M6JW1EFPBCJX8J7D2ZACHS3D）: 接続の実装と契約の差分
§A3 の契約のうち次を実装し、一部を変えた。理由と未達は [進捗 a3](../progress/2026-11-04-long-horizon/a3.md)。
1. **接続**: `crates/task-dispatch/src/dispatcher/project_decider_ts.rs`（lib の `task_dispatch::project_decider_ts`）の
   `TsProjectDecider` が A1 の `ProjectDecider` を実装する。tick ごとに dispatcher が `ProjectDecider::attach` で store と
   領域 `project_controller` の宛先（`script_target` の mode・generation・口）を渡す。判断の記録 `proposal_json` は
   `ProjectDecider::take_record` が返す `celeris.project-decision/1`（入力・snapshot の SHA-256・generation・proposal・
   explanation・違反・分類・guard）で、A1 の既定の判定では従来どおり入力の JSON。daemon は `NoopProjectDecider` の代わりに
   `TsProjectDecider` を入れる（`crates/celeris/src/daemon/bootstrap.rs`）。
2. **mode の組**: 案件 `shadow` は領域 `compare`・`canary`・`ts`、案件 `enabled` は領域 `ts` か `canary` で選ばれた案件
   （単位は案件 id）のときだけ TS を呼ぶ。足りなければ TS を呼ばず効果なし・phase `waiting` の判断（`pc/script-unavailable`）。
   TS の例外・timeout（2 秒）は `Err` で A1 の backoff と `max_retries` の停止に任せる。検証落ちは効果なし（`pc/invalid:<code…>`）。
3. **`OpenEscalation`**: 受信箱の新種類 `project_controller` は足さず、`ProjectEffect::OpenEscalation` を notice（`bad_news`、
   source key `project_controller:escalate:<案件>:<question_key>`）で実装した（変えた行: §A3・付記 A1 の 2「受信箱の新種類」）。
   理由: 受信箱の種類を足すと `InboxKind`・API schema・web の生成型・triage の分類まで要り、この子の範囲を超える。notice は
   source key で冪等なので「同じ問いを 2 回聞かない」は満たす。notice の記録と phase `blocked` は同じ transaction
   （`SqliteStore::project_controller_escalate`）。tx1 で `blocked` にすると効果の門が Hold にするので、判断の phase は
   `waiting` にして効果の tx で `blocked` にする。受信箱への昇格は A9 か後続で決める（進捗の提案）。
4. **起票の冪等キーの行**: `ProjectEffect::CreateRoot` に `meta`（A2 の `LaunchRootMeta` の JSON）を足し、
   `SqliteStore::project_controller_create_root` が `result_json` に残す（snapshot が controller の root key を引くため。
   A1 の store の file を A3 が変えた唯一の箇所）。
5. **runaway の停止**: Rust の `runaway_guard` が、進捗（`gap.progress_epoch`）の増えない `replan`・`launch_root`
   （`applied`・`suppressed`・`pending`）が `max_no_progress_decisions`（既定 5）続いたら proposal を `escalate`
   （`no_progress:<epoch>`）に置き換える。TS も同じ規則（`pc/no-progress`）を持つ。CoS 入力の 1 日の上限は判断の履歴から
   数え（`cos_inputs_remaining_today`・`roots_remaining_today`）、TS は残り 0 なら `replan` の代わりに待つ。
6. **拒否**: `crates/task-api/src/cos/goal_guard.rs` が `/cos/operations` の `instructed_by` の無い要求を断る:
   `project.update` の `request`（422 `project_request_human_only`）、draft でない task の `task.update` の `objective`・
   `acceptance`（422 `task_goal_human_only`）、mode `enabled` の案件への root の `task.create`（422 `project_controller_owns_launch`。
   `controller_decision_id` は足していない。続きの root は controller が起票する）、controller が `off` でない案件の案件 thread の
   run の `release.promote`（422 `production_requires_human`。§A3 に無い行を足した。状態行が無い・`off` の案件と統括 CoS の
   promote は従来の gate のまま〈I13。既存の試験 `cos_ops_admin_config_daemon_and_external_effects_run_once` の挙動〉）。断った要求は `rejected` の監査。
7. **dry-run**: HTTP の `POST …/controller/dry-run` は足さず、`TsProjectDecider::dry_run`（何も書かない）を足した。
   API state に TS の口が無い（script layer は dispatcher 側）ため。HTTP の route は未達。
8. **E2E** は 1 file でなく試験の対象の crate に置いた（接頭辞 `project_cos_`、計 20 件）: task-dispatch の
   `dispatcher/tests/project_cos.rs`（10、fake の script で tick を通す）、task-api の `tests/project_cos_guard.rs`（6）、
   celeris の `tests/project_cos_shadow.rs`（4、本物の bundle）。`scripts/dev/test-project-cos-shadow.sh` が 3 つを走らせ raw を検査する。
