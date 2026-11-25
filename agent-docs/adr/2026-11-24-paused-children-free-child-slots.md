# 自分で一時停止した子 task は同時の子 task の枠に数えず、枠の待ちを waits に出す

- 日付: 2026-11-24
- 状態: 採用
- 関連: ADR-0079（木の task。D3 の `max_parallel_child_tasks`、R1b 付記 10、R2a 付記 10、D10 の生存確認）、
  2026-10-11-task-wait-reasons（`TaskWait`）、2026-10-23-stage-order-and-plan-stall（`PlanStalled`）
- task: 01M87M5AECGN6ABZTS10S5WE0X

## 1. 事象（2026-11-24、本番）

- root 01M856X8F2DHR1RAKD7GKG6DM0（MemT）の計画 v3 の段 main に kind task の unit e2-e5-e6（子 01M86W2CVH19C96K55GR9VKE73）と
  e3-e4（子 01M86W2CW77KWE991GGBG4Y3CF）があった。人の判断で、運用者が 13:34:46Z に両方の子を `POST /tasks/{id}/pause` で
  一時停止した（子の `paused_at` あり、status は `ready`）。
- 13:49:10Z の CoS の replan で unit e1b（depends_on e1-pilot、done）と e1-critic（depends_on e1b）が足された。e1b は `ready` のまま
  20:31Z まで子 task が作られず、root には event が 1 件も出なかった。
- 原因: `reconcile_tree_units`（`crates/task-dispatch/src/dispatcher/tree_units.rs`）は `running` の kind task の unit（子を持つ unit）を
  すべて `max_parallel_child_tasks`（既定 2）に数える。一時停止した子の unit も `running` のまま数えられ、E2・E3 が枠を 2 つとも占めた。
  E1b は枠を待ち、E2・E3 は人の判断で E1b を待つ、という待ち合いになった。
- 待ちが見えなかった: `GET /tasks/{root}/execution` の `waits` は kind `children`（E2・E3）だけで、e1b が枠を待つことは出なかった。
  `plan_stall` は現在の段に ready の kind task の unit があるので出ず、生存確認（`task_core::tree::liveness`）は「同時の子 task の空きを待っている」を
  名指しの待ちに分類するので、`StallDetected` も受信箱の item も出なかった。
- 運用者は人の承認を得て 20:31Z に本番 config の `execution.tree.max_parallel_child_tasks` を一時的に 3 にし、e1b の子
  01M87M4CEXH3BSQGBFF1AA0EA4 が 20:32Z に作られた。

## 2. 決定

### D1. 枠を占める子（自分の一時停止と祖先の一時停止の区別）

同時の子 task の枠を占めるのは、`running` の kind task の unit のうち、**子 task 自身に `paused_at` が無い**ものだけにする。

- 子 task 自身に `paused_at` がある（`POST /tasks/{child}/pause` で自分が一時停止された）子は枠に数えない。人がその子を止めたのは
  「その子の仕事を今は進めない」という判断で、その間に兄弟の仕事を止める理由は無い。数えると今回のように兄弟を黙って止める。
- **祖先の一時停止で止まっている子（子自身の `paused_at` は無い）は従来どおり数える。** 子の祖先が止まっているなら、子の親（この unit を
  持つ節点）も同じ一時停止の下にある（祖先の集合は親を含む）。枠を空けて新しい子を作っても、その子も同じ祖先の一時停止で走らない。
  祖先の再開では止まっていた子がすべて一斉に走るので、数えないと再開の瞬間に上限を超える子が並ぶ。
- 判定は `task_core::wait::child_slot_holders`（純粋関数）1 か所に置き、dispatcher の子の生成（`reconcile_tree_units`）・`waits`
  （`plan_waits`）・生存確認の事実（`task_ops::tree::node_liveness_facts`）が同じ関数を呼ぶ。子が終端なら unit は次の tick で写されて
  `running` でなくなる（従来どおり）。
- 子 task の作成の候補（現在の段の `ready` の kind task の unit で、子がまだ無いもの、seq 順）も `task_core::wait::child_creation_candidates`
  1 か所に置き、dispatcher と待ちの判定が同じ並びを使う。

### D2. 一時停止を解いた子が上限を超える場合

**上限を一時的に超えてよい（再開を待たせない）。** 一時停止を解いた子はその tick から枠を占め、枠を占める子の数が上限を超えている間は
新しい子 task を作らない（`open >= limit` で作らない、従来の判定のまま）。上限を下回ったら次の tick から作る。

- 理由: 再開は人の明示の操作で、それを dispatcher が黙って保留すると、再開した子に新しい待ちの状態（「親の枠の空き待ち」）と
  その表示・解除の経路が要る（状態の追加）。上限の目的は「同時に新しく起こす子の数を抑える」ことで、人が再開した 1 件が一時的に
  超えることは目的に反しない。超えた分は新しい子の作成を止めることで自然に戻る。
