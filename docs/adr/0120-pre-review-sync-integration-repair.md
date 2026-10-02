# ADR-0120: review 前同期の衝突を成果保持型 IntegrationRepair で解消する

---
tasks: [01M3XNQH6DRVP790WNGEXKMTZG, 01M3XZ5PYSTTC6GXAH8TZVRHSA]
---

- 日付: 2026-10-02
- 状態: 採用（Phase 2 の衝突解消経路を実装。D5 の WU 個別表示は未完了）
- 関連: [ADR-0118 D6](0118-review-target-sync-and-merge-candidate.md)（review 前同期と衝突）、[ADR-0072 D16](0072-task-execution-decomposition.md)（ReviewRepair・repair WU）、[ADR-0043 D5](0043-workspaces.md)（衝突の解消タスク）、[ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md)（RepairScope・RepairOrigin）、[ADR-0079 D6](0079-recursive-task-decomposition.md)（再帰統合）

## 背景

ADR-0118 は最終レビューの deterministic checks と reviewer の前に task worktree を target（root は既定ブランチ、tree child は親ブランチ）へ rebase し、検査した SHA を merge candidate として固定した。衝突（`task_ops::changes::SyncOutcome::Conflict`）の自動解消は Phase 2 に残し、D6 付記の実装では `crates/task-dispatch/src/dispatcher/review_spawn.rs` が `rebase --abort` 後に同期を諦め、`WorkerProgress`（`review target sync skipped for <repo>: rebase onto <target_ref> <target_sha> conflicted in [...] and was aborted; ...`）を残して**未同期の HEAD** を checks/reviewer に渡している。

この経路では次の問題が残る。

1. reviewer が見るのは target と統合していない HEAD で、合格しても merge candidate が無い。root は review 合格の後に delivery の `validate_candidate` が `[merge-base]` の Blocked にして `make_repair` の局所修復へ回り、修復後に再レビューをやり直す。review を 1 回無駄にする。
2. tree child は `ReviewTargetSynced` が無いので段階統合で候補を照合できず、ADR-0079 の段階末尾 merge で同じ衝突に当たって初めて扱われる。
3. 並列開発で target が進むほど衝突は増える。衝突は実装の不合格ではないのに、後段の失敗経路（`[merge-base]`・段階統合の衝突）で実装失敗と同じ扱いを受けやすい。

一方で既存の部品はそろっている。`review_verdict.rs` の `try_review_repair`（ADR-0072 D16）は、修復できる不合格に対して attempts を消費せず（`Trigger::ReviewRepair`、Reviewing → Ready）`kind = repair` の WU を daemon が追加し、元の worktree とブランチの上で直させる。ADR-0043 D5 の「衝突の解消」タスクは、worktree をそのまま残し、衝突ファイル一覧を渡して rebase を完了させることを受け入れ条件にしていた。本 ADR はこの二つを組み合わせ、review 前同期の衝突を **成果を保ったまま** 解消する IntegrationRepair を定める。

## 決定

### D1. ADR-0118 D6 付記の衝突経路を置き換える

`review_spawn.rs` の同期ループで `SyncOutcome::Conflict { target_sha, files }` を受けたとき、**同期を諦めて未同期 HEAD を review する現行の動作（ADR-0118 D6 付記）を本 ADR の IntegrationRepair 起票に置き換える**。置き換えるのは `Conflict` だけである。

- `Dirty` / `Failed`（dirty・進行中の rebase・ref 不読・detached HEAD）は ADR-0118 D2 付記のまま（同期を省き未同期 HEAD を review）。衝突ではなく、修復 WU に渡す衝突ファイルも無いため。
- remote workspace・`RepoRef` の無いリポジトリ・worktree の無いリポジトリは従来どおり同期の対象外で、IntegrationRepair も起こさない。
- D4 の上限超過・復旧不能のときだけ、現行の「同期を省き未同期 HEAD を review」経路（＝ root は delivery の `[merge-base]` 経路、子は段階統合の衝突経路）へ落とす。

