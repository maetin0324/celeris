# ADR 2026-11-18: Dispatcher の 2 領域で self.store・self.config に触る file を減らす pilot（アーキ v4 E）

- 状態: Accepted（本文は設計。この ADR を書いた WorkUnit `adr` は `crates/`・`scripts/`・`web/` を変えていない。実装と実測の結果は末尾の「付記 2026-11-18」）
- task: 01M7MZTKB6GQ241F6JJYSPJNS1（依頼キー celeris-architecture-scalability-v4-20261117 の子 E、親 01M7MZT8KX547464YYS76QNP4G、依存 C 01M7MZTKB29VF5TX7FCQ15XEMW）
- 進捗: [2026-11-18-arch-v4-e.md](../progress/2026-11-18-arch-v4-e.md)、計測コマンド: [measure-commands.md](../progress/2026-11-18-arch-v4-e/measure-commands.md)
- 関係: [ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md)（D2・D3・D7 の付記）、ADR-0082（dispatcher.rs の module map）、
  A の baseline（`artifacts/baseline.md` §6・§7。task 01M7MZTKAXM8GJGK8BPE13CQRM）、ADR 2026-11-18 org-write-shared-use-case（D の WorkUnit adr の branch にあり、この ADR を書いた時点で main には未統合）（D）

## 背景

基準は main `bc665168c9caf1898adc4c611404bbee056daf9a`。`crates/task-dispatch/src/dispatcher/` の非試験 69 file のうち、
`self.store` を書く file は 44（633 出現）、`self.config` は 35（218）。論理 scope の 76 file では 48（661）・39（224）。
この ADR を書いた時点の HEAD（件名 `integrate wu/loc-fix (phase fix)`）で同じ snippet を当てても値は同じ（B・C は dispatcher/ の参照を変えていない）。

依頼の目安は、work_units と、review_verdict か cos_chat の 2 領域で、store に触る file を 5 以上、config に触る file を 3 以上減らすこと。
判断の関数が自分で store と設定を読むので、読み書きの所在が 44 file に散り、1 つの変更の審査の範囲が広がる（A の §5 の S2）。

## 決定の要約

- D1 対象 file は判断と組み立てだけを持ち、共有の状態（`SqliteStore`・設定の全体・`Dispatcher`・`CosChatLaunch`）に触る経路を型で持たない。
  読み書きは、移す前から self.store・self.config に触る同じ論理 scope の入口 file か、既存の owner の method が行い、対象 file の純粋関数に値を渡す
  （ADR dispatcher-state-ownership D3 の「判断は入力 → 提案の純粋関数、副作用は owner か入口」と同じ分け方）。
- D2 新しい Manager・trait・Port・Facade・owner の struct を足さない。足してよい型は、純粋関数の入力・結果の小さい struct・enum（対象 file の中、`pub(super)` 以下、状態を持たない）だけ。
- D3 呼び出し元から見た `Dispatcher` の method の名と引数を変えない（`review_target_ref`・`observe_behind_target`・`review_sync_targets`・`auto_leaf_decision`・
  `record_run_write_sets`）。`dispatcher/worker_finish.rs`（高 risk の `worker-finish`）・`health_review/design_gate.rs`（`review-gate`）・`health_review/check_item.rs` は変えない。
  worker_finish.rs を変える必要が出ても、呼び出しの引数の変更だけに限る。
- D4 transaction の範囲・event・lease の意味を変えない。store への書き込みの回数・順序・失敗時の扱い（warn だけ、`let _ =`）も変えない。
- D5 新しい file を dispatcher/ に足さない。対象 file を消さない・併合しない・`#[path]` で付け替えない。

ADR dispatcher-state-ownership との関係: D2 の 1・2（owner の field は private、owner の method は `Dispatcher` を受けない）は、
既存の `ExecutionCoordination` の method だけを使うので保たれる。owner の method は足さない。D2 の 4（旧い本体を同じ変更で消す）と
D2 の 5（200 行を超える関数を丸ごと移さない）に従う。D3 の transaction の範囲の規則（分けない・まとめない）にも従う。

## 対象 file と狭い操作

