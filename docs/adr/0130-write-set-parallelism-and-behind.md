# ADR-0130: expected/actual write-set による並列制御と target からの behind 指標

---
tasks: [01M3YE0JTQEYBFDTV3HCHR4G9J]
---

- 日付: 2026-10-02
- 状態: 採用（設計。実装は後続 WorkUnit）
- 関連: [ADR-0043](0043-workspaces.md)、[ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md)、[ADR-0079](0079-recursive-task-decomposition.md)、[ADR-0118](0118-review-target-sync-and-merge-candidate.md)、[ADR-0120](0120-pre-review-sync-integration-repair.md)

## 背景と着手時の観測

`execution_scheduler.rs` は WU の終了と工程の決着を決める純粋関数で、`dispatcher/work_units.rs` は同一 task の工程・依存・`max_parallel_work_units` と worktree 可否で次の WU を選ぶ。`capacity.rs` は CoS run と通常 run のプロバイダ枠を分ける。同じ登録元 Git repo の**別 task** の変更箇所は、いずれの gate も比較していない。ADR-0074 の WU 用 worktree は task 内で独立しており、ADR-0079 の子 task は親 branch を target とする。remote/shared/非 Git の WU は現在も直列 1 に倒す。

`task-ops/changes.rs` の `sync_onto_target` は clean な task worktree を review 前に target へ rebase し、`rebase_and_advance` は default branch の取り込み時に fast-forward する。`review.rs` の検査後 stale 判定、`dispatcher/review_verdict.rs` の ReviewRepair / IntegrationRepair、`celeris/src/delivery.rs` の merge candidate 照合は、この SHA 固定を前提にしている。write-set hint はこれらの正しさの代わりにはならない。

着手時の登録元 `main` は `7b77f17a39b37b4ae1ab847cd264110b4e05ff3a`（2026-10-02 13:09 UTC）、WU の HEAD は `db7abe3e485d`。ネットワーク fetch はせず登録元の main ref を読んだ。`git merge-tree --write-tree --name-only HEAD main` は exit 1、衝突候補は `task-api/query.rs` とその tests、`task-core/cluster_job/tests.rs`・`store/migrations.rs`・`store/tests.rs`、`task-ops/delivery.rs` とその tests、`docs/PROGRESS.md`、`gui/app/routes/inbox.tsx` の 9 ファイル。review / delivery / migration / GUI の並行変更との統合リスクは実在する。本 ADR のパスはこの一覧にない。登録元 main と全ローカル `celeris/*` ブランチ（計 158 ref）の `docs/adr` を走査した最大番号は 0129 で、0130 は未使用だった。0116 は browser launcher 用に避ける。これは着手時の snapshot であり、後続の葉は統合時に再確認する。

## 決定

### D1. expected_write_paths は明示的な粗い hint

`expected_write_paths?: string[]` を task、WU、`celeris.execution-plan/3` の `units[]` に任意で持たせる。`PlanUnitSpec` の `kind: task` は子 task の作成時に task へ引き継ぎ、`kind: leaf` は実行 WU へ引き継ぐ。v1/v2 の `WorkUnitSpec` にも任意欄を足すが、既存 JSON と空欄の直列/並列挙動は変えない。人・planner は同じ欄を書ける。replan で明示値を変えられるが、進行中の run の予約は開始時の値を保持する。

値は Git repo root からの `/` 区切りの相対パス prefix とする。末尾 `/` はディレクトリ prefix、末尾 `/` の無い値はその名前の file **または**ディレクトリ prefix とする。`crates/task-core/` は `crates/task-core/src/x.rs` に一致し、`crates/task-corex/` には一致しない。空文字、絶対パス、`.`・`..`・重複 slash、NUL は拒否する。正規化した値をソート・重複排除する。repo ごとの独立した配列はこの Phase では設けず、選択済みの各 Git repo に同じ prefix 配列を適用する。照合は共通 repo ID ごとに行う。これで複数 repo の指定は保守的になるが、repo の取り違えで衝突を見逃さない。`context.paths`、`RepairScope.allowed_paths`、acceptance の差分検査とは別物であり、権限制限にも検査範囲にも使わない。

未指定と空配列は同じ「hint なし」。旧 task の既定の実行、同一 task の WU 並列、他 task との同時実行をそのまま許し、actual から暗黙の hint を推測して gate に使わない。子 task は自分の明示値を優先し、無ければ親 unit の値を継ぐ。WU は自分の明示値を優先し、無ければ task の値を継ぐ。自動作成の review / integration repair は、それぞれの task worktree を共有する既存の排他と review lock に従う。

### D2. actual write-set は Git の確定差分を run ごとに記録する