衝突を受けた時点で `rebase --abort` は済んでおり、task ブランチと worktree は衝突前の HEAD（`before_sha`）のまま保たれている（ADR-0118 D6）。この不変条件を IntegrationRepair の前提とする。abort に失敗した場合（`Failed` 扱い）は IntegrationRepair を起こさない。

### D2. 衝突時は IntegrationRepair WU を起票する（ReviewFail・失敗・成果破棄にしない）

衝突は実装内容の不合格ではない。`Trigger::ReviewFail` を使わず、task を `failed` にせず、worktree・ブランチ・コミットを破棄・reset しない。代わりに `try_review_repair` と同じ形で repair WU を daemon が追加する。新しい関数 `try_integration_repair`（`dispatcher/review_verdict.rs` の `try_review_repair` の隣に置く）がこれを行い、`review_spawn.rs` の `Conflict` 分岐から呼ぶ。

**WU の形**（`try_review_repair` の `WorkUnitSpec` を踏襲）:

| 欄 | 値 |
|---|---|
| `kind` | `WorkUnitKind::Repair` |
| `key` | `integration-repair-<n>`（`n` は task の integration_repair WU の通し番号。1 始まり） |
| `title` | `repair (integration_repair): target 同期の衝突解消`（`repair_bucket_of_title` が bucket `integration_repair` を読み戻せる形） |
| `phase` | `None`（daemon 追加扱い。R7-12 以降の配送 repair と同じく replan の対象外） |
| `budget` | 新しい `RepairClass::IntegrationConflict`（bucket `integration_repair`）の `budget()` |
| `checks` | 決定的に 2 件: `git merge-base --is-ancestor <target_sha> HEAD` と `test -z "$(git status --porcelain)"` |
| `context` | 空（最小の context。元の実装の context は作り直さない。ADR-0072 D16） |

**遷移と attempts**: 計画のある task は既存 seq の続きに `Ready` で足し、atomic task は ADR-0072 D5 どおり暗黙の `main`（done）を実体化してから足す（`try_review_repair` の 2 分岐をそのまま共有する）。task は `Trigger::ReviewRepair`（Reviewing → Ready）で worker へ戻す。この Trigger は attempts を消費しない（ADR-0072 D11 の repair の行）。`WorkUnitTransitioned.reason` は `integration_repair` とする。review lock は起票の後に解放し、reviewer run は起動しない（起動済みなら `Cancelled` で閉じる。ADR-0118 D4 付記の stale と同じ扱い）。

**worktree と成果の保持**: 修復 WU は ADR-0072 D6 の直列実行どおり、元の task worktree とブランチの上で走る。新しい worktree を作らず、元の実装 WU の成果（コミット）をそのまま土台にする。ADR-0043 D5 の「衝突の解消」タスクと同じく、衝突を起こした worktree を残し、衝突ファイル一覧を渡して rebase の完了だけを求める。ただし ADR-0043 のような別 task は作らない（task の木を増やさず、review 入口に戻すため）。

**objective**: `task_core::build_repair_objective` は再利用せず、新しい関数 **`task_core::build_integration_repair_objective`**（`crates/task-core/src/execution.rs`、`build_repair_objective` の隣）を足す。理由:

1. `build_repair_objective` は「次の検査が失敗した。失敗を直すことだけをせよ」で始まり、`failing_details`（`Verdict.reason`）を前提にする。衝突は検査の失敗ではなく、必要な入力（衝突ファイル・`target_sha`・`before_sha`・target ref）が違う。
2. `build_repair_objective` は ADR-0074 付記で「`scope` が空なら従来の出力と 1 バイトも変わらない」を試験で固定しており、引数を足すと他の 3 つの呼び出し元（`schedule_integration_check_repair`、`try_review_repair`、celeris `delivery.rs`）の出力を変える危険がある。
3. `RepairScope`（許可範囲・範囲外差分の検査）は同じ型をそのまま引数に取り、範囲外なら `plan_issue` で終える指示も同じ文面で足す（範囲の集め方は `try_review_repair` と共有する）。