行番号は base の行。移す先の self.store・self.config の値は C3 の 1 行の数。

### 第 1 領域: WU（work_units.rs と、WU の dispatch・write set・behind target・auto leaf の file）

| file（行数） | 今の self.store・self.config | 移す先（狭い操作） | 対象 file に残すもの | 減る見込み |
|---|---|---|---|---|
| `dispatcher/behind_target.rs`（94） | store: 20（`task_ops::behind_target::observe_behind_target(self.store.as_ref(), …)`）、49（`self.store.repo_get(repo_id)`）。config: 45（`self.config.worktree_branch_prefix`）。3 method とも `impl Dispatcher` | review の同期の入口 `dispatcher/review_spawn.rs`（store 47・config 14。`observe_behind_target` の呼び出し元 530・534 行）に、名を変えない `Dispatcher` の薄い入口 3 つ。入口が repo の既定 branch・branch の prefix を読み、`observe_behind_target` の記録は既存の `task_ops::behind_target::observe_behind_target` を呼ぶ | store と設定に触らない関数: `review_target(parent: Option<String>, configured_default: Option<&str>, source: &Path) -> Option<String>`（親の branch か既定 branch。git を読むだけ）、`sync_sites(task, workspaces) -> Vec<(RepoId, PathBuf, PathBuf)>`（worktree の dir が有る repo） | store 1・config 1 |
| `dispatcher/auto_leaf.rs`（219） | store: 185（`task_ops::tree::decision_path(self.store.as_ref(), task)`）。config: 183（`self.config.execution.limits.tree`）。`impl Dispatcher` は 171-219 行 | WU の入口 `dispatcher/work_units.rs`（store 57・config 11）に `Dispatcher::auto_leaf_decision` の薄い入口（limits と decision path を読み、純粋関数を呼ぶ）。worker_finish.rs:499 の呼び出しは変えない | 純粋関数 `decision_request(task, wu, run_id, counts, checkpoint, limits: TreeLimits, path: Vec<DecisionPathEntry>) -> DecisionRequest`（今の 186-218 行の組み立て）。`AutoLeafWatch`・`AutoLeafCounts`・`counts`（既に events を引数で受ける） | store 1・config 1 |
| `dispatcher/write_set_gate.rs`（122） | store: 69（`effective_task_write_paths`）、72（`work_unit_expected_write_paths`）。config: 無し。`impl Dispatcher { admit_write_set, write_reservation_for }` | dispatch の入口 `dispatcher/dispatch_run.rs`（store 30・config 25。呼び出し元 239 行）に `admit_write_set` の入口: hint を store から読み（会話の task と planner の run は今と同じく読まない）、純粋関数で予約を作り、既存の owner の method `ExecutionCoordination::admit_write_set` に判定させ、`Hold` なら今と同じく `note_dispatch_wait` | 純粋関数 `reservation(task, task_hint, wu_hint: Option<Option<Vec<String>>>) -> Option<WriteReservation>`（inherited の判定・`normalize_write_paths`・repo 鍵）と `write_set_repo_keys` | store 1 |
| `dispatcher/write_set_record.rs`（197） | store: 118（`record_run_write_set`）、153（`record_work_unit_write_set`）。config: 無し。`impl Dispatcher { capture_run_write_bases, record_run_write_sets, record_work_unit_write_sets }` | 記録の書き込みは `dispatcher/work_units.rs` の `record_run_write_sets` の入口（worker_finish.rs:831 の呼び出しを変えない）。開始 HEAD の保持は既存の owner の method `ExecutionCoordination::put_run_write_bases`・`take_run_write_bases`。dispatch_run.rs:979 の `capture_run_write_bases` は dispatch_run.rs の入口で `put_run_write_bases(run_id, run_write_bases(task, ws))` | store と設定に触らない関数（git の読み取りは `task_ops::changes`）: `run_write_bases(task, ws) -> Vec<RunWriteBase>`、`run_records(task, run_id, wu_id, bases, has_local_site) -> Vec<WriteSetRecord>`、`work_unit_records(task, wu, ws) -> Vec<WriteSetRecord>`、`git_sites`、`write_set_record` | store 1 |
| `dispatcher/work_units.rs`（2060） | store 57・config 11 | 入口として残る（減らない）。上の入口を受けるので出現は増えうるが、file 数は増えない | — | 0 |