Git worktree を使う実装 run について、開始前に repo ごとの `base_sha` と `HEAD` を固定する。終了後、WU の自動 commit が済んだ時点で `git diff --name-only -z <base_sha>..<head_sha>` を repo ごとに実行し、NUL 区切りの path を UTF-8 へ変換、ソート・重複排除して保存する。WU の `base_sha` は ADR-0074/0079 の `base_commit`（依存 WU または親 branch から分岐した SHA）、atomic task の run は開始前の HEAD とする。run が yield/失敗して未 commit の編集を持つ場合は `complete=false` として記録し、後続の確定 commit 後に同じ WU の基点から再採取する。Git diff の対象はコミット済みの path で、未追跡ファイルや未 commit の編集を確定実績と偽らない。rename は `--no-renames` を併用して旧名・新名を両方数える。失敗・不読は空配列にせず `status=unavailable` と理由を残す。remote の `.git` を手元の同期写しで走査せず、Git が有効な remote worktree での読み取りが実装できるまで unavailable とする。

新 migration は **0039 以上**の未使用番号を取り、`run_write_sets`（`run_id, task_id, work_unit_id?, repo_id, base_sha, head_sha, paths_json, status, recorded_at`、`run_id,repo_id` を一意）に保存する。再試行で同じ run を読んでも upsert は同じ snapshot で冪等とし、違う SHA を上書きしない。WU の累積実績は run 行から導出し、WU 完了時にその WU の `base_commit..HEAD` を最終 snapshot として `work_unit_write_sets`（`work_unit_id,repo_id` 一意）へ保存する。両方とも task / run / WU の索引を付ける。rebase は commit SHA を変えるので snapshot の `base_sha/head_sha` を常に併記し、過去の実績は書き換えない。actual は観測・改善の材料であり、現在走る run の予測値や merge 安全性の証明には使わない。

### D3. 重なり判定と待機

`task-core` に I/O のない `write_set_overlap(a,b)` を置く。同じ repo ID で、正規化した prefix が等しい、片方がもう片方の path segment 境界での祖先、または同一 file を指すとき **強い重なり**とする。例: `src/` と `src/a.rs` は強い、`src/a.rs` と `src/ab.rs` は重ならない。別 repo、空集合は重ならない。閾値は「強い重なりが 1 組以上」で、新しい run の起動を待たせる。path の個数や文字列の部分一致に基づく確率閾値は設けない。

dispatcher は通常の capacity / lease / pause / 工程 gate の後、run の開始直前に共通 repo の実行中予約を照合する。同じ task の WU と別 task の実装 run、子 task の同じ repo の run を対象に含める。候補が競合したら `ready` のまま次 tick を待つ。失敗遷移・`ReviewFail`・retry・attempts 加算をしない。待機中は対象 task ID / WU ID と待機開始 tick を診断に残す。複数 dispatcher がある場合は、照合と予約確保を store transaction で原子的に行い、lease 消失・run 終了で予約を解放/復旧する。run 開始後に hint を変えても予約は変わらない。

同一条件の待機者は待機開始順、task ID、WU seq の順で起こす。**連続 3 回の dispatch tick** で競合のため起動できなかった候補を通常候補より先に照合する。ただし既に走る競合 run を止めず、容量・pause・親子依存を超えない。予約解放後は最古待機者を先に採るため、新着の高優先 task による追い越しで飢餓しない。既存の `max_concurrency` と provider 枠、ADR-0074 の WU 上限は引き続き上限である。hint の欠落は従来互換を優先してこの予約を作らないので、その task との競合は抑制できない。したがって本 gate は静的な安全保証ではない。

### D4. target からの behind commits と age

各 Git task branch とその **現在の target**（root は登録元 repo の既定 branch、tree child は親 task branch）について、`target_sha` と `head_sha` を同じ採取で固定する。`behind_target_commits = git rev-list --count <head_sha>..<target_sha>` と定義する。target のみにある commit 数であり、`git rev-list --left-right --count` の ahead 値は含めない。target が HEAD の祖先なら 0。複数 repo は repo ごとの値を保持し、task の代表値は最大の behind commits とする。Git/ref 不読、remote/shared/非 Git は `null`（0 にしない）。

`behind_target_since` は正の behind を最初に**観測した** UTC 時刻を store に保持し、同じ target 系列で正の間は target が進んでも維持する。0 を観測したときに消す。age は `max(0, now - behind_target_since)` 秒で、コミット日時から推測しない。未観測・不読は `null`、0 のときは 0 秒。`behind_target_observed_at` と両 SHA も保存し、古い snapshot を新しい値として表示しない。測定点は run dispatch 候補を選ぶ前、review 前 sync の候補選択前、sync の後、API の task 詳細読取時とする。API read は Git への再測定を強制せず、最後の snapshot とその観測時刻を返す。stale の優先判定は review 前に再測定した値だけを使う。

