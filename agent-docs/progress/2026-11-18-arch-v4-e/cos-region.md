---
title: アーキ v4 E — cos_chat 領域（relay_rejudge・slot_wait を store・設定に触らない関数にする）
tasks: [01M7QT7K78ZABD62HM4J3BKNJE]
status: running
updated: 2026-11-18
---
# アーキ v4 E — cos_chat 領域: relay_rejudge・slot_wait

WorkUnit relay（relay_rejudge.rs）の節の後に、WorkUnit slot（slot_wait.rs）の節、最後に WorkUnit guard
（`store_access_guard_cos_chat` の試験と、2 領域の before/after の仕上げ）の節が入る。

設計は [ADR 2026-11-18 dispatcher-store-access-pilot](../../adr/2026-11-18-dispatcher-store-access-pilot.md) の「第 2 領域: cos_chat/」の表。
計測コマンドは [measure-commands.md](measure-commands.md) の C3・E1・E2・E3。WU 領域の記録は [wu-region.md](wu-region.md)。
この領域の `self.store`・`self.config` は `Dispatcher` の欄ではなく `CosChatLaunch` の欄（同じ DB の `Arc<SqliteStore>` と `CosChatLaunchConfig`）。

## relay（relay_rejudge.rs）

基準は WorkUnit の base（`$CELERIS_WU_BASE`。tree は wu-region を統合した HEAD と同じ）。task 全体の基準は `git merge-base HEAD main`。
検査は提出時の HEAD（作業ツリー）で実行した。

### 結論

- `relay_rejudge.rs` から `impl CosChatLaunch` と `&mut Dispatcher`・`&task_core::SqliteStore` の引数を消した。
  E3 の 7 token は comment を含めて 14 → 0。残したのは store・設定・`Dispatcher`・`CosChatLaunch` に触らない関数
  `covers`・`fresh`・`pick`・`plan_per_thread` と型の別名 `RelayPick`（状態なし、`pub(super)`）だけ。
- `relay_rejudge`・`plan_rejudge` は同じ名と引数で `launch.rs` の `impl CosChatLaunch` に 1 つずつある。claim と起動の共通部
  `launch_rejudge` も同じ本体のまま `launch.rs` へ移した（private）。`tick_cos_chat_launch` の呼び出しは変えていない。
- dispatcher/ の self.store を書く file は 40 → 39、self.config は 33 → 32（WU の base 比）。crate 全体も同じだけ減り、
  非試験 file 数は 69 のまま、SqliteStore を書く file は 50 → 49。
- `git merge-base HEAD main`（f85df020 系）比では wu-region と合わせて self.store 44 → 39（−5）、self.config 35 → 32（−3）。
  task の閾値（−5・−3）にこの時点で届いている。slot の分はこれに上乗せになる。

### 移した操作（旧い場所 → 新しい場所）

| 操作 | 旧い場所 | 新しい場所 |
|---|---|---|
| `CosChatLaunch::relay_rejudge`（名・引数は同じ。呼び出し元 `tick_cos_chat_launch` は変えない） | `relay_rejudge.rs` の `impl CosChatLaunch` | `launch.rs` の `impl CosChatLaunch`（`claim_queued` の直後） |
| `CosChatLaunch::plan_rejudge`（同上） | `relay_rejudge.rs` の 2 つ目の `impl CosChatLaunch` | `launch.rs` の `impl CosChatLaunch` |
| claim と起動 `launch_rejudge`（空き thread・枠の確認 → claim〈store が 1 transaction で item と run を結ぶ今の経路〉→ `chat_run_get` → `run_configs.insert(…, self.config.clone())` → `launch_routes`） | `relay_rejudge.rs` の private method | `launch.rs` の private method（本体は同じ） |
| 候補の読み出し `self.store.cos_triage_relay_candidates(now)`・`self.store.cos_triage_plan_rejudge_candidates()` | `relay_rejudge.rs` | `launch.rs` の `relay_rejudge`・`plan_rejudge` |
| item の task の読み出し `task_ops::human_inbox::item_task_id(self.store.as_ref(), …)`（候補ごとに 1 回、同じ順） | `relay_rejudge.rs` | `launch.rs` の `relay_rejudge` |
| 受信箱の組み立て `human_inbox_touching`（scoped）/ `human_inbox_items`（full）と `relay_inbox_builds` の加算 | `relay_rejudge.rs` | `launch.rs` の `relay_rejudge`（条件と加算は同じ） |
| 走っていない thread と判定済みの集合で候補を選ぶ | `relay_rejudge` の中の `filter` 2 つ | `relay_rejudge.rs` の純粋関数 `fresh(candidates, running, seen)` |
| inbox の item から thread ごとの `RelayRejudge` と判定済みにする組を作る | `relay_rejudge` の中の loop（`rejudge_settled` に直接 insert） | `relay_rejudge.rs` の純粋関数 `pick(fresh, inbox) -> RelayPick`。入口が返った組を `rejudge_settled.extend` する |
| plan の候補の thread ごとの束ね | `plan_rejudge` の中の loop | `relay_rejudge.rs` の純粋関数 `plan_per_thread(candidates, running)` |
| `covers` | `relay_rejudge.rs`（private） | `relay_rejudge.rs`（private のまま） |