WU 領域に入れないもの: `dispatcher/wu_base.rs` は既に 0・0。`dispatcher/tree_units.rs`（store 56・config 19）は木の unit で WU の領域の外。
`work_units.rs` 自身の判断（`wu_dispatch_gate` など）は今回動かさない。2060 行の大部分は WU の遷移と commit の副作用で、
lease・restart・並列 WU の反例の本体に直接触れ、D2 の 5（200 行を超える関数を丸ごと移さない）の分割が先に要る。pilot の範囲を越える。

WU 領域の見込み: store −4（behind_target・auto_leaf・write_set_gate・write_set_record）、config −2（behind_target・auto_leaf）。

### 第 2 領域: cos_chat/

この領域の `self` は `CosChatLaunch`（`cos_chat/launch.rs:218`。`pub store: Arc<SqliteStore>`・`pub config: CosChatLaunchConfig`）か
`ChatRunSink`（`cos_chat/sink.rs`）で、store は `Dispatcher` と同じ SQLite の `Arc`。C3 は字面で数えるので、これも数に入る。

| file（行数） | 今の self.store・self.config | 移す先（狭い操作） | 対象 file に残すもの | 減る見込み |
|---|---|---|---|---|
| `cos_chat/relay_rejudge.rs`（160） | store（1 行）: 51（`item_task_id(self.store.as_ref(), …)`）、123（`claim(&self.store, &run_id)`）。改行で割れた参照: 27-28（`cos_triage_relay_candidates`）、126-127、143-144（`cos_triage_plan_rejudge_candidates`）。config: 131（`self.run_configs.insert(…, self.config.clone())`）。`&mut Dispatcher` 25・108・141、`&task_core::SqliteStore` 112 | 入口 `cos_chat/launch.rs`（store 15・config 3。`relay_rejudge`・`plan_rejudge` を 348-354 行で呼ぶ）に、候補の読み出し・受信箱の item の組み立て・claim（store が 1 transaction で item と run を結ぶ今の経路）・run の起動（`launch_routes`）を置く | 純粋関数: `covers`、候補の選別（候補・走っている thread・判定済みの集合・item の task・受信箱の item → 起こす `(thread, 判定の入力)` の列）、plan の再判定の選別 | store 1・config 1（SqliteStore を書く file も 1） |
| `cos_chat/slot_wait.rs`（194） | store（1 行）: 55、73、140、181、191。改行で割れた参照: 146-147、168-169（E1 で 8、1 行の C3 で 6）。config: 105（`run_configs.get(…).unwrap_or(&self.config)`）。`&mut Dispatcher` 66（`now_utc`・`coord.accepting_new_work`・`maint.disk_ready`・`has_capacity`・`launch_routes`） | chat run の状態の書き込み（`chat_run_status`・`chat_run_finish_prelaunch`→`chat_run_finish`・system message・credential の revoke）は、run の閉じ方を既に書く `cos_chat/control.rs`（store 10・config 1。`close_stopped`）と `cos_chat/routes.rs`（store 10・config 2。`hold_slot_wait` の呼び出し元 191 行・`fail_routes`）へ。待ちの巡回と起動は `cos_chat/launch.rs` へ | 純粋関数 `step(wait, run, now, gates) -> SlotWaitStep`（`Drop`・`Close { state, prelaunch, text }`・`Hold`・`Launch`）と待ちの型 | store 1・config 1 |

`slot_wait.rs` の `close_unlaunched` と `routes.rs` の `fail_routes`・`control.rs` の `close_stopped` は、書き込みの並び（prelaunch の終わり → 失敗なら通常の終わり、
credential の revoke）が似ている。cos-region は 3 つを比べ、同じ仕様なら 1 つの関数にする（同じ仕様の二重実装を消す）。state・prelaunch の理由・通知の文が違えば、
意味の違うものをまとめず両方残し、違いを進捗に書く。

