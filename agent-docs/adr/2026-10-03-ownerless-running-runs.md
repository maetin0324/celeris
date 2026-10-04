# ADR 2026-10-03-ownerless-running-runs: lease を持たない run の行を running のまま残さない

---
tasks: [01M41M32MWB3AEQ0QP7C23Q3CN]
---

- 日付: 2026-10-03
- 状態: 採用
- task: `01M41M32MWB3AEQ0QP7C23Q3CN`
- 関連: ADR-0040 D4（ライブ引き継ぎと draining）、ADR-0070 D4/D5 と付記「F5-fix6」（孤児の回収）、ADR-0079 付記「R6-1」D4（終端 task の `runs` 行の照合）、ADR-0007 D5（決定的 check の後にだけ reviewer run を起こす）、ADR-0014 D1（reviewer run の `WorkerStarted` / `WorkerFinished`）

## 状況

2026-10-03 本番: reviewer run `01M4129NX8R264QXMQVEZRDKHM`（task `01M40B3GWG2XFJHC6HTRZH0RBG`、14:22:11 UTC 開始、旧 instance = release `32b19f45d8a9`）の `runs` 行が、16:01 の `0225c752ef0c` へのライブ引き継ぎの後も `status='running'`・`finished_at NULL` のまま 3 時間以上残った。処理するプロセスはどこにも無い。「running の run が 0 になるのを待って再起動する」運用が終わらなかった。

task の event 列（`celerisctl log`）で確かめた事実:

- `[896] WorkerStarted{run_id: 01M4129N…, role: Reviewer}` の直後が `[897] Transitioned{Reviewing → Ready, review_fail}` で、判定は「`not evaluated: a deterministic check failed`」。この run の `WorkerFinished` は無い。
- task はその後 replan されて `Running` に戻り、終端になっていない。

event の時刻は `celerisctl log` に出ないので、`review_fail` が 16:01 の引き継ぎの前か後かはこの task の中では確かめていない。ただし下の経路 A は引き継ぎと無関係に同じ行を残す。

2 例目（人の割り込み、同じ instance の中）: reviewer run `01M41EKM2QGD4T65F8ZDS4RFF7`（同じ task、17:57:20 UTC 開始、instance は引き継ぎ後の `0225c752ef0c` のまま）。events では `[1272] Transitioned{ready → reviewing, plan_complete}` → `[1274] WorkerStarted{run_id: 01M41EKM…, role: reviewer}` の後、28 分の決定的 check を経て `[1275] Transitioned{reviewing → ready, review_fail}` と `review_verdict` 5 件が続く。判定の `run_id` は **レビュー対象の最後の WU の worker run** `01M417R6…` で、reviewer run は起動されていない（criterion 4 は「not evaluated: a deterministic check failed」）。つまりこの行は引き継ぎと無関係に、下の経路 A で残った。回収はライブ引き継ぎに限らず、同じ instance の中でも働かなければならない。

`runs` 行が running のまま残る経路は 3 つあった。どれも **lease を持たない run**（reviewer run は lease を持たない）で起きる。lease を持つ worker run は lease の失効と孤児の回収（ADR-0070 付記 F5-fix6）が閉じる。

- **経路 A（同じ instance の中）**: `spawn_review` は review を始める前に reviewer run の `WorkerStarted`・`runs` 行・quota の枠を書く。決定的 check が落ちると reviewer は起動されず（ADR-0007 D5）、`ReviewOutcome.reviewer_run` は無い。`on_review_finished` は `reviewer_run` に名前がある run しか閉じないので、用意だけした run の行が残る。task が終端になれば R6-1 の照合が閉じるが、終端にならない間は残り続ける。
- **経路 B（SIGTERM / SIGINT の停止）**: `interrupt_runs_on_shutdown` は worker run だけを記録し、review は次の daemon の `recover_reviews` に任せていた。次の daemon は**新しい** reviewer run を起こすので、旧い reviewer run の行は誰も閉じない。draining の旧 instance を止めたときも同じ。
- **経路 C（crash など）**: 終端でない task の running の行を、持ち主がいなくなった後で閉じる照合が無かった。

task の側は既存の仕組みで進む（`Reviewing` で誰も抱えていない task は `recover_reviews` が拾う。review の per-task flock が二重の review を防ぐ）。残るのは `runs` 行だけだが、行が残る限り「running が 0」の判定が成り立たない。

## 決定

### D1. 用意だけして起動しなかった reviewer run は review の完了で閉じる（経路 A）

`on_review_finished` は、`ReviewEntry.review_run_id` があるのに `ReviewOutcome.reviewer_run` がその run を返さなかったとき、quota の枠を解放し、`WorkerFinished{role: reviewer, outcome: "interrupted: reviewer run not started (…)", end: cancelled}` を追記する。store が同じ transaction で `runs` 行を `cancelled` にする。判定の適用は変えない。

### D2. SIGTERM / SIGINT で止まる instance は手元の review も閉じる（経路 B）

`interrupt_runs_on_shutdown` は worker run に続けて手元の review を止め、reviewer run に `WorkerFinished{…"interrupted: review interrupted (daemon shutdown …)", end: cancelled}` を残す。task は `Reviewing` のまま置く。次の active の `recover_reviews` が review をやり直す（attempts は消費しない）。drain の途中で止められた旧 instance もこの経路を通る。

drain が正常に終わる場合は従来どおり: `in_flight` は review を数えるので、旧 instance は review の判定を保存してから exit する。`drain_force_abort` の打ち切りは `abort_all_runs` が reviewer run を閉じる（F5-fix3）。