署名は `build_integration_repair_objective(target_ref: &str, target_sha: &str, before_sha: &str, conflict_files: &[String], task_title: &str, task_objective: &str, scope: Option<&RepairScope>) -> String`（決定的な文字列合成のみ、I/O なし）。本文は次を含む。

- 「task の成果を保ったまま、`git rebase <target_sha>` を完了させよ。衝突の解消以外の変更をするな」
- `## 同期先`: `target_ref` と `target_sha`（branch 名ではなく SHA に rebase させ、修復の結果を決定的にする）
- `## 衝突前の HEAD`: `before_sha`（`git reset --hard`・`git checkout -- .`・`push --force` で成果を捨てるな。やり直すときは `git rebase --abort` で `before_sha` に戻る）
- `## 衝突したファイル`: `conflict_files` を 1 行 1 件
- 解消の方針（ADR-0043 の考え方）: 両側の意図を残す。target 側の変更を消さない。解消後に `git rebase --continue` で rebase を完了し、WU の `checks`（祖先関係と clean）を自分で実行して exit を確かめる
- 対象タスク（title と objective の先頭 600 文字。`build_repair_objective` と同じ切り詰め）
- 解消できない（両側の意図が矛盾する・設計判断が要る）なら、`before_sha` に戻してから `result.json` に `{"yield":{"plan_issue":"<どのファイルで何が矛盾したか>"}}` を書いて終える

`before_sha` は `SyncOutcome::Conflict` の時点の HEAD である。現行の `Conflict` は `target_sha` と `files` しか持たないので、`review_spawn.rs` が abort 後の HEAD を `rev_parse` で読むか、`SyncOutcome::Conflict` に `before_sha` を足す（実装の葉が選ぶ。どちらでも abort 後の HEAD と一致することを試験で確かめる）。

### D3. 修復 WU の完了後は最新 target で同期をやり直す

修復 WU の結果は、それ自体を合格とみなさず、必ず review 入口（`spawn_review` の同期）をもう一度通す。

1. 修復 WU が `done`（WU の 2 つの checks が exit 0）になったら、最後の WU の完了として従来どおり `WorkerDone`（Running → Reviewing）で review 入口へ戻る。修復 WU 専用の review は作らない。
2. 入口は **その時点の** target ref を読み直す（ADR-0118 D1）。修復が使った `target_sha` に固定しない。
   - target が進んでいなければ HEAD は `target_sha` を祖先に持つので `UpToDate`、進んでいれば新しい target へ `Rebased` になる。どちらも新しい `reviewed_sha` を固定し、`ReviewTargetSynced` を記録してから **全 deterministic checks → reviewer** を新しい SHA で行う（ADR-0118 D3）。修復前の HEAD に対する検査結果・verdict は使わない。
   - 再び `Conflict` になったら（target の再進行で別の衝突が出た、または修復が不十分だった）、D4 の上限内なら次の IntegrationRepair（`integration-repair-<n+1>`）を起票する。objective には新しい `target_sha` と、そのときの HEAD を `before_sha` として渡す。
3. 同期中の target 再進行（`rev_parse` が `target_sha` と食い違う）・検査後の HEAD/target の変化は、ADR-0118 D4 付記の stale 経路（`ReviewTargetAdvanced`、`MAX_TARGET_RESYNCS`）がそのまま扱う。stale は IntegrationRepair の回数に数えず、IntegrationRepair も stale の回数（`MAX_TARGET_RESYNCS`）に数えない（衝突は「SHA を取り直せば済む」再進行ではなく、作業が要る事象なので別に数える）。
4. 修復後の checks/reviewer が実装内容を不合格にしたら、従来どおり `try_review_repair`（ReviewRepair）または `ReviewFail` が扱う。IntegrationRepair はこの判定に関与しない。`try_review_repair` の `max_repairs`・`max_repairs_per_class` の計数からは bucket `integration_repair` の WU を除く（衝突解消で実装修復の枠を食わない）。
5. root の delivery は、合格した attempt に merge candidate があるので ADR-0118 D4 の候補照合で取り込む。tree child は `ReviewTargetSynced` の候補で ADR-0118 D5 の段階統合の照合を受ける。