cos_chat に入れないもの: `routes.rs`（11・2。改行込み）は起動の経路の中心で、外すと launch.rs に 200 行を超える処理が移る。`root_done.rs`・`triage.rs`・
`fallback.rs` は 13〜24 の参照を持ち、1 file を外す費用が大きい。`sink.rs` は chat run の書き手そのもの。`digest.rs`・`rollover.rs` などは既に 0・0。

cos_chat 領域の見込み: store −2、config −2。

### 合計の見込みと届かないとき

| 数え方 | before | 見込み（after） | 純減 |
|---|---|---|---|
| C3 scope 69 file: self.store | 44 file / 633 | 38 file | −6（目安 5） |
| C3 scope 69 file: self.config | 35 file / 218 | 31 file | −4（目安 3） |
| C3 論理 scope 76 file: self.store | 48 / 661 | 42 | −6 |
| C3 論理 scope 76 file: self.config | 39 / 224 | 35 | −4 |

非試験 file の数（分母）は 69・76 のまま。出現数は入口へ移した分で同じか増えうる。成果は file の数で、出現数は増えた理由を進捗に書く。
目安に対する余裕は store・config とも 1 file。cos の 2 file のどちらかが D4（意味を変えない）か下の LoC の上限で外せなければ、
WU 領域だけでは store −4・config −2 で目安に届かない。そのときは数を合わせるために別の file を寄せず、進捗に数値と理由を書いて
measure の check を落とし、replan を求める。

LoC の上限: 2 領域の production の純増は +80 行を目安にする（入口の関数 8 個ほどの signature と引数の分）。C12 の `views.product_only.lines` の差で測る。
超えたら進捗に理由を書く。+200 行を超えたら、減った file の数に見合わないので成果に数えず replan を求める。

### review_verdict.rs を選ばない理由

- `dispatcher/review_verdict.rs` は 1 file に self.store 32・self.config 3。全部を外しても file の数は store 1・config 1 しか減らない。cos_chat は 2 file で store 2・config 2 が減る。
- `on_review_finished` は stale review・restart・replan の反例（`cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored`・
  `cex_late_review_pass_after_replan_is_dropped`・`cex_late_review_pass_after_repair_is_dropped`・
  `cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate`）の本体。32 か所の読み書きを `review_decision::judge_review` と入口に分けると、
  review lock・runs 行の確認・計画の開き直しの順序に触れる範囲が最大になる。1 file の減りに対して risk が大きい。

### D と write set が重ならないこと

D（01M7MZTKB4RYM80S6CT3RST2N9）の設計（ADR 2026-11-18 org-write-shared-use-case（D の WorkUnit adr の branch にあり、この ADR を書いた時点で main には未統合））が変える path は
`crates/task-api/src/`（handlers・cos・skills・schema・新しい org_write.rs）と `crates/task-core/src/store/org.rs`。E が変える path は
`crates/task-dispatch/src/dispatcher/` の下だけ（試験は `dispatcher/tests/structure_guard.rs`）。重ならないので D と並行してよい。

## 数え方と数えないもの

- 主の数: A の baseline.md §7 C3 の snippet（measure-commands.md に写した）。scope は dispatcher/ の非試験 69 file、論理 scope は
  baseline.md §6 の 1（69 − `project_decider_ts.rs` ＋ `#[path]` の 8 file = 76 file）。after で `#[path]` が変わったら
  `git grep -n '#\[path' $B -- crates/task-dispatch/src` で選び直す。before は scope 44/633・35/218、論理 48/661・39/224。
- 補助の数 E1（A と違う数え方。理由: 字面の抜け）: C3 は 1 行の中の `self.store` だけを数え、rustfmt が `self` と `.store` を別の行に割った参照を数えない
  （relay_rejudge.rs の 27-28 行など）。改行の入れ方で数が変わるので、`self\s*\.\s*(store|config)\b` を file 全体に当てた数を並べる。
  before は scope 46 file/759・35/227、論理 50/791・39/233。C3 と E1 の両方で減っていなければ成果に数えない。
