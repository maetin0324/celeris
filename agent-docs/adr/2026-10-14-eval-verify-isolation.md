# ADR 2026-10-14: verify は評価試行に触れない・試行ファイルの削除は root の配下に限る・live 昇格で走行中の評価試行は旧 instance が終端まで走らせる

---
tasks: [01M4WSXTXWQEJVJFT1JF1VQ19K]
---

- 日付: 2026-10-14
- 状態: 採用
- 関連: ADR-0040 D3（verify は本番のコピーの検証専用）・D4（ライブ引き継ぎと draining）、ADR-0041 D5（verify の煙試験と
  `set_eligible_tasks`）、ADR-0070 付記 F5-fix6（停止時の `interrupt_runs_on_shutdown`）、
  ADR 2026-10-03-ownerless-running-runs（lease を持たない run の行）、
  ADR 2026-10-11-llm-source-model-routing-evaluation 付記 2026-10-12（eval-dispatch）D4、
  ADR 2026-10-14-eval-trial-scratch-target、ADR-0139（verify が本番 port に相乗りした先例）

## 1. 文脈

2026-10-14 08:52Z、本評価（batch `01M4W9H8…`）の走行中に delivery の verify（release `de94e591f80d`）が走った。
`verify.sh` は本番 DB の写し `staging.sqlite3` と**本番の config**で `celeris --mode verify` を起こす
（`--workspace-root` だけ staging）。

1. **verify が本番の試行ファイルを消した。** verify の daemon は `set_accepting_new_work(true)` で tick を回すので、
   `tick` → `tick_eval_trials` → `reconcile_eval_trials` が走る（`crates/task-dispatch/src/dispatcher/eval_trials.rs`）。
   写しの DB には本番の `running` の試行がそのまま入っており、verify の `eval_runs`（手元の評価 run）は空なので、
   `eval_recovery_action(Running, tracked = false)` が `Error` を返す。`fail_and_cleanup_eval_trial` →
   `remove_eval_trial_files` は DB の `worktree_path`（本番の `/local/celeris/data/workspaces/eval/<trial>/repo`）を
   `git worktree remove --force` と `remove_dir_all` で消し、`remove_eval_target` は config の scratch pool（本番の
   `/local/celeris/data/scratch`）の `targets/agent-eval-<trial>` を消して lease を release した。
   削除先の境界の検査は無い（`--workspace-root` は staging なのに、DB の絶対パスを信じた）。
2. **live 昇格で走行中の試行が error になり、`runs` 行が running のまま残った。** 08:53Z の昇格
   （`3325c40d6db1` → `de94e591f80d`）で:
   - `Dispatcher::in_flight` は `eval_runs` を数えない。旧 instance の drain は評価 run を待たずに `Step::Drained` で
     exit する。drain の終了では `interrupt_runs_on_shutdown`（`abort_eval_runs` を含む）を呼ばない（`run.rs` は
     `Exit::Signal` のときだけ呼ぶ）。旧 instance の評価 run は採点も `runs` 行の終端も書かれないまま消える。
   - 新 instance は active になった最初の tick から `reconcile_eval_trials` を回し、自分の `eval_runs` に無い
     `running` を持ち主の生死を見ずに `error`（"daemon recovery: run disappeared while trial was running"）にして
     ファイルを消す。旧 instance がまだ走らせている間でも同じ（draining の旧 instance の作業ツリーを消しうる）。
   - 回復の `Error` は `eval_trials` を終端にするだけで `runs` 行を閉じない。`reconcile_ownerless_runs` は
     role `eval` の行を対象外にしている（eval-dispatch 付記 D4「生死は `eval_trials` が持つ」）。誰も閉じない。

## 2. 決定

### D1. 評価試行を扱うのは自分の `daemon_instances` 行を持つ instance だけ（verify は触れない）

評価試行の起動（`tick_eval_trials` の claim）・回復（`reconcile_eval_trials`）・後始末（`remove_eval_trial_files`・
`remove_eval_target`・`abort_eval_runs`）・`eval_trials` / `runs`（role `eval`）/ scratch lease への書き込みは、
dispatcher が自分の instance id（`set_orphan_takeover` 済みの `OrphanTakeover`）を持つときだけ行う。