### D4. 回数上限と、rollback・従来経路へ落とす条件

上限は定数 **`MAX_INTEGRATION_REPAIRS: u32 = 2`**（`crates/task-ops/src/delivery.rs` の `MAX_TARGET_RESYNCS` の隣に置き、dispatcher と試験が同じ値を参照する）。数えるのは task の work_units のうち `kind = repair` かつ bucket `integration_repair` の行の数で、`try_review_repair` の per-class 計数と同じく `repair_bucket_of_title` で読み戻す（専用の欄を足さない）。上限は task の寿命で数え、人の `Rereview`・`Reopen` でも数え直さない（無限の自動修復を防ぐ。人は後述の従来経路の結果を見て判断する）。

次のどれかに当たったら IntegrationRepair を起票しない／打ち切り、**従来経路へ落とす**。従来経路とは ADR-0118 D6 付記の動作（成果を保った未同期 HEAD で checks/reviewer へ進み、merge candidate を記録しない。root は delivery の `[merge-base]` 経路と `make_repair`、子は ADR-0079 の段階統合の衝突経路）である。どの条件でも attempts を消費せず（`ReviewFail` を使わない）、task を `failed` にしない。

1. **上限超過**: 起票しようとした時点で integration_repair WU が `MAX_INTEGRATION_REPAIRS` 件ある。
2. **修復 WU が `plan_issue` で終えた**: 解消に設計判断が要る・範囲外の衝突。worker_finish の `plan_issue` 経路は daemon 追加の integration_repair WU については replan を起こさず、従来経路へ落とす（衝突は計画の誤りではない）。
3. **修復 WU が `failed`**（WU の retry 上限、checks の不合格が続く）・`budget_exhausted` のまま再開できない。
4. **成果の保持が確認できない**: 修復 WU の終了時に task ブランチ HEAD が `before_sha` を祖先に持たず、かつ `target_sha` を祖先に持つ rebase 結果でもない（成果を捨てた疑い）、または worktree が dirty・rebase が進行中のまま残った。
5. **前提が崩れた**: 衝突時の `rebase --abort` が失敗した（ADR-0118 D6 の「手動確認が必要な状態」のまま止める。IntegrationRepair も従来経路も起こさない）、worktree が消えた、remote/shared workspace に変わった。

条件 3 と 4 では **rollback** する: task ブランチを修復 WU の起票時に記録した `before_sha` へ戻す（`git rebase --abort` が要ればそれを先に行い、ブランチ ref を `before_sha` へ戻して worktree を clean にする）。`before_sha` は元の実装の成果そのものなので、rollback で失うのは修復 WU の途中の作業だけである。rollback は `before_sha` が task ブランチの reflog 上にあり worktree に未コミットの変更が無いときだけ行う。満たさないときは rollback せず、理由を残して ADR-0118 D6 の「手動確認」で止める（成果を消し得る操作を推測で行わない）。条件 1・2 は修復 WU が `before_sha` に戻して終える約束（D2 の objective）なので rollback を要さない。ただし HEAD が `before_sha` と異なれば条件 4 と同じく rollback する。

上限超過・従来経路への切り替え・rollback の観測（event 名・API 欄・GUI 表示）は D5 で定める。

### D5. event・API・GUI で衝突の修復を実装失敗から区別する