- 新しい状態・分岐・設定欄は足さない。

### D3. 枠の待ちの見せ方

`TaskWait` に独立した待ち `child_slot` を足す:

```json
{"kind": "child_slot", "unit_keys": ["e1b"], "task_ids": ["01M86W2C…", "01M86W2C…"], "limit": 2}
```

- `unit_keys`: 子 task の枠を待つ unit（現在の段の `ready` の kind task の unit で子がまだ無いもの、seq 順。D1 の候補）。
- `task_ids`: 枠を占める子（D1 の規則。自分で一時停止した子は入らない。id 順）。
- `limit`: 実効の `max_parallel_child_tasks`。
- 出す条件: 候補が 1 件以上あり、枠を占める子の数が `limit` 以上のとき。枠に空きがあれば次の tick で子が作られるので出さない
  （一瞬で解ける待ちを出さない）。
- 上限は config の値なので、DB だけでは決まらない。`plan_waits` は上限を引数（`Option<usize>`）で受け取り、`None` なら `child_slot` を
  出さない（推測しない）。上限は `task_ops::wait::MemoryWaits`（DB の外から渡す待ちの材料）に持たせ、API は daemon と同じ実効の上限
  （`ApiState` の `tree_limits`）、dispatcher は自分の config を入れる。`celerisctl` と受信箱の draft の判定は従来どおり `None`。
- 既存の `children`（未完の子の完了待ち）はそのまま出す（一時停止した子も、unit は子の完了を待つ）。`child_slot` は別の待ちとして並ぶ。

### D4. 待ち合いを人に知らせるか

**受信箱に新しく知らせる経路は足さない。** 理由:

- D1 で、自分で一時停止した子は枠を占めないので、今回の待ち合い（枠を占める子がすべて自分で一時停止中で、ほかに進む run が無い）は
  起きない。枠を待つ unit は次の tick で子 task になる。
- 枠を占める子がすべて祖先の一時停止で止まっている場合、節点自身も同じ一時停止の下にあり、節点は `ready_tasks` に入らない
  （生存確認の事実 `eligible = false`、`TaskWait::Paused`）。人が自分で止めた木を「止まっている」と知らせ直すことになるので知らせない。
- 枠を占める子が 1 件でも止まっていなければ、その子が進むので待ちは解ける。

生存確認（ADR-0079 D10）の ready の kind task の unit の分類は、上の規則と同じ材料に揃える:

- 枠を待つ（D3 の `child_slot` に入る）unit は名指しの待ち（`Waiting`、reason `child_slot`）。
- それ以外の、決定を待たない ready の kind task の unit は `Runnable`（`child_creation`、次の tick で子を作る）。従来の「非終端の子が
  1 件でもあれば枠待ち」という近似（一時停止した子も数えていた）を消す。

### D5. 変えないもの

- 子 task の status と `paused_at` の意味、`POST /tasks/{id}/pause` / `resume`。
- 親が待つ委譲の子の判定（`task_ops::delegate::child_wait`）。一時停止した子も親は完了を待つ（子の成果が要る）。
- 親（または祖先）が一時停止されている間に `reconcile_tree_units` が子を作るかどうか（従来どおり。作った子は祖先の一時停止で走らない）。

## 3. 試験

- `crates/task-dispatch/src/dispatcher/tests/tree_child_slots.rs`
  - 自分で一時停止した子 2 件と ready の kind task の unit がある木で、`max_parallel_child_tasks = 2` のまま、次の照合でその unit の子が
    作られる（D1）。
  - 親が一時停止され、子 2 件が祖先の一時停止で止まっている木では、ready の unit の子は作られず、`waits` に `child_slot` が出る（D1・D3）。
  - 一時停止を解いた子で枠が 3/2 になっても、再開した子は `ready_tasks` に入り（待たせない）、次の ready の unit の子は作られず
    `child_slot` で待つ。枠が 2 を下回ったら作られる（D2）。
  - `GET /tasks/{id}/execution` と同じ材料（`task_ops::view::task_detail` の `waits`）に、待つ unit の key と枠を占める子の id を持つ
    `child_slot` が出る（D3）。
- `crates/task-core/src/wait/child_slot/tests.rs`: 枠を占める子の規則（自分の一時停止を除く）、`child_slot` の出る条件、上限 `None` で
  出さないこと。
- `crates/task-core/src/tree/r3b_tests.rs`: 生存確認で、枠を占める子がすべて自分で一時停止した節点の ready の unit が `Runnable`
  （`child_creation`）になり、枠を待つ unit は `Waiting`（`child_slot`）になる（D4）。

## 4. 採らない

- 一時停止した子の unit を `running` 以外（新しい状態）に移す: 子の状態の写しと replay に新しい状態の遷移が要り、D1 の数え方だけで足りる。
- 再開した子を枠の空きまで待たせる: D2 の理由。
- 待ち合いを `StallDetected` / 受信箱に乗せる: D4 の理由。