振る舞いを変えていない点:

- store の読み書きの回数と順序は同じ: 候補の読み出し → 候補ごとの `item_task_id` → 受信箱 1 回（scoped か full）→ thread ごとの claim・`chat_run_get`。
  失敗は今と同じく `Err(String)` で返り、`tick_cos_chat_launch` が warn する。
- `rejudge_settled` に入る組は同じ（閉じた待ちの item の全 relay と、item の task の外の relay）。旧くは loop の中で 1 組ずつ insert し、
  新しくは `pick` が返した組を launch の前にまとめて `extend` する。集合なので順序は結果を変えず、launch の前に入る点も同じ。
- `plan_rejudge` の thread の束ねは、旧くと同じく `BTreeMap` で thread 順、item は store の順。

### before/after

C3（1 行の `self.store`・`self.config`）、E1（改行をまたぐ `self\s*\.\s*store`）、E3（7 token: self.store self.config SqliteStore Dispatcher
CosChatLaunch(Config) DispatchConfig `use super::*`。comment の行も数える）。

| file | 行数 | C3 self.store（file 内の出現） | C3 self.config | E1 self.store | E3 |
|---|---|---|---|---|---|
| `cos_chat/relay_rejudge.rs` | 160 → 87 | 2 → 0 | 1 → 0 | 5 → 0 | `5 1 1 4 3 0 0`（計 14）→ `0 0 0 0 0 0 0`（計 0） |
| `cos_chat/launch.rs`（入口） | 1086 → 1170 | 15 → 17 | 3 → 4 | 24 → 29 | `24 3 6 6 9 0 0`（計 48）→ `29 4 7 9 9 0 0`（計 58） |

guard と同じ数え方（`//` の comment を除き空白を詰めた source に `\Q…\E` で数える）でも、`relay_rejudge.rs` の 7 token はすべて 0。

C3・E2 の file 数（`git grep` の作業ツリー検索。非試験 file）:

| 数 | WU の base → after | `git merge-base HEAD main` → after |
|---|---|---|
| dispatcher/ の非試験 file | 69 → 69 | 69 → 69 |
| dispatcher/ self.store の file / 出現 | 40 / 633 → 39 / 633 | 44 → 39 |
| dispatcher/ self.config の file / 出現 | 33 / 220 → 32 / 220 | 35 → 32 |
| crate self.store の file | 49 → 48 | 53 → 48 |
| crate self.config の file | 38 → 37 | 40 → 37 |
| crate SqliteStore の file | 50 → 49 | 50 → 49 |
| dispatcher/ SqliteStore の file | 27 → 26 | — |

出現数（C3）は同じ: `relay_rejudge.rs` の 1 行の参照 2（`item_task_id(self.store.as_ref(), …)`・`claim(&self.store, …)`）と
config 1（`self.config.clone()`）がそのまま `launch.rs` へ移った。

### health（module の本体の行）

`python3 scripts/dev/repo-health/repo_health.py --repo "$(pwd)" --base $CELERIS_WU_BASE --head worktree --policy-version 1` → exit 0、
`verdict=pass`、`fail_grade=0 new=0 worsened=0`。module `crates/task-dispatch/src/dispatcher` の本体の行の合計（空行・注釈・use を除く）は
32332 → 32331（−1）。repo-health の `_logic_lines` で 2 file の合計は 1050 → 1049（`relay_rejudge.rs` 128 → 59、`launch.rs` 922 → 990）。
純粋関数の signature で増えた分は、`plan_rejudge` の束ねを `for` の式に直接書く・`keyed` の組を作らずに `item_task_id` の列だけを持つ、で相殺した
（cos_chat/ の他の file は詰めていない）。