- `--mode verify` は `daemon_instances` の行を持たず `set_orphan_takeover` を呼ばない（ADR-0040 D3）ので、
  `tick_eval_trials` は何も読まず何も書かず 0 を返す。写しの DB の評価試行・lease・`runs` にも、DB に書かれた
  本番の絶対パスにも触れない。celerisctl・設定を渡さない組み立ても同じ。
- 判定は「instance id を持つか」の構造で行い、`DaemonMode` を見る分岐や config の欄（`reserved_runs = 0` を
  verify で上書き等）では閉じない。D3 の所有の記録に instance id が要るので、id の無い dispatcher は試行を
  起動できない、と同じ規則になる。
- `tick` の呼び出し側の条件（`accepting_new_work && disk_ready`）は変えない。回復は standby・verify では走らず、
  draining の旧 instance は自分の評価 run の完了の記録（`on_eval_worker_finished`）だけを行う（D3）。
- 試験の組み立ては評価試行を回すなら `set_orphan_takeover` を呼ぶ。

### D2. 試行ファイルの削除は daemon 設定の root の配下に限る

- 作業ツリー: `remove_eval_trial_files` は `worktree_path` を、daemon の `workspace_root` の `eval/<trial_id>/` の
  配下（起動時に `start_eval_trial` が作る `<workspace_root>/eval/<trial_id>/repo`）にあるときだけ消す。比較は
  正規化した path（`..`・symlink を解いた実 path。対象が既に無ければ親の実 path + 名前）で行い、`trial_id` の
  dir 名が一致しないものも拒む。外なら `git worktree remove` も `remove_dir_all` もせず、`warn` を出して
  `eval_trials.error` は書き換えない（試行の終端化は続ける。拒んだ path はファイルを残す側に倒す）。
- target: `remove_eval_target` は `pool.target_dir(agent-eval-<trial_id>)`（設定の scratch pool の
  `targets/` の直下）だけを消す。pool の root が空・相対なら消さない。DB の値から target の path を組み立てない
  （今も組み立てていない。試験で固定する）。
- 試行の `worktree_path` / `repo` は DB の値なので、`git -C <repo> worktree prune` 等で repo 側を書き換えるのも
  同じ境界の内側のときだけ。

D1 があれば verify はそもそもこの経路に来ない。D2 は D1 の外側の守り（写しの DB を別 root で開いた daemon、
手で編集された行、将来の別経路）で、D1 と独立に試験する。

### D3. live 昇格: 走行中の評価試行は旧 instance が終端まで走らせ、新 instance が採点する

採るのは「旧 instance が終端まで走らせる」。新 instance への引き継ぎ（実行中の process を新 instance が抱え直す）は
採らない（§3）。

1. **所有の記録**: `eval_trials` に `instance_id TEXT`（試行を起動した instance。migration `0078`）を足す。
   `start_eval_trial` が `running` にするとき自分の instance id を書く（claim と同じ更新で書けるならそこで）。
   終端・`queued` に戻すときは消さない（記録として残す。回復の判定は `running` の行だけを見る）。
2. **drain は評価 run を待つ**: `Dispatcher::in_flight` に `eval_runs.len()` を足す。旧 instance は評価 run の完了まで
   draining に留まり、`on_eval_worker_finished` で `runs` 行を閉じて試行を `scoring` にしてから `Step::Drained` で
   exit する。評価 run は `EVAL_MAX_WALL_SECS`（1800 秒）で必ず終わるので、drain の延びはこれが上限
   （`drain_force_abort = false` の既定では本番の run の方が長い）。draining の instance は新しい試行を起動しない
   （`tick_eval_trials` は `accepting_new_work` のときだけ）。
3. **採点は新 instance**: `scoring` の試行は従来どおり active の `reconcile_eval_trials`（`EvalRecoveryAction::Score`）
   が採点する。旧 instance は draining 中に採点しない（hidden check は数分かかり、新 instance と二重に走らない
   ようにする。`scoring` の行の採点の排他は従来の `eval_trial_finish` の状態遷移に任せる）。
