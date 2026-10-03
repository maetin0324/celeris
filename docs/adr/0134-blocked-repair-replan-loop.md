# ADR-0134: daemon が足した repair WU の plan_issue と replan の繰り返しを解く

---
tasks: [01M3YMQ8G597TJ5J4H550QPW2P]
---

- 日付: 2026-10-02
- 状態: Accepted（実装は後続 WorkUnit `replan-supersede`・`settle-ready`・`repro`）
- 関連: [ADR-0072](0072-task-execution-decomposition.md) D17/D18、[ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md) F5-fix、[ADR-0079](0079-recursive-task-decomposition.md) 付記「R7-9」「R7-12」

## 背景

2026-10-02 夜、定期実行の task `01M3YF3NS2EGTZD2BBWNPG1K28` で次が起きた。段 `core` の統合検査が `tests/e2e/tests/phase7_scenarios.rs` の旧前提 assert で落ち、daemon が統合の repair WU `repair-core-2` を足した。直すべき file はその repair WU の許可範囲（RepairScope）の外なので、repair WU は `blocked(plan_issue)` で止まった。planner は replan で同じ内容を直す葉 `e2e-cancel` を段 `core` に足したが、計画 v3〜v6 の replan が同じ内容で繰り返され、`e2e-cancel` は一度も走らなかった。人が task を一時停止して止めた。原因は 3 つある。

1. **settle_phase の判定順**（`crates/task-dispatch/src/execution_scheduler.rs` `settle_phase`）。段の中に `failed`、または `blocked(limit|plan_issue|dependency_failed)` の unit が 1 つでもあれば、同じ段に `Ready` の葉があっても先に `PhaseSettle::Failure` を返す。新しい版で足した `e2e-cancel` が `Ready` になっても、走る前に再び replan に入る。
2. **daemon_added の持ち越しと DaemonAddedKeyReused**（`crates/task-ops/src/execution.rs` の replan）。daemon が足した WU（統合 WU・統合の repair WU）は planner の視野に無いので、段が新しい版に残っていれば base のまま持ち越す（ADR-0074 F5-fix）。planner が同じ key を書くと `DaemonAddedKeyReused` で拒否される（ADR-0079 R7-12）。planner には `blocked(plan_issue)` の repair WU を取り除く手段が無い。
3. **人の回答で再開する経路の条件**（`crates/task-dispatch/src/dispatcher/work_units.rs`）。人の回答（`Transitioned.reason == "answer"`）による `blocked(question|limit|plan_issue)` の再開は、scheduler が行き詰まりと判定したとき（並列なら `settle_phase` が `Question`/`Failure`、直列なら `next_work_unit` が `Stuck`）だけ試す。replan の承認は人の回答ではないので、この経路は働かない。再開できても、repair WU は範囲外なので再び plan_issue で止まる。

結果として、計画の承認のたびに `Failure` → replan → 同じ計画の再提出が回り、planner run が無駄に使われた。

## D1. replan の新しい版で、止まった daemon の repair WU を superseded にする

replan の新しい版を採るとき（`task_ops::execution` の replan）、次の条件をすべて満たす WU は、新しい版に書かれていなくても持ち越さず `superseded` にする。

- daemon が足した WU（`task_core::is_daemon_added_work_unit`）で、`kind = repair`（統合 WU そのものは除く）。
- `status = blocked` かつ `blocked_reason` が `plan_issue` または `limit`。

superseded にした行には今の削除と同じ扱いをする。`Event::WorkUnitTransitioned { from: blocked, to: superseded, reason: "replan v{N}" }` を出し、`ReplanDiff.removed` に key を載せる。`blocked_reason` は外す。