### 検査（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(rejudge) | test(relay) | test(cos_chat_triage) | test(store_access_guard) | test(structure_guard)'`
  → 156 passed。`cos_chat_triage_relay_after_escalation_rejudges_covered_items_after_the_running_run`・
  `cos_chat_triage_relay_for_a_closed_wait_builds_the_inbox_once`（scoped / full の加算と判定済みの組）・
  `cos_chat_triage_answered_decision_rejudges_the_escalated_plan_approval_once` を含む。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（`repo-health: verdict=pass exit=0 … fail_grade=0 new=0 worsened=0`、
  対象 crate: celeris celeris-campaign celerisctl llm-proxy task-dispatch、nextest 3935 passed・11 skipped、doc test は失敗 0）。
  末尾の `rm: cannot remove …/own-eval: Permission denied` は scratch の後片付けの出力で、exit code に影響していない。
- `git diff $CELERIS_WU_BASE -- crates/task-dispatch/src/dispatcher/tests crates/task-dispatch/src/dispatcher/worker_finish.rs crates/task-dispatch/src/health_review crates/task-dispatch/src/execution_coordination.rs` → 差分なし。
- 試験は足していない・弱めていない（method の名と引数を変えないので、既存の振る舞いの試験がそのまま新しい経路を通る）。

### 未解決・次の候補

- `store_access_guard_cos_chat` の試験は guard の WorkUnit が structure_guard.rs に足す（この WorkUnit は dispatcher/tests/ を変えない）。
- `docs/architecture-map.md` の cos_chat の行の入口の記述は close の段で直す（ADR の撤去対象）。

## slot（slot_wait.rs）

基準は WorkUnit の base（`$CELERIS_WU_BASE`。tree は relay の後の HEAD と同じ）。task 全体の基準は `git merge-base HEAD main`。
検査は提出時の HEAD（作業ツリー）で実行した。

### 結論

- `slot_wait.rs` から `impl CosChatLaunch` と `&mut Dispatcher` の引数を消した。E3 の 7 token は comment を含めて 13 → 0。
  残したのは `SLOT_WAIT_TEXT`・`SlotWait`・`SlotWaits` と、値だけで次の手を決める `SlotWaitStep`（`Drop`・`Close { state, prelaunch, text }`・
  `Hold`・`Launch`）・`step(run, thread_running, max_wall_secs, gate, now)`。`step` は run の状態・`launched_at`・`reason`（`stop_intent`）・
  `started_at` と、thread で run が走っているか、run の設定の `max_wall_secs`、gate の値だけを読む。
- `hold_slot_wait` は `routes.rs`（呼び出し元 `launch_routes` と同じ file。private にした）、`retry_slot_waits` は `launch.rs`
  （呼び出し元 `tick_cos_chat_launch` と同じ file）、`close_unlaunched` は `control.rs` の `close_stopped` の直後の `impl CosChatLaunch` に、
  名と引数を変えずに 1 つずつ置いた。
- dispatcher/ の self.store を書く file は 39 → 38、self.config は 32 → 31（WU の base 比）。crate 全体も同じだけ減り、
  非試験 file 数は 69 のまま、SqliteStore を書く file は 49 → 49。
- `git merge-base HEAD main`（f85df020 系）比では wu-region・relay と合わせて self.store 44 → 38（−6）、self.config 35 → 31（−4）。

### 移した操作（旧い場所 → 新しい場所）

