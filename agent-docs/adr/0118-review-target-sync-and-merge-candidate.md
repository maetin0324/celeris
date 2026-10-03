# ADR-0118: 最終レビュー前に target へ同期し、検査済みの merge candidate を固定する

---
tasks: [01M3WZQZ5DD7EKJNM9997A7NBY]
---

- 日付: 2026-10-02
- 状態: 採用（設計。実装は後続 WorkUnit）
- 関連: [ADR-0043 D5](0043-workspaces.md)、[ADR-0051](0051-supervised-delivery.md)、[ADR-0079 D6](0079-recursive-task-decomposition.md)

## 背景

root task の実装ブランチは古い main を基点とすることがある。現行の ADR-0043 D5 は取り込み時に rebase するため、その rebase が作った SHA は reviewer が見た SHA と異なる。ADR-0051 の自己配送はレビュー開始時の base/head を固定して早送りするため、main が先に進むと停止する。子 task も親ブランチへ統合する前に同じずれを起こし得る。検査した内容と取り込む内容を一致させ、target の更新を見落とさない手順を定める。

着手時の登録元 `main` は `8dc45bd3490c9ea98d0bff9f41e3d763dbe6e971`。`git merge-tree --write-tree --name-only HEAD main` は tree `c2546ff5a08de149c76bbcbe4f6292145a430aad` を返し、衝突ファイルは無かった。これはこの worktree の基点に対する観測であり、実行時の無衝突を保証しない。番号 0116/0117 は並列ブランチが使用するため 0118 とする。

## 決定

### D1. 同期の位置と対象

実装型 root task は worker run の完了後、計画型 task は全 WorkUnit と最終工程の統合が完了した後、**最終レビューの deterministic checks と reviewer run の双方より前**に同期する。`reviewing` へ移る経路（通常完了、再開、再レビュー、ReviewRepair 後）を同じ入口へ集約し、復旧 tick でも同期前の checks を実行しない。人の承認待ちなどでレビューを起動できない場合は、起動する時点で target を再読取する。

target は root では登録元リポジトリの既定ブランチ（通常 `main`）、ADR-0079 D6 の tree child では親 task のブランチである。リポジトリごとに `refs/heads/<target>` の SHA を読み、同期開始時の `target_sha` とする。子の `base_commit` は差分の由来として保持するが、同期先を古い base に固定しない。Git worktree を持たない task はこの同期の対象外とし、既存のレビュー経路を維持する。

### D2. task worktree 上の同期

対象 task の **clean な Git worktree** で `git rebase <target_sha>` を行う。`task-ops/src/changes.rs` の `rebase_and_advance` から rebase 実行、衝突ファイルの収集、`rebase --abort`、結果 SHA の取得を共通化する。ただし取り込み先 ref の早送り、作業ツリー削除、push は同期関数に含めない。既に `target_sha` が task HEAD の祖先なら rebase を省き、現在の HEAD を使う。target と HEAD が同一の場合も同じ扱いにする。

dirty worktree、進行中の rebase、対象 ref が読めない場合は branch を動かさず、既存の失敗経路で理由を残してレビューを止める。未コミット成果を自動 stash・reset・破棄しない。rebase の終了時には HEAD と task ブランチ ref の一致、および clean 状態を検証する。同期中に target ref が変われば、その SHA を候補に採用せず D4 の再同期へ進む。

remote workspace や shared workspace でローカルの task Git worktree/対象ブランチを持たない場合はローカル rebase を試みない。remote 実行・remote reviewer の現行の意味は保ち、同期済み SHA を捏造しない。`spawn_review` がレビュー対象を確保できず起動しない場合も記録を作らない。この除外を root/child の Git worktree の成功として扱わない。

#### D2 付記（2026-10-02、同期を省く場合の実装）