既存の `RepairScheduled`（`class = "integration_repair"`、`origin = review`）と `WorkUnitTransitioned.reason = "integration_repair"` は従来の repair 集計・WU 遷移のために発行する。それに加え、衝突固有の入力と結末を次の追記専用 event に残す。event の JSON `type` は表の snake_case 名とする。いずれも task の event であり、task 状態や attempts を event 単体で変更しない。

| event (`Event` variant) | payload 欄と発行時点 |
|---|---|
| `integration_repair_scheduled` (`IntegrationRepairScheduled`) | `work_unit_id`, `key`, `repo_id`, `target_ref`, `target_sha`, `before_sha`, `conflict_files: Vec<String>`, `attempt: u32`。WU の追加・`ReviewRepair` 遷移と同じ transaction で 1 回発行する。`attempt` は起票済み integration_repair WU の通し番号（1 始まり）。`conflict_files` は重複を除きパス順に並べ、衝突ファイルだけを渡す。 |
| `integration_repair_resolved` (`IntegrationRepairResolved`) | `work_unit_id`, `repo_id`, `target_sha`, `reviewed_sha`, `attempt`。該当 WU 完了後、最新 target への再同期が成功して `ReviewTargetSynced` を記録するときに発行する。ここでの `target_sha` は**再同期時**の SHA であり、起票時の target が進んでいれば異なる。checks/reviewer 合格を意味せず、その後の判定は D3 のまま。 |
| `integration_repair_exhausted` (`IntegrationRepairExhausted`) | `work_unit_id: Option<String>`, `repo_id`, `target_sha`, `before_sha`, `attempt`, `reason`, `rollback_to_sha: Option<String>`, `fallback: bool`。D4 の打ち切り 1 回につき 1 件。`reason` は `limit_reached` / `plan_issue` / `work_unit_failed` / `budget_exhausted` / `result_untrusted` / `abort_failed` / `worktree_unavailable` の固定値。安全な rollback が完了した場合だけ `rollback_to_sha = before_sha`。従来の未同期 HEAD で review へ進める場合だけ `fallback = true`。abort 失敗・安全に戻せない状態は `fallback = false` として手動確認で止める。 |

`integration_repair_scheduled` を先に書いてから WU だけが残る半端な状態を作らない。WU の完了と再同期の間に daemon が再起動したら、未解決の scheduled event と WU 状態から再開し、resolved/exhausted を重複発行しない。`RepairScheduled` の class 集計は既存の `execution_metrics` を使い、上限判定は D4 の WU 数を正とする。event 件数を上限に使わない。

`GET /tasks/{id}` の `TaskDetail.integration_repair` を省略可能な表示用 object として追加する。欄は `state`（`scheduled` / `resolved` / `exhausted`）、`work_unit_id: Option<String>`, `attempt`, `max_attempts`, `target_ref: Option<String>`, `target_sha`, `before_sha: Option<String>`, `conflict_files`, `reason: Option<IntegrationRepairExhaustReason>`, `rollback_to_sha: Option<String>`, `fallback: Option<bool>`。最後の integration_repair event と対応する scheduled event から決定的に組み立て、履歴が無い task では欄を省略する。`max_attempts` は `MAX_INTEGRATION_REPAIRS`。`resolved` の `target_sha` は再同期時の値を返す。`reason`・`rollback_to_sha`・`fallback` は exhausted のときだけ値を持つ。`limit_reached` のように対応する WU が無い場合は `work_unit_id` と `target_ref` を省略する。

WU の応答 `WorkUnitView.integration_repair`（`GET /tasks/{id}/execution-plan` の `work_units[]`）と task 詳細の `ExecutionWorkUnitView.integration_repair` には、対応する scheduled event がある WU に限り `{repo_id, target_ref, target_sha, before_sha, conflict_files, attempt}` を付ける。WU の `kind = repair`・title だけから SHA や衝突ファイルを推測しない。既存行・通常の ReviewRepair・remote task では欄を省略する。`TaskDetail.integration_repair` は現在の状況、WU 側は各 attempt の起票時 snapshot なので、再同期後の target SHA は両者で異なり得る。event と応答型の schema、および GUI の生成型を更新するが、旧 task の JSON の意味は変えない。