- 補助の数 E2: `SqliteStore` を書く file の数（scope 27、論理 29）と、crate 全体（`crates/task-dispatch/src` の非試験 138 file）の
  self.store 53/683・self.config 40/238・SqliteStore 50/195。dispatcher/ の数が減って crate 全体の数が同じだけ減らない（dispatcher/ の外へ移した）なら、
  その分を成果に数えない。SqliteStore の file が増えたら（対象の関数が `&SqliteStore` を引数で受ける形にした）成果に数えない。
- 補助の数 E3: 対象 6 file の token の数（guard と同じ token。measure-commands.md の E3。before の合計は 5・4・4・5・14・13）。
- 数えないもの:
  - 分母の増加（dispatcher/ に file を足して比を下げる）。
  - scope の外への移動（dispatcher/ の外の module・`#[path]` の付け替え・task-ops などの別 crate へ移して呼ぶ）。既にある `task_ops` の関数を呼ぶのは移動ではない。
  - file の併合・削除（対象 file を入口に併合して数を減らす）。入口に移すのは store・設定の読み書きと `Dispatcher` の method の呼び出しだけで、判断の本体は対象 file に残す。
  - field の梱包（store と config を新しい struct に包み `self.ctx.store` のように呼ぶ）。
  - 別名（`let s = &self.store;`・`let Dispatcher { store, .. } = self;`・`fn store(&self)` の getter・`&self.store` を引数で対象 file の関数に渡す）。
  - 新しい Manager・trait・Port・Facade。改行で 1 行の字面を割ること（E1 で見る）。

## 証明（可視性・依存・負の試験）

1. 可視性（コンパイル）: 対象 6 file に `impl Dispatcher`・`impl CosChatLaunch` を置かず、関数の引数・戻り値・欄に `Dispatcher`・`CosChatLaunch`・
   `SqliteStore`・`DispatchConfig`・`CosChatLaunchConfig` を置かない。対象 file の関数は値と小さい入力の struct だけを受けるので、
   コンパイラはこの file から store と設定の全体に届く経路を通さない。`use super::*;` の glob（auto_leaf・write_set_gate・write_set_record）を明示の `use` にする
   （glob のままでは `Dispatcher` の名が見え、依存の計測と guard が効かない）。owner の method は `ExecutionCoordination` の既存のもの
   （field は private、`Dispatcher` を受けない。`structure_guard_execution_coordination` が確かめている）だけを使う。
2. 依存の計測: measure-commands.md の C3・E1・E2・E3 の before/after。dispatcher の SCC（A の §4: 3 単位・4 組・22 本）が C1・C2 で悪化しないこと、
   C7b の S2（direct 6・transitive 14・4 package）が増えないこと。
3. 負の試験（`crates/task-dispatch/src/dispatcher/tests/structure_guard.rs` に置く）:
   - helper `shared_state_access(src: &str) -> Vec<&'static str>`: `//` の comment（doc comment を含む）を除き、空白を詰めた source に次の token が
     現れるかを返す。`self.store`、`self.config`、`SqliteStore`、`Dispatcher`（`&Dispatcher`・`&mut Dispatcher`・`impl Dispatcher` を含む。`DispatchError` は別の語）、
     `CosChatLaunch`（`CosChatLaunchConfig` を含む）、`DispatchConfig`、`usesuper::*`。
   - `store_access_guard_work_units`: `include_str!` で読む behind_target・auto_leaf・write_set_gate・write_set_record に違反が 0。
   - `store_access_guard_cos_chat`: relay_rejudge・slot_wait に違反が 0。
   - `store_access_guard_detects_each_violation`（負の試験）: 違反を 1 つずつ含む source（`self.store.task_get(id)`、`self\n    .config.max_wall_secs`、
     `fn f(s: &SqliteStore)`、`fn f(d: &mut Dispatcher)`、`impl Dispatcher {`、`use super::*;`、`let s = &self.store;`、`fn f(l: &mut CosChatLaunch)`）を
     helper に通して各々が検出されること、comment の中の語・`DispatchError`・`store_path` のような似た語を検出しないことを確かめる。
   - 試験の doc comment に、守る不変条件と、既存の試験で検出できない理由（振る舞いの試験は「どの file が store に触るか」を見ないので、
     対象 file に self.store を戻しても全部通る）を書く。
   - 置き方: wu-region が helper・負の試験・`store_access_guard_work_units` を structure_guard.rs の末尾に足す。cos-region は wu-region と並行に走り
     helper を使えないので、structure_guard.rs を変えず、自分の範囲を measure-commands.md の E3 を check にして確かめる。
     `store_access_guard_cos_chat`（helper を呼ぶ 10 行ほど）は、impl の統合の後に measure が structure_guard.rs に足す。
     同じ helper を 2 つ書かず、同じ file の同じ場所を並行に変えない。