4. **回復の条件（新 instance が旧の試行を error にしない）**: `reconcile_eval_trials` は `running` の試行を、
   手元の `eval_runs` に無いことに加えて、**記録された持ち主が生きていない**ときだけ `Error` にする。
   持ち主の生死は `daemon_instances` の行で、`orphan::holder_gone` と同じ定義（role が active / draining、
   `drained_at` が無い、heartbeat が新しい、pid が生きている）を**その 1 instance に**当てる。持ち主が自分なら
   手元に無い＝失われた run として `Error`。`instance_id` が NULL の行（migration 前に起動した試行）は
   「他に生きている instance が 1 つも無い」（`holder_gone`）ときだけ `Error`。生きている持ち主の試行は、
   `started_at + EVAL_MAX_WALL_SECS + check_timeout_secs + lease_grace` を過ぎても `running` なら `Error` にする
   （持ち主が閉じ忘れた行の上限。ownerless-running-runs D3 の期限と同じ考え方）。
5. **取り残しの `runs` 行を残さない**:
   - 回復で `Error` にした試行は、同じ処理で `run_id` の `runs` 行を `cancelled`（`run_index_finish`、
     理由 `interrupted: eval recovery: …`）で閉じる。
   - `reconcile_ownerless_runs` の role `eval` の除外を、次の照合に置き換える: running の `eval` 行のうち手元に無く、
     対応する試行（`eval_trials.run_id` で引く）が終端・`scoring`・無い、または試行の持ち主が生きていない
     ものを `cancelled` で閉じる。これで本番 DB に既に残っている 08:53Z の 3 行（試行は `error`）も、この release が
     active になった最初の照合で閉じる。手で DB を書き換えない。
6. **停止（SIGTERM）と drain の打ち切り**: `interrupt_runs_on_shutdown` の `abort_eval_runs`（試行を `error`
   "dispatcher stopped the run"、target を消す、`runs` 行を `cancelled`）は変えない。`DrainTimedOut` の
   `abort_all_runs` も `abort_eval_runs` を呼ぶ（今は評価 run を止めず、`runs` 行も試行も残る）。

旧い挙動（持ち主を見ずに回復で error にする、`in_flight` が評価 run を数えない、回復の error が `runs` 行を
残す、ownerless 照合の `eval` 除外）は残さない。

### D4. 本番への移行

- schema: migration `0078`（`eval_trials.instance_id` を足す。既存行は NULL）。昇格で自動に当たる。
- config・手での DB 書き換えは無い。取り残しの `runs` 行は D3.5 が閉じる。
- 08:52Z に消えた試行（`error`）の再実行は評価の運用（batch の再投入）で行い、この ADR の範囲外。

## 3. 採らない

- **新 instance に走行中の評価 run を引き継ぐ**: 評価 run は旧 process の tokio task と子 process で、新 instance が
  途中から抱え直す手段が無い（worker run も lease の失効を待って作り直す）。作り直しは試行の wall time と
  費用を捨て、評価の測定値（経過時間・usage）も壊す。
- **drain で評価 run を止めて `queued` に戻す**: 昇格のたびに走行中の試行が捨てられ、昇格が多い日は評価が進まない。
  評価 run の上限は 30 分で、drain が待てる長さ。
- **verify の判定を `DaemonMode` / config の上書きで行う**: verify で起こさない裏方を 1 つずつ足す形になり、
  新しい段が入るたびに漏れる（今回の漏れ方）。instance id の有無は「本番の持ち主として記録を書ける者」の構造。
- **`runs` 行に持ち主の instance を書く**: ownerless-running-runs §採らない と同じ理由。持ち主は `eval_trials` に書く。

## 4. 受け入れ条件と試験（実装の葉が置く）

- verify と同じ組み立て（`set_orphan_takeover` 無し）の dispatcher が、写しの DB の `running` の試行・`scoring` の
  試行・`queued` の試行に対して tick を回しても、試行の状態・`runs` 行・lease・作業ツリー・target が変わらない。
- `worktree_path` が `workspace_root` の外（別の一時 dir）を指す試行を回復で `Error` にしても、その dir が残る。
  root の配下のものは消える。
- 旧 instance（draining、生きている）が評価 run を持つ間、新 instance の回復は試行を `error` にせず、作業ツリーも
  消さない。旧が完了を記録すると新が採点する。`in_flight` が評価 run を数え、drain は評価 run の完了で終わる。
- 持ち主が死んだ（heartbeat が古い・pid が無い）試行は `error` になり、その `runs` 行が `cancelled` になる。
  試行が終端なのに running の `eval` 行が、active の照合で閉じる。