metrics の task 単位の欄を `behind_target_commits`, `behind_target_age_seconds`, `behind_target_observed_at` とし、`GET /api/v1/tasks/{id}` の `TaskDetail.behind_target` に repo 別 `repo_id`, `target_sha`, `head_sha` と同名の 3 欄、代表値を置く。`GET /api/v1/tasks/{id}/execution` の要約にも代表値を出す。GUI の task 詳細は「target から遅れ: N commits / HH:MM 経過（観測 UTC 時刻）」と repo 別内訳を表示し、null は「計測不可」と表示する。集計 API で時刻に依存する age を履歴 event の再生から再計算しない。

### D5. Phase 1 の review 前 sync の stale 優先

ADR-0118 の同期入口に到達して review lock・human gate を通った task の待ち行列だけを並べ替える。behind が正のものを先にし、その中では `behind_target_age_seconds` 降順、`behind_target_commits` 降順、待機開始順、task ID の順にする。behind 0・計測不可は既存の待機順にする。連続 3 回の有資格 tick で選ばれなかった task は age 順より先の FIFO 枠に上げる。空き容量や lock を奪わず、走行中の review を中断しない。

選択後は ADR-0118 D1/D4 のとおり target ref を再読取し、SHA が動けば stale 経路で再同期する。優先順位は判定結果を短絡しない。`SyncOutcome::Conflict` は ADR-0120 の IntegrationRepair に渡し、dirty/remote の例外も同 ADR に従う。target の再進行を `ReviewFail` にせず attempts を消費しない。

### D6. 既存の統合不変条件

write-set が非重複でも Git merge/rebase の成功は保証されない。ADR-0043 の `rebase_and_advance` の clean / Busy / Conflict と compare-and-swap の fast-forward、ADR-0118 の `sync_onto_target` と検査済み merge candidate SHA、`review.rs` の stale 再検査を必ず残す。ReviewRepair は同期後の検査不合格、IntegrationRepair は同期衝突に限り、実績差分との不一致だけで自動 repair を起票しない。ADR-0079 の子→親 branch の段階統合、統合後の checks、root の `celeris/src/delivery.rs` の候補照合と selfdeploy gate はそのまま通す。

remote worktree はローカル Git worktree と同一視しない。ADR-0079 の push-after-run / remote-exec と、remote/shared の WU 直列・子 branch 非統合の例外を保つ。実績や behind が計測不可でも task を失敗にせず、無根拠な 0 を出さない。完全な静的予測は目指さない。

### D7. 後続 WorkUnit の分担と検証

| 葉 | 所有する変更 |
|---|---|
| `core-model` | `task-core` の optional hint 型、純粋な重なり判定、0039 以上の migration と store、plan/3 の unit 欄。既存 plan の読み戻しを維持する |
| `actual-record` | run / WU の Git 差分採取と確定 snapshot の保存。`write_set` を含む一時 Git repo 試験 |
| `parallel-gate` | dispatcher の原子的予約と待機、公平性、WU/子 task の共通 repo 判定。固定 tick の `write_set` 試験 |
| `behind-metric` | target SHA の採取、behind/since の store と metrics。`behind_target` を含む一時 Git repo 試験 |
| `api-writeset` | task 作成/更新・plan 入力と TaskDetail/Execution の欄、API schema の再生成 |
| `stale-priority` | ADR-0118 の review 前 sync 待ち行列の順位と FIFO 救済。固定 tick の `behind_target` 試験 |
| `gui` | task 詳細の期待/実績 write-set と behind/age の表示、型生成 |
| `verify` | 全 workspace の test/clippy/fmt、main との衝突再見積もり、`docs/architecture-map.md` と `docs/PROGRESS.md` の記録 |

テストは外部ネットワーク、実 claude、systemd を使わず、一時 Git repo と固定 tick (`tick_until` / `run_until`) で決定的に書く。実装の production code で `unwrap()` を使わない。後続実装が本決定から外れる必要があれば、変更前に本 ADR へ理由と新しい不変条件を追記する。

## 付記（behind-metric の実装時の決定）

- 保存先は migration 0040 の `task_behind_targets`（`(task_id, repo_id)` ごとに最後の 1 行）。`target_ref` を target 系列の識別に使い、系列が変われば `behind_target_since` を測り直す。計測不可（`null`）の観測は同じ系列の `since` を消さない。観測時刻が保存済みより古い観測は捨てる。
- 計測は `task_ops::behind_target::observe_behind_target`（`HEAD` と target を先に SHA で固定してから `rev-list --count`）。この葉で配線した測定点は ADR-0118 の review 前 sync の直前と直後（`Dispatcher::observe_behind_target`。失敗は warn のみ）と、API/Execution 読取時の snapshot 読み出し（Git を測らない）。run dispatch 候補選択前の測定点は dispatcher の候補選択を変える `parallel-gate` / `stale-priority` と同じ箇所になるため、それらの葉で配線する。
- metrics の 3 欄は `task_core::ExecutionMetrics` に置き（`GET /tasks/{id}/execution` と task 詳細の Execution 節）、`TaskDetail.behind_target` の repo 別内訳は `api-writeset` が `task_ops::behind_target::behind_target_of` の結果を載せる。