## 反例試験

既存の反例試験を弱めず、名前を変えずに通す（ADR dispatcher-state-ownership の付記 2026-11-14 の表。すべて `crates/task-dispatch/src/dispatcher/tests/` の下）。

| 範囲 | 試験 |
|---|---|
| lease | `cex_lease_restart.rs`: `cex_stale_holder_late_finish_does_not_overwrite_the_reassigned_run`、`cex_lease_renew_after_expired_snapshot_keeps_the_holder` |
| restart | `cex_lease_restart.rs`: `cex_draining_instance_reclaims_its_expired_lease_without_redispatching`、`cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent`、`cex_restart_after_wu_done_promotes_dependents_once` |
| stale review | `cex_lease_restart.rs`: `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored`。`cex_review_authority.rs`: `cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate` |
| 並列 WU | `cex_lease_restart.rs`: `cex_two_parallel_work_unit_runs_finishing_in_one_tick_lose_no_update`、`cex_run_finish_and_work_unit_check_finish_in_one_tick` |
| replan | `cex_late_completion.rs`: `cex_late_review_and_integration_after_replan_are_dropped`、`cex_late_review_pass_after_replan_is_dropped`、`cex_late_review_pass_after_repair_is_dropped` |

上の 12 本はこの ADR を書いた時点の HEAD にあることを `git grep -l "fn <名>\b" -- crates` で確かめた。
領域の振る舞いを守る既存の試験: `write_set_gate.rs`（`write_set_gate_*` 7 本。並列・直列化・hint なし・別 repo・飢餓なし・継いだ hint）、
`write_set_record.rs`（`write_set_record_*` 3 本）、`cos_slot_wait.rs`（slot の待ち・cooldown・予約・期限・stop の 5 本）、
`cos_chat_triage.rs` の `cos_chat_triage_relay_after_escalation_rejudges_covered_items_after_the_running_run`・
`cos_chat_triage_answered_decision_rejudges_the_escalated_plan_approval_once`、`stale_priority.rs`・`target_sync.rs`（behind target と review の target）、
`tree_decisions.rs`・`tree_gate.rs`（auto leaf）、`structure_guard_*`。どれも `bash scripts/dev/test-affected.sh` が task-dispatch の変更で走らせる。
全体の試験は release gate だけで走らせる（ADR 2026-10-20 full-tests-at-release-only）。

## 撤去対象

- `behind_target.rs`・`auto_leaf.rs`・`write_set_gate.rs`・`write_set_record.rs` の `impl Dispatcher` の block（入口へ移した method の旧い本体。同じ名の method は入口に 1 つだけ置く）。
- `relay_rejudge.rs`・`slot_wait.rs` の `impl CosChatLaunch` の block と `&mut Dispatcher`・`&task_core::SqliteStore` の引数。
- 対象 file の `use super::*;`。
- `close_unlaunched` と `fail_routes`・`close_stopped` が同じ仕様なら、重なる方（上の cos_chat の節）。
- 文書: `docs/architecture-map.md` の WU・cos_chat の行の入口の記述と、`dispatcher.rs` の module map（ADR-0082）の該当行の説明（close が直す）。
- 旧い経路の試験は無い（method の名を変えないので、既存の試験はそのまま新しい経路を通る）。試験を消さない・弱めない。
- 旧新の並存・互換分岐・切替 flag は置かない（同じ変更で旧い本体を消す）。

## 実装前 brief（wu-region・cos-region が使う）

1. 根本原因: `Dispatcher`（と `CosChatLaunch`）が store と設定を field に持ち、`impl Dispatcher`・`impl CosChatLaunch` を書く file はどれもそれを直接読める。
   判断の関数が自分で store を読むので、状態の読み書きの所在が 44 file に散る。症状は、1 つの変更の審査と影響の範囲が広がること。