### D3. 持ち主のいない running の行を、active が起動時と定期的に閉じる（経路 C）

`Dispatcher::reconcile_ownerless_runs`（`crates/task-dispatch/src/dispatcher/ownerless_runs.rs`）。判定は `runs` 行・lease・`daemon_instances`（heartbeat と pid の生死）だけで行う。LLM は呼ばない。

- 動くのは active（`accepting_new_work`）で、かつ自分の `daemon_instances` の行を持つとき（`set_orphan_takeover` 済み）だけ。`--mode verify`・celerisctl・設定を渡さない組み立てでは動かない。
- 時機は active になった最初の tick と、その後 `RUNS_RECONCILE_INTERVAL_SECS`（600 秒）ごと。時計は dispatcher の注入できる時計を使う。
- `runs` の running の行のうち、次のどれにも当たらないものが対象:
  1. この instance が run id で手元に持っている（`running`・`reviewing` の判定 run と reviewer run・`checking`）。
  2. task が無い・面倒を見ない task（ADR-0041 D5）・終端の task（R6-1 の照合の担当）。
  3. task か WU の lease を持っている（lease の失効と F5-fix6 の孤児の回収が担当。task / WU の状態もそちらが戻す）。
- 対象の行は次のどちらかで閉じる（`WorkerFinished{outcome: "interrupted: orphan_takeover: …", end: cancelled}`。role は行の role）:
  - run を抱えうる他の instance が 1 つも生きていない（F5-fix6 の `holder_gone` と同じ定義）。
  - 他の instance が生きていても、行の期限を過ぎた。期限は `started_at +`（reviewer run は `review_timeout + lease_grace`、それ以外は task の `max_wall_secs + lease_grace`）。draining の旧 instance が自分の review を見ている間は横取りしない。その instance 自身が行を閉じ忘れた場合もこの期限で閉じる。
- task は遷移させない。`Reviewing` で誰も抱えていない task は同じ tick の `recover_reviews` が review をやり直す。lease を持つ worker run の task は既存の `InfraRequeue`（attempts を消費しない）で `Ready` に戻る。

持ち主がまだ生きていた run を期限で閉じた場合、持ち主が後から書く `WorkerFinished` はそのまま追記される（行は終端のまま）。

### D5. 手元のレビューの取りこぼしは毎 tick 閉じる（同じ instance の中、引き継ぎと無関係）

D3 は「手元に無い行」だけを見る。同じ instance が `reviewing` に抱えたまま終わらないレビューは D3 の対象外なので、`Dispatcher::reap_lost_reviews`（同じ module）が毎 tick、active・draining を問わず手元のレビューを見る（残ったままだと `in_flight` が 0 にならず、drain も「running が 0 になるのを待つ」運用も終わらない）。

- **完了が届かない**: レビューの tokio task は終わっている（`JoinHandle::is_finished`）のに判定の完了（`Completion::Review`）が届かなかった（panic・abort）。完了は task が終わる前に送られ、次の tick の冒頭の `drain_completions` が拾うので、2 tick 続けて終わって見えた task だけを取りこぼしと判定する（`lost_review_watch`）。
- **期限越え**: `since + ownerless_ttl(reviewer)` を過ぎても終わらない。検査と reviewer run は各々 timeout で終わるので、越えるのは何かで固まったレビューだけ。D3 と同じ長さにして、持ち主の生きている run を早く閉じない側に倒す。`since` は実時計で記録されるので期限も同じ実時計で測る（dispatcher の差し替え時計と混ぜると、時計を進める試験で生きたレビューを期限越えと誤る。試験は `since` を差し替えて再現する）。

どちらも reviewer run を起こしていれば quota を解放し、`WorkerFinished{role: reviewer, end: cancelled, outcome: "interrupted: review lost (…)" / "interrupted: review stopped (…)"}` で `runs` 行を閉じ、レビューを止める（`stop_review`）。task は `Reviewing` のまま、`recover_reviews` がやり直す（attempts は消費しない）。

同じ instance の中での回収のまとめ: 経路 A は D1（review の完了時）、手元のレビューの取りこぼしは D5（毎 tick）、手元に無い行（旧 release が残した行を含む）は D3（起動時と 600 秒ごと）。

### D4. 人の操作

本番 DB に既に残っている行（上の run を含む）は、この release が active になった最初の tick の D3 が閉じる。手で DB を書き換える必要は無い。

## 採らない

- `runs` 行に持ち主の `instance_id` を書く: `runs` は events から再構築する派生索引（ADR-0072 D5）で、`WorkerStarted` に instance が無い。event と migration を変える割に、D3 の「lease 無し + 生きた instance 無し／期限切れ」で足りる。
- reviewer run に lease を持たせる: review は task の状態（`Reviewing`）と flock で排他している。lease を足すと `acquire_lease` の経路が 2 つになる。
- drain の終了（`Step::Drained`）でも review を打ち切る: `in_flight` が 0 になってから exit するので、手元に review は無い。

## 受け入れ条件と試験

- 旧 instance が reviewer run を抱えたまま引き継ぎ・停止したとき、その行が終端になり、新 instance が review をやり直す（`crates/task-dispatch/src/dispatcher/tests/ownerless_runs.rs`）。修正前はこの行が running のまま残る。
- どの instance にも持ち主がいない running の行を、新 instance が起動時と定期的に閉じ、task が先へ進む（同上。時計の差し替えで決定的に再現）。
- 同じ instance の中で、判定を届けずに終わったレビュー（handle の abort で模す）と期限を越えたレビュー（`since` の差し替え）が閉じられ、レビューがやり直される（同上 T2a / T2b）。