- 理由: plan_issue（範囲外）と limit（試行の上限）は、その repair WU のままでは進めないという daemon 自身の判定である。replan はその判定を受けて planner が範囲を組み直す場面なので、止まった repair WU を残す理由が無い。直す内容は planner が新しい葉として書く（今回の `e2e-cancel`）。
- `running`・`ready`・`pending`・`failed`・`blocked(question|dependency_failed)` の repair WU、統合 WU、done の WU は今のまま持ち越す（ADR-0074 F5-fix を狭めない）。`question` は人の答えで進めるので消さない。
- **統合 WU は残す。** superseded の repair WU は統合 WU の依存として待たれない（superseded は生きた unit ではない）。その段の残りの unit（新しい葉を含む）がすべて done になれば、統合 WU は R7-9 の開き直し（`stale_stage_integrations` / `reopened_integration`）または通常の `PhaseSettle::Integrate` で再び統合する。同じ段に次の統合失敗が起きれば、daemon は新しい番号の repair WU（`repair-{phase}-{n+1}`）を足す。superseded にした key は再利用しない。
- planner は daemon の key を書かなくてよい。`DaemonAddedKeyReused` の検査（R7-12）は生きた daemon WU だけを見るので、superseded にした key を planner が書いても衝突しないが、新しい葉は別の key で書くことを規則とする。

## D2. settle_phase は同じ段の進められる葉を先に走らせる

`settle_phase` の判定順を次にする。

1. `Running` の非 task unit があれば `Wait`（今のまま）。
2. 段に `blocked(question)` があれば `Question`（今のまま最優先）。
3. 同じ段に `Ready` または `NeedsContinuation` の**非統合** WU があれば `Advance`。
4. それが尽きても `failed`、または `blocked(limit|plan_issue|dependency_failed)` が残っていれば `Failure`。
5. 以下は今のまま（段がすべて done なら `Integrate` など）。

- 理由: 新しい版で足した葉が、同じ段の止まった unit より先に走る。葉が done になれば D1 と合わせて段の統合へ進み、replan は起きない。葉が無ければ 4. で今と同じく `Failure` → replan になるので、止まった unit を放置することはない。
- 失敗した unit に依存する葉は `blocked(dependency_failed)` か `pending` なので 3. に当たらず、失敗を飛ばして下流を走らせることは無い。
- daemon に LLM 呼び出しは入れない。D1・D2 とも行の状態だけを見る決定的な規則で、replay（`task_ops::replay`）は同じ関数で同じ結果を出す。D1 の判定は `task_ops::execution::is_blocked_daemon_repair` に切り出してあり、`replay::rebuild_work_units_and_runs` もこの関数を呼んで replan の畳み込みで同じ行を `superseded` にし、統合 WU の `depends_on` からも外す（試験 `replay_matches_live_after_blocked_repair_superseded`・`_limit`）。

## D3. WU 単位の取り下げ・再開 API は作らない

`POST /tasks/{id}/work-units/{key}/withdraw`・`resume` のような WU 単位の操作 API は今回作らない。

- 理由: D1・D2 で、今回の行き詰まり（範囲外で止まった daemon の repair WU と、同じ内容の新しい葉）は人の操作なしに解ける。WU 単位の API は計画の版・統合 WU の依存・replay の再現性（WU の遷移は計画の採用と run の結果からだけ起きる）に新しい経路を足すことになり、影響が大きい。
- 未解決: planner が新しい葉を足さないまま、人が「この repair WU だけを捨てたい／やり直したい」場面は、今は task の一時停止と replan（または人の回答）でしか扱えない。必要が繰り返し観測されたら、別 ADR で API と replay 上の扱いを決める。

## 試験

後続 WorkUnit で次の名前の試験を足す。

- `blocked_repair_superseded`（task-ops）: daemon が足した `blocked(plan_issue)` と `blocked(limit)` の repair WU が replan の新しい版で `superseded` になり、`reason = "replan vN"` の `WorkUnitTransitioned` と `ReplanDiff.removed` に載ること。`blocked(question)`・`failed`・統合 WU は持ち越されること。
- `settle_ready_before_blocked`（task-dispatch `execution_scheduler`）: 同じ段に `blocked(plan_issue)` と `Ready` の葉があるとき `Advance`、葉が尽きると `Failure`、`blocked(question)` があれば `Question` を返すこと。
- `blocked_repair_replan_loop`（task-dispatch dispatcher の再現試験）: 統合失敗 → repair WU が plan_issue で blocked → planner が同じ内容の葉を足した replan、の場面で、replan が繰り返されず葉が走り、段の統合へ進むこと。

## 影響

- 止まっている task `01M3YF3NS2EGTZD2BBWNPG1K28` は、修正を含む release の後に人が再開する。手順は `repro` WorkUnit が `docs/PROGRESS.md` に書く。