2. 責務 owner: 読み書きの入口は review_spawn.rs（review の同期）・dispatch_run.rs（dispatch）・work_units.rs（WU の終わりの記録）・
   launch.rs・control.rs・routes.rs（chat run）。write set の予約と開始 HEAD の状態の owner は `ExecutionCoordination`。
3. 既存の再利用先: `ExecutionCoordination::admit_write_set`・`put_run_write_bases`・`take_run_write_bases`、`task_ops::behind_target::observe_behind_target`、
   `task_ops::tree::decision_path`、`task_ops::changes::*`、`control::close_stopped`、上の反例と領域の試験、`structure_guard.rs`。
4. 選択肢: (a) 採用: 判断を対象 file の純粋関数に残し、読み書きを既存の入口と owner に寄せる。(b) 不採用: `WuStorePort`・`ChatRunWriter` のような新しい trait・struct
   （Port の追加で、依存も影響の範囲も減らない）。(c) 不採用: 対象 file を入口に併合（数えない）。(d) 不採用: review_verdict.rs（上の理由）。
   (e) 不採用: `&SqliteStore` を引数で対象 file に渡す（別名と同じ。E2 と guard が検出する）。
5. 新しい状態・分岐・fallback・port: 無し。足す型は純粋関数の入力・結果の小さい struct・enum だけで、状態を持たない。
6. 撤去対象: 上の「撤去対象」。
7. 不変条件: transaction の範囲・event・lease の意味、書き込みの回数・順序・失敗時の扱いを変えない（反例試験と領域の試験）。対象 6 file が共有の状態に触らない
   （`store_access_guard_*`）。分母 69・76 が増えない（C3）。高 risk の class の path（worker_finish.rs・design_gate.rs ほか）を変えない。

## 付記 2026-11-18（実測の結果。WorkUnit close）

実装は WorkUnit wu-region（write-set・target-leaf・guard）と cos-region（relay・slot・guard）、計測は measure（counts・tests）。値の詳細と再現コマンドは
[metrics.md](../progress/2026-11-18-arch-v4-e/metrics.md)、[wu-region.md](../progress/2026-11-18-arch-v4-e/wu-region.md)、
[cos-region.md](../progress/2026-11-18-arch-v4-e/cos-region.md)。基準は main `bc665168c9ca`（B0）と `git merge-base HEAD main`（B1。件名
`integrate wu/loc-fix (phase fix)`）。B0 と B1 の値は全部同じ。

### 減った file と数値

- self.store を書かなくなった file（6）: `behind_target.rs`・`auto_leaf.rs`・`write_set_gate.rs`・`write_set_record.rs`・`cos_chat/relay_rejudge.rs`・`cos_chat/slot_wait.rs`。
- self.config を書かなくなった file（4）: `behind_target.rs`・`auto_leaf.rs`・`cos_chat/relay_rejudge.rs`・`cos_chat/slot_wait.rs`。
- 増えた file は無い。出現が増えたのは前から書いていた入口の 6 file（`dispatch_run.rs`・`review_spawn.rs`・`work_units.rs`・`cos_chat/{launch,routes,control}.rs`）だけ。

| 計測 | before（B0・B1） | after（HEAD） | 差 |
|---|---|---|---|
| C3 依頼 scope 69 file: self.store | 44 file / 633 | 38 / 633 | −6（目安 −5） |
| C3 依頼 scope: self.config | 35 / 218 | 31 / 220 | −4（目安 −3） |
| C3 論理 scope 76 file: self.store・self.config | 48 / 661・39 / 224 | 42 / 661・35 / 226 | −6・−4 |
| E1（改行込み）依頼 scope: store・config | 46 / 759・35 / 227 | 40 / 759・31 / 227 | −6・−4 |
| E2 crate 全体 138 file: store・config・SqliteStore の file | 53・40・50 | 47・36・49 | −6・−4・−1 |
| 分母（依頼 scope・論理 scope・crate） | 69・76・138 | 69・76・138 | 0 |
| E3 対象 6 file の token | 5・4・4・5・14・13 | 0・0・0・0・0・0 | — |
| C12 production（全言語・Rust） B1 → HEAD | — | — | +61・+61（目安 +80 の中） |
| C12 own_total_with_tests B1 → HEAD | — | — | +195（試験 +134 は guard の試験） |

