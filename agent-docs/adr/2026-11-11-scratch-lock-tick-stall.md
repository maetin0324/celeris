# scratch のロック待ちを tick から除く

- 日付: 2026-11-11
- 状態: Accepted（実装中）
- 関連: [ADR-0075](0075-tiered-build-cache.md)、[ADR-0129](0129-host-sccache-reflink-targets.md)

## 実装前 brief

1. 根本原因: seed の全体コピーが pool の排他ロックを保持する。maintenance・通常 GC・seed GC は tick 内で同じ flock を待つ。checks 用の `check_cargo_target_env` と評価試行の割り当ても同期コピーを呼ぶ。通常 run の `worker_task` は既に spawn_blocking 経由である。handoff は要求時刻だけを保存し、要求元が終了しても古い active に要求が残る。
2. 責務 owner: pool の排他は task-worker::scratch、seed の生存は scratch::seed、tick の掃除は task-dispatch::scratch_gc と dispatcher::maintenance。割り当ての実行場所は dispatcher::housekeeping・eval_trials。handoff の判断は celeris::instance、要求の永続化は task-core::store::instances。
3. 既存の再利用先: SeedCopyOps の偽コピー、seed の claim、通常 run の spawn_blocking、instance_set_role_if の transaction 内判定、DaemonSnapshot.last_tick_at。
4. 選択肢: ロック待ちの上限を延ばす案は tick 停止を残すため採らない。掃除は try-lock が失敗したら次回へ回す。割り当ては tick 外で準備し、完了した結果だけを使う。コピー時は pool ロックと別の参照保護を持ち、GC は使用中の seed と owner を退避しない。
5. 新しい状態・分岐・fallback・port: try-lock の Busy 分岐を追加する。maintenance は Ready のまま残す。コピー保護は process 終了で解放する lock を使い、永続 task 状態をメモリへ移さない。handoff の要求元は既存の行（自分より新しく drain していない standby・active）から導き、新しい列は足さない。drain と要求の破棄は transaction 内で行の存在と pid の生存を再確認してから書く。新しい fallback・port は無し。
6. 撤去対象: tick の pool 待機、pool を保持した全体コピー、要求元を検証しない drain。既存の失敗時の seed 不使用契約は変更しない。
7. 不変条件: active の単独性、events の追記専用、実行中 target とコピー中 seed の保護を維持する。別 fd の pool lock を保持したまま掃除が戻る試験、偽コピーを gate で止める試験、要求元の削除・死亡・生存の試験で確認する。既存試験は競合する lock と消えた要求元を作らないため検出できない。

## 決定

- tick から呼ぶ掃除は `Pool::try_lock` を使う。Busy は失敗や完了にせず、次回に回す。maintenance は Dispatch 遷移より前に判定する。
- 待機できる経路の pool lock は、呼び出し位置と待機秒数をログに残す。tick の非待機経路はスキップ理由をログに残す。
- seed のコピーは tick 外で行い、全体のコピー・走査・検証の間は pool lock を保持しない。pool lock の下では世代の固定（`pin_current_seed`。世代の dir の `.copy-pin` に共有 flock）と lease の書き込みだけを行い、写しの後に lock を取り直して target の記録と lease を書く。
- seed の GC（`retire_stale_generations`・登録外 repo の退避）は `.copy-pin` が持たれている世代と、その世代を持つ repo を退避せず次回に回す。
- 同じ owner の割り当ては owner の dir の `.alloc.lock`（排他 flock）で直列にする。pool lock より先に、外で取る（lock の順序は owner → pool。pool lock を持って owner の lock を待つ経路は無い）。owner の GC（`scratch_gc::execute`）は `.alloc.lock` が持たれている owner を消さない。
- handoff の要求元は新しい列を足さず、既存の行から導く: 自分より新しく（`started_at`, `instance_id`）、drain しておらず、pid が生きている standby か active の行。要求を書くのは起動時の standby と、古い active を見つけた新しい active だけで、どちらもこの条件を満たす。standby は要求の前に自分の行を書く。
- active は draining を `instance_set_role_if` の transaction の中で要求元を確かめてから書く。要求元が居なければ、同じく transaction の中で確かめてから `instance_register_if` で自分の行を書き直して要求を消し（`DropStaleHandoff`）、active に残る。promote.sh が新 daemon を止めて行が消えた後に残った要求は、古い active の次の tick で消える。
- 最後の tick の時刻は既存の `DaemonSnapshot.last_tick_at`（`GET /daemon`）を流用する。新しい時計の正本を作らない。handoff の要求を処理した log（draining・要求の破棄）に、要求からの経過秒数 `request_age_secs` を出す。

## 実装状況

実装済み: 掃除の try-lock 化、pool lock を持たない seed の写し（世代の pin・owner の `.alloc.lock`）、handoff の要求元の確認、`request_age_secs` の log。

未実装: `housekeeping::check_cargo_target_env` と評価試行（`eval_trials`）の割り当て・touch・release・set_ttl を tick の外へ移すこと。写しは pool lock を持たなくなったが、この 2 経路は tick の中で写しそのものを同期に待つ。