| 操作 | 旧い場所 | 新しい場所 |
|---|---|---|
| `CosChatLaunch::hold_slot_wait`（待ちの表への記録と `chat_run_status(Waiting, SLOT_WAIT_TEXT…)`。名・引数は同じ） | `slot_wait.rs` の `impl CosChatLaunch`（`pub(super)`） | `routes.rs` の `impl CosChatLaunch`（`launch_routes` の直後、private） |
| `CosChatLaunch::retry_slot_waits`（`chat_run_get`・`run_configs.get(…).unwrap_or(&self.config)`・gate の読み出し、`launch_routes`、起動後の `chat_run_status(Working, …)`・`chat_run_get` の再読み。名・引数は同じ） | `slot_wait.rs` の `impl CosChatLaunch` | `launch.rs` の `impl CosChatLaunch`（`claim_queued` の前） |
| `CosChatLaunch::close_unlaunched`（`chat_run_finish_prelaunch`→`chat_run_finish`、Failed のときの system message、credential の revoke、`digest_due`。名・引数は同じ） | `slot_wait.rs` の `impl CosChatLaunch`（private） | `control.rs` の `close_stopped` の直後の `impl CosChatLaunch`（`pub(super)`） |
| 待ちの判定（終端・起動済み・走っている thread → Stopping → 期限 → gate） | `retry_slot_waits` の中の `if` の列 | `slot_wait.rs` の純粋関数 `step` が `SlotWaitStep` を返し、入口が `match` で書く |
| run の開始からの秒（`started_at` が無い・読めないときは 0） | `slot_wait.rs` と `routes.rs` の `launch_routes` に同じ式が 2 つ | `routes.rs` の純粋関数 `run_elapsed`（`step` と `launch_routes` が呼ぶ） |

振る舞いを変えていない点:

- 判定の順序は旧いのと同じ（終端・起動済み・走っている thread・Running/Stopping 以外 → Stopping → 期限 → gate）。gate（`accepting_new_work`・
  `disk_ready`・`config.enabled`・`has_capacity`）は旧くは期限の後に短絡で読み、新しくは `step` の前に読む。4 つとも field と件数を読むだけで書かないので、
  読む時点の違いは結果を変えない。
- store の読み書きの回数・順序は同じ: wait ごとに `chat_run_get` → （Drop なら表から外す｜Close なら `close_unlaunched`｜Launch なら `launch_routes` →
  走っていれば表から外して `chat_run_status(Working)`、走っていなければ `chat_run_get` の再読みで終端か読めなければ外す）。
  `close_unlaunched` の本体は変えていない（`finish_prelaunch` がどの Err でも `chat_run_finish` に落ちる、Failed のときだけ system message、
  credential の revoke、`digest_due`）。
- 巡回は旧くは key の列を写して `get` し直し、新しくは表の写しを回す。巡回の中で表から外れるのは今の run だけで（Drop・`close_unlaunched`・起動後の外し）、
  `launch_routes` が呼ぶ `hold_slot_wait` は待っている run には何も書かないので、他の run が足されたり外れたりしない。回る run と値は同じ。

### 行数を相殺した詰め（cos_chat/ の中、振る舞いを変えない）

- `launch_routes` の `let mut cfg = config;`（何もしない代入）を消し、`let mut cfg = …clone();` に直接書いた。
- `launch_routes` と `step` の同じ式（run の開始からの秒）を `routes.rs` の `run_elapsed` 1 つにした。`retry_routes` の期限の式は `started_at` が無いときに
  期限切れにしない（`is_some_and`）ので意味が違い、まとめていない。
- `retry_slot_waits` は thread id を `&str` に束ねて、`run_configs.get`・`launch_routes` の呼び出しを 1 行に収めた。

### close の 3 関数の比較（併合しない）

| 観点 | `close_unlaunched`（control.rs、枠待ち） | `fail_routes`（routes.rs） | `close_stopped`（control.rs） |
|---|---|---|---|
| 呼ぶ所 | `retry_slot_waits`（Stopping・期限） | `retry_routes`・`launch_routes`（期限・route なし・起動失敗） | `retry_routes`・`fail_routes`（Stopping の run） |
| 閉じる state | 引数（Stopped・Interrupted・Failed） | 常に Failed | `stop_intent` で Stopped か Interrupted |
| run の再読み | しない | する。Stopping なら `close_stopped` に任せて戻る | しない（呼び出し元の run） |
| 起動前の閉じ方 | `chat_run_finish_prelaunch(state, prelaunch, text)` | 先に `chat_run_append_text(failure_text)`、次に `chat_run_finish_prelaunch(Failed, prelaunch, reason)` | Stop は `chat_run_recover(HumanStop, reason か "stopped by human")`、Interrupt は `chat_run_finish_prelaunch(Interrupted, StoppedBeforeLaunch, "interrupted")` |
| 落ちたときの扱い | どの Err でも `chat_run_finish(state, None, text)` | `Conflict` だけ `chat_run_finish(Failed, None, reason)`、他の Err は warn | `Conflict` だけ下の通常の終わりへ、他の Err は `Err` を返す |
| 起動済みの run | 扱わない（`step` が Drop にする） | sink があれば `sink.finish(Failed, final_text)`、無ければ `chat_run_finish(Failed, Some(text), reason)` | `chat_run_finish(state, None, run.reason)` |
| 通知 | Failed のときだけ system message（key `cos-route:<run>:slot-wait-expired`、文「CoS が返事できませんでした。理由: <text>」） | 自分では出さない（呼び出し元の `route_notice` が key `cos-route:<run>:<route>:<kind>` で出す。文は `failure_text` の 2 文） | 出さない |
| credential の revoke | する | する（Stopping で戻るときはしない） | しない |
| `digest_due` | 立てる | 立てる（Stopping で戻るときは立てない） | 立てない |
| 枠待ちの表 | 外す | 触らない | 触らない |