dirty・進行中の rebase・ref 不読・detached HEAD など `sync_onto_target` が `Dirty`/`Failed` を返した場合、`dispatcher/review_spawn.rs` は「レビューを止める」のではなく **同期を省く**。branch は動かさず、`Event::WorkerProgress`（`review target sync skipped for <repo>: <理由>; reviewing the unsynced HEAD without a merge candidate`）を残し、未同期の HEAD で従来どおり checks/reviewer へ進む。`ReviewTargetSynced` と delivery の三つの SHA は記録しない。遷移を起こさないので attempts は消費しない。root の未コミット変更は従来どおり delivery の「コミット後に再レビュー」（`[merge-base]` の Blocked）が扱う。task に登録されていない（`RepoRef` の無い）リポジトリは同期しない。

#### D4 付記（2026-10-02、stale の実装）

stale（同期中の target 再進行、delivery の base/head と同期 snapshot の不一致、検査後の HEAD・branch・target の変化）は遷移せず `Event::ReviewTargetAdvanced`（検査した `reviewed_sha` と今の `target_sha`、`attempt`）と理由の `WorkerProgress`（`review target stale: …`）を残し、task は `reviewing` のまま次の tick の review 入口で再 sync → 全 checks → reviewer をやり直す。検査後の変化は `ReviewOutcome::target_stale` で返し、`review_verdict.rs` は判定を適用しない（reviewer run は `Cancelled` で閉じる）。回数は直近の遷移（`Transitioned{rereview}` の直後に `ReviewTargetAdvanced` が続く自動の再レビューは除く）以後の `ReviewTargetAdvanced` の数で、root delivery の自動再レビューと共有する。`MAX_TARGET_RESYNCS`（2）を超えたら `WorkerProgress`（接頭辞 `review target resync halted: `、両 SHA を含む）を一度だけ残し、`reviewing` のまま review を起動しない（attempts 不変、`failed` にしない）。人のコメント（`interrupt`）などの遷移で数え直して再開する。

### D3. 検査した SHA の記録

同期後、checks を起動する直前の HEAD を `reviewed_sha` として固定し、`merge_candidate_sha = reviewed_sha` とする。`target_sha` は同じ同期で読んだ target ref である。三つ組はリポジトリ単位かつ review attempt 単位で保持する。checks と reviewer は同じ task worktree を見て、レビューの task 単位ロックを verdict の保存まで持つ。checks の前後、reviewer の前後、verdict 保存前に HEAD と task ブランチ ref が `reviewed_sha` から動いていないことを確かめる。変化した attempt の合格判定を破棄し、D4 に戻す。reviewer がいない task でも deterministic checks の SHA を同じように固定する。

`crates/task-core` の migration `0037` で `deliveries` 行に nullable な `target_sha`・`reviewed_sha`・`merge_candidate_sha` を追加し、`Delivery` 型と store の compare-and-swap 保存で JSON 本体と列を原子的に一致させる。旧行の NULL を「検査済み」とみなさない。ADR-0051 の delivery は root の単一リポジトリだけに作るので、tree child に delivery 行を増やさない。すべての対象 task には新しい監査 event（例 `ReviewTargetSynced`）に task/review run、repo、三つの SHA と attempt を記録し、子はこの event を候補の正本とする。verdict と結び付いた **最新の合格 attempt** だけが取り込み資格を持つ。rebase によって旧 SHA が変わる度に旧候補を無効化し、再検査前に delivery を `MergeQueued` にしない。型の JSON schema も再生成する。

### D4. target 再進行と回数上限

review 開始直前、verdict 保存前、root の merge/delivery 直前、子を親の段階へ統合する直前に、target ref と記録済み `target_sha` を照合する。異なるときは stale として既存の合格 verdict を取り込みに使わず、**再 sync → 全 deterministic checks → reviewer** を一組としてやり直す。SHA が一致していても task ブランチ HEAD と `merge_candidate_sha` が異なれば同じく停止して再検査する。古い review を無言で承認扱いしない。

自動再試行は初回に加え最大 2 回（合計 3 attempt）とする。target がなお動く、あるいは同時更新で一貫した snapshot を取れない場合は理由と両 SHA を event/状態に残して安全に停止し、次の明示的な再レビューで再開できるようにする。stale 自体はコードの不合格ではなく、review failure attempts や ReviewRepair の回数に算入しない。再試行は既存の review task ロックと delivery の CAS を通し、二つの reviewer を同時に走らせない。