`gui/` の task 詳細では、`TaskDetail.integration_repair.state = scheduled` の間、ページの状態欄直下（`FailureBanner` より上、タブの外）に中立色の案内を出す。文言は「target drift に伴う integration repair：成果を保持して同期の衝突を解消中（`attempt` / `max_attempts`）」とし、実装失敗の赤いバナーや通常の review 不合格として表示しない。`exhausted` では同じ位置に理由・rollback の結果と、`fallback = true` なら「従来の取り込み経路で続行」、`false` なら「手動確認が必要」を示す。`resolved` は常設バナーを消し、タイムラインに再同期先 SHA を表示する。Execution 節の該当 WU 行では `integration_repair` 欄があるとき「target 同期の衝突解消」badge と target SHA・衝突ファイルを表示し、一般の `repair` badge だけに潰さない。トークンや絶対 worktree パスは表示しない。

**migration は不要。** 回数は既存 `work_units`、入力と結末は既存 `events` の JSON payload に保持し、task/API の欄はそこから投影する。専用の DB 列・表・index を増やさないため、`crates/task-core/migrations` に新規 SQL を置かず `SCHEMA_VERSION = 37` を維持する。後続で永続列が必要と判明した場合は別の決定とし、その時点で main と全 `celeris/*` ブランチの migration 名を `git ls-tree` で再走査して番号を決める（現時点の 0037 は複数ブランチで使用済みで、0038 を予約しない）。

### D6. root delivery・再帰統合・remote worktree との境界

root の正常系は、D3 の再同期・全 checks・reviewer を通った `reviewed_sha = merge_candidate_sha` だけを `crates/celeris/src/delivery.rs` の `check_candidate` / `validate_candidate` に渡す。delivery の CAS・`--ff-only`・stale 時の再レビューは ADR-0118 D4 のまま。target がさらに進んだときは `ReviewTargetAdvanced` による再同期を先に試し、衝突すれば D1〜D4 に戻る。D4 の fallback で候補を記録しなかった root に限り、ADR-0118 D6 付記の `check_candidate` の「候補 NULL かつ target が HEAD の祖先でない」判定を経て、従来の `[merge-base]` Blocked → `make_repair` を使う。IntegrationRepair の上限や attempts を配送 repair の回数へ流用しない。

tree child の target は main ではなく親 task branch である。再同期した SHA の `ReviewTargetSynced` を子の段階統合の候補照合へ渡し、親への merge・冪等な「既に祖先」の判定・段階末尾の衝突処理は ADR-0079 D6 のままにする。親が子の review 後に進めば候補 stale として再同期・再検査し、D4 の fallback なら候補なしの従来の段階統合経路へ渡す。子を main に直接取り込まず、子の IntegrationRepair WU を親の plan/replan の leaf として数えない。

remote/shared workspace はローカル task branch を target へ rebase する対象ではない（D1、ADR-0118 D2）。IntegrationRepair のイベント・候補 SHA を捏造せず、クラスタ側 worktree のファイルが正であること、run 後の push と pull 保護、reviewer の `.celeris/remote-exec` 指示、remote child でブランチ統合をしないことを ADR-0079 のまま保つ。ローカルの写しで `git rebase` したり remote worktree を新設したりしない。

## 付記: 後続の実装葉が突き合わせる契約

以下は設計時の具体名であり、実装葉は event の serde 名・API の JSON 欄・GUI の生成型・試験名を照合してから完了とする。名前を変えたら本文と表を同じ commit で直す。