state の決め方・Stop の閉じ方（`chat_run_recover` か `finish_prelaunch` か）・Interrupt の文（`run.reason` か `"interrupted"`）・落ちたときの範囲（全 Err か
`Conflict` だけか）・通知の key と文・revoke と `digest_due` の有無が違う。同じ仕様ではないので 1 つにまとめていない（ADR の「意味の違うものをまとめず
両方残し、違いを進捗に書く」）。

### before/after

C3（1 行の `self.store`・`self.config`）、E1（改行をまたぐ `self\s*\.\s*store`）、E3（7 token。comment の行も数える）。WU の base → 作業ツリー。

| file | 行数 | C3 self.store（file 内の出現） | C3 self.config | E1 self.store | E3 |
|---|---|---|---|---|---|
| `cos_chat/slot_wait.rs` | 194 → 96 | 6 → 0 | 1 → 0 | 8 → 0 | `8 1 0 2 2 0 0`（計 13）→ `0 0 0 0 0 0 0`（計 0） |
| `cos_chat/routes.rs` | 289 → 320 | 10 → 11 | 2 → 2 | 11 → 12 | `11 2 0 3 2 0 0`（計 18）→ `12 2 0 3 2 0 0`（計 19） |
| `cos_chat/launch.rs`（入口） | 1170 → 1215 | 17 → 19 | 4 → 5 | 29 → 32 | `29 4 7 9 9 0 0`（計 58）→ `32 5 7 10 9 0 0`（計 63） |
| `cos_chat/control.rs` | 327 → 367 | 10 → 13 | 1 → 1 | 14 → 18 | `14 1 3 2 2 0 0`（計 22）→ `18 1 3 2 3 0 0`（計 27） |

E1 の self.store は `slot_wait.rs` の 8 が routes 1・launch 3・control 4 に移り、合計は同じ。guard と同じ数え方（`//` の comment を除き空白を詰め、
`\Q…\E` で数える）で `slot_wait.rs` の self.store・self.config・SqliteStore・Dispatcher・CosChatLaunch・DispatchConfig・`use super::*`・`impl CosChatLaunch` はすべて 0。

C3・E2 の file 数（`git grep` の作業ツリー検索。非試験 file）:

| 数 | WU の base → after | `git merge-base HEAD main` → after |
|---|---|---|
| dispatcher/ の非試験 file | 69 → 69 | 69 → 69 |
| dispatcher/ self.store の file / 出現 | 39 / 633 → 38 / 633 | 44 / 633 → 38 / 633 |
| dispatcher/ self.config の file / 出現 | 32 / 220 → 31 / 220 | 35 / 218 → 31 / 220 |
| crate self.store の file | 48 → 47 | 53 → 47 |
| crate self.config の file | 37 → 36 | 40 → 36 |
| crate SqliteStore の file | 49 → 49 | 50 → 49 |
| dispatcher/ SqliteStore の file | 26 → 26 | — |

### health（module の本体の行）

`python3 scripts/dev/repo-health/repo_health.py --repo "$(pwd)" --base $CELERIS_WU_BASE --head worktree --policy-version 1` → exit 0、
`verdict=pass`、`fail_grade=0 new=0 worsened=0`、`deps.cycle`・`size.responsibility-growth` とも pass。module `crates/task-dispatch/src/dispatcher` の
本体の行の合計は 32331 → 32329（−2）。repo-health の `_logic_lines` で 4 file の合計は 1690 → 1688（`slot_wait.rs` 161 → 56、`routes.rs` 266 → 292、
`control.rs` 273 → 310、`launch.rs` 990 → 1030）。