root の delivery は ADR-0051 の fast-forward、release/verify、push 方針を維持する。merge 直前に target と候補を照合し、一致した `merge_candidate_sha` **だけ**を取り込む。ADR-0043 D5 の人による merge/PR でも候補照合を行う。PR の外部側での merge は GitHub 側の操作なので Celeris が検査済み SHA と取り込み先 SHA の同一性を保証したと記録しない。

### D5. tree child の再帰統合

ADR-0079 D5/D6 の子→親ブランチの段階末尾 merge と順序を維持する。統合直前に子ブランチ HEAD が、合格した最新 attempt の `merge_candidate_sha` と等しいことを確認する。親ブランチが `target_sha` から進んでいれば、子を再同期・再検査・再レビューしてから統合する。候補が変われば旧 child verdict は無効である。shared/remote で子ブランチを merge しない既存の例外はそのまま適用し、存在しない候補 SHA を照合したことにしない。

### D6. 衝突と ReviewRepair

rebase 衝突はこの Phase では自動解消しない。共通関数が衝突ファイルを収集し `git rebase --abort` を実行して、元のブランチと worktree の成果を保つ。abort 失敗時は追加の Git 変更を行わず、手動確認が必要な状態として止める。ADR-0043 D5 の衝突/失敗経路へ理由を渡し、レビュー・merge・delivery に進めない。衝突解消作業の自動化は Phase 2 に残す。

`dispatcher/review_verdict.rs` の ReviewRepair は **同期後に実行した checks/reviewer が実装内容を不合格としたとき**の修復経路として維持する。修復 WU が HEAD を変えたら、新しい同期と全 checks/reviewer を要求する。target 再進行だけで修復 WU を作らず、既存の `merge_base` 修復ヒントも、同期や衝突を成功と偽装する理由に使わない。

#### D6 付記（2026-10-02、衝突の実装）

rebase 衝突（`SyncOutcome::Conflict`）は `rebase --abort` 済みで成果が保たれているので、review は止めずに同期を省く（D2 付記と同じ `WorkerProgress` に衝突ファイルと target SHA を含める）。merge candidate を記録しないので、root は review 合格後に delivery の `check_candidate` が「候補 NULL かつ既定ブランチが HEAD の祖先でない」を stale とせず `merge_reviewed` に進め、`validate_candidate` が merge-base の照合で `[merge-base]` の Blocked にし、従来どおり `make_repair` の局所修復へ進む（自動の再レビューを繰り返さない）。候補 NULL でも祖先関係が成り立つ行は従来どおり再レビューを要求する。tree child は `ReviewTargetSynced` が無いので段階統合は候補を照合せず、ADR-0079 の段階末尾 merge とその衝突経路が扱う。同期の停止はどれも `ReviewFail` を使わず、attempts を消費しない。

## 実装箇所と検証

| crate/module | 変更する責任 |
|---|---|
| `task-core` (`delivery.rs`, event、store、migration `0037`) | 三つの SHA の永続化、旧行の扱い、event と schema |
| `task-ops` (`changes.rs`) | worktree rebase/衝突検出の共通化、task branch と target の照合 |
| `task-dispatch` (`dispatcher/review_spawn.rs`, `review_verdict.rs`, `review.rs`, tree integration) | checks/reviewer 前の同期、同一 SHA の検証、stale 時の再レビュー、子候補の照合 |
| `celeris` (`delivery.rs`) | root delivery 直前の target/候補照合と stale 時の再レビュー要求 |

並列の review-decisions が触る `review.rs`、root delivery が触る `delivery.rs`、browser 系の行は各 WorkUnit の担当範囲で変更する。テストは一時 Git repo だけで、古い同一 base からの 2 task の無衝突同期（`pre_review_sync`）、三つの SHA と target 再進行時の再検査（`reviewed_sha`）、子→親統合、衝突時の abort/成果保持、dirty/remote 除外、上限到達を検証する。外部ネットワーク、実 claude、実 systemd は使わない。