| 種別 | 具体名・照合点 |
|---|---|
| event | `IntegrationRepairScheduled` / `integration_repair_scheduled`、`IntegrationRepairResolved` / `integration_repair_resolved`、`IntegrationRepairExhausted` / `integration_repair_exhausted`。payload は D5 の欄名どおり。既存 `RepairScheduled.class = integration_repair` と `WorkUnitTransitioned.reason = integration_repair` も残す。 |
| API・GUI | `TaskDetail.integration_repair`、`WorkUnitView.integration_repair`、`ExecutionWorkUnitView.integration_repair`。`state`, `work_unit_id`, `attempt`, `max_attempts`, `target_ref`, `target_sha`, `before_sha`, `conflict_files`, `reason`, `rollback_to_sha`, `fallback` は D5 の用途どおり。GUI は task 詳細の状態欄直下と Execution 節の WU 行。 |
| 定数・分類 | `MAX_INTEGRATION_REPAIRS = 2`、`RepairClass::IntegrationConflict` の bucket `integration_repair`、`integration-repair-<n>`、`repair (integration_repair): target 同期の衝突解消`。 |
| 関数 | `task_core::build_integration_repair_objective`、`dispatcher::try_integration_repair`、`repair_bucket_of_title`、既存の `sync_onto_target`、`spawn_review`、`try_review_repair`、delivery の `check_candidate` / `validate_candidate`。 |
| 試験名 | 実装上の試験名は `integration_repair_schedules_atomic_and_preserves_results`、`integration_repair_resumes_review_on_latest_target`、`integration_repair_schedules_limit_falls_back_to_unsynced_review`、`integration_repair_exhausted_result_untrusted_rolls_back_before_sha_once`、`integration_repair_events_serde_round_trip_and_type_names`、`task_detail_shows_integration_repair_scheduled_state` など。各名に `integration_repair` を含み、名前フィルタで 0 件にならないようにする。 |

## 付記: Phase 2 実装との照合（2026-10-02）

- `IntegrationRepairScheduled` / `Resolved` / `Exhausted` の serde `type`、D5 の payload 欄、`IntegrationRepairExhaustReason` の 7 固定値は `task-core` の event・enum と一致する。`RepairScheduled.class` と `WorkUnitTransitioned.reason` は `integration_repair`、WU の key は `integration-repair-<n>`、上限 `MAX_INTEGRATION_REPAIRS` は 2 件で一致する。
- `TaskDetail.integration_repair` と受信箱の `AttentionItem::Failed.integration_repair` は実装済み。表示 object の `state`、`work_unit_id`、`attempt`、`max_attempts`、`target_ref`、`target_sha`、`before_sha`、`conflict_files`、`reason`、`rollback_to_sha`、`fallback` は実装と一致する。一方、D5 の `WorkUnitView.integration_repair` と `ExecutionWorkUnitView.integration_repair`、GUI の WU 行の専用 badge は未実装。`IntegrationRepairScheduled` から WU ごとの起票時 snapshot を組み立てる関数は `task_core::integration_repair_snapshots` として存在するが、API 応答にはまだ渡していない。D5 の未完了項目として追跡する。
- rollback の現実の安全条件は `dispatcher/worker_finish.rs::rollback_integration_repair` に従う。task branch 上で、**rollback 判定時点で** `git status --porcelain` が空、`before_sha` がその branch の reflog にあり、必要な `rebase --abort` が成功した場合だけ `reset --hard before_sha` を行う。rebase 中に未コミットの変更が残る場合は、その変更を推測で捨てず `fallback = false` の手動確認に止める。成功時だけ `rollback_to_sha = before_sha` を記録する。
- `limit_reached` は `work_unit_id = null`、`attempt = 2`、`fallback = true` で記録する。修復 WU の `plan_issue` / `failed` / 信頼できない結果は、`before_sha` の clean な復元に成功した場合だけ未同期 HEAD の review に戻す。abort 失敗または安全な復元ができない場合は review を進めない。これらは `integration_repair_exhausted` の `reason` と `fallback` で区別する。