出現数が減らないのは、対象 file の参照を入口へ同じ数だけ移したから（読み書きの回数は同じ）。self.config の C3 の +2 は `work_units.rs` で rustfmt が
`self\n.config` に割っていた 2 か所を 1 行にした分で、E1 では 227 → 227。dispatcher の SCC（今の checker で 4 単位・7 組・51 本）と C7b の S2
（direct 6・transitive 14・package 4）は B1 → HEAD で変わらない。B1..HEAD の crates/ の差分は task-dispatch の 13 file（全部 M）だけ。

### 証明の結果

- 可視性: 対象 6 file に `impl Dispatcher`・`impl CosChatLaunch`・`use super::*;` が無く、引数・戻り値・欄に `Dispatcher`・`CosChatLaunch`・
  `SqliteStore`・`DispatchConfig`・`CosChatLaunchConfig` が無い（E3 が 0）。足した型は `type RelayPick`・`enum SlotWaitStep` と、可視性を
  `pub(super)` にした既存の `struct GitSite` だけ。trait・Manager・Port・Facade と、store・config を包む struct は無い。
- 負の試験: `structure_guard.rs` の `store_access_guard_work_units`・`store_access_guard_cos_chat`・`store_access_guard_detects_each_violation`
  （helper `shared_state_access`）。対象 file に `self.store` を一時的に入れると落ちることを wu-region と cos-region が確かめた（commit していない）。
- 反例試験: 上の「反例試験」の表の 12 本がすべて pass。filter `cex_ | replan | parallel_work_unit | structure_guard | store_access_guard` の 121 本も全部 pass。

### ADR の表と違ったこと

- `record_run_write_sets` の入口は `work_units.rs` ではなく `dispatch_run.rs` に置いた（人の決定 `record-run-write-sets-home`、2026-11-18）。
  `work_units.rs` は本体の行が閾値 2000 を超えた file（base 2060）で、repo health の `size.responsibility-growth` と `source-size` が 1 行の増加でも落ちる。
  開始 HEAD の固定（`put_run_write_bases`）と取り出し・記録（`take_run_write_bases`）が同じ `dispatch_run.rs` に並ぶ。
  `capture_run_write_bases` は method にせず、`dispatch_run.rs` の 1 か所で `put_run_write_bases(run_id, run_write_bases(task, ws))` を呼ぶ形にした。
- 「`work_units.rs` は入口として残る（減らない）」の行は、次のように読み替える: `work_units.rs` は閾値を超えていて行を増やせず、module の本体の行の
  合計（base 32338）も増やせない。入口を足すなら同じ量を同じ file で詰める（target-leaf は `auto_leaf_decision` の入口の分を、振る舞いを変えない
  6 か所の圧縮で相殺し、2060 → 2056）。
- `close_unlaunched`（`control.rs` に移した）・`fail_routes`・`close_stopped` は、閉じる state・落ちたときの範囲・通知・revoke・`digest_due` が違うので
  併合していない（cos-region.md の比較の表）。

### 未達の項目

- 目安（store −5・config −3）・分母不増・crate 全体で同数・SqliteStore 不増・production +80 以内は、すべて満たした。余裕は store・config とも 1 file。
- 共通受入条件 (5) の test-affected の時間は 1 回（769.32 s。A の S2 中央値 655.59 s より +113.73 s）だけで、悪化かどうかを判定していない（要判断。3 回の比較は G）。
- (1) の A の before（3 単位・4 組・22 本）は B0 の checker の値で、今の checker（arch-v4 B の変更後）では B0・B1・HEAD とも 4・7・51。
  同じ checker で比べた差は 0。
- (3)（handlers⇄cos・SQL の公開面）は D の範囲で、E は扱っていない。
- release gate（`scripts/selfdeploy/release.sh`）は未実施（人が行う）。
