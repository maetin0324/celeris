# ADR 2026-10-27: done の後に main が進んで unintegrated になった task を、衝突しなければ自動で取り込み直す

- 状態: 提案（2026-10-27。実装は次の WorkUnit）
- tasks: [01M5YYZ7FJ4REHEKFMDPV4KCGG]
- 関係: ADR 2026-10-11-stranded-done-branches、ADR 2026-10-12-stranded-sweep-single-flight、
  ADR 2026-10-16-project-auto-integrate、ADR 2026-10-02-parallel-integration-auto-resolve、
  ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse、ADR-0118（pre-review target sync）

## 問題

2026-10-27 に 01M5XXF7QH は 16:04:19Z に `review_pass` で done になった。17 秒後に別 task の取り込みで main が進み、
head が main の祖先でなくなった。ff-only の取り込みは行われず、stranded sweep は `recorded head is not an ancestor of the
repository default branch` の `unintegrated` 行を作るだけで、自動では取り込み直されなかった。operator が
`git merge-tree` で clean を確かめ、`POST /tasks/{id}/integrations/{repo}/merge` で手で取り込んだ。

現行の経路は次のとおり。

- 配送（`crates/celeris/src/delivery.rs`）は `MergeQueued` で候補の鮮度を見て、stale なら `request_rereview`（再 sync → 再 check →
  再 review）、fresh なら `merge --ff-only`。done になった後に main が進むと、配送がすでに閉じた・Blocked の task はここへ戻らない。
- stranded sweep（`crates/celeris/src/daemon/stranded_sweep.rs` が 300 秒ごと、single-flight。`task_ops::stranded::sweep`）は
  done の root task の head が main の祖先かを見て `unintegrated` 行を upsert し、受信箱の `unintegrated` 項目になる。
  人の操作は `stranded::{merge, hold, discard, reopen}`。`merge` は no-ff を一時 worktree で行うが check を走らせない。
- 無言ではないが、人が気づいて押すまで残る。並列 task が多い日は同じことが繰り返される。

## 決定

### D1. 対象の条件

sweep が次をすべて満たす `unintegrated` 行を「再取り込み候補」にする。

1. task が done の root task（sweep の既存の絞り込み）で、`changed_repo`、ローカル repo。
2. 案件の `auto_integrate = true`（`task_ops::delivery::begin` と同じ判定を共通の関数にして使う）。
3. 行の理由が `recorded head is not an ancestor ...`（main が進んだだけ）。repo 解決不能・branch 消失・head 消失・remote は対象外
   （従来どおり受信箱に出る）。
4. 行が `held` / `discarded` でない（sweep は元からこれらを飛ばす）。人が `reopen` した行は候補になる。
5. 配送が走行中でない（`Delivery` が `Reviewing`/`MergeQueued`/`Merging`/`Preparing` でない）。走行中の配送が自分で扱う。
6. 自動の試行回数が D5 の上限未満。

### D2. 判定（git だけ。LLM を使わない）

`task_ops::stranded::reintegrate::classify(repo, head, default_branch)` を新設する（純粋な git 呼び出し。`git_output` を使う）。

- `git merge-tree --write-tree <default> <head>` が 0 なら **clean**。非 0 は **conflict**。
- clean のとき、`git merge-base <default> <head>` を base とし、`git diff --name-only base <default>`（main 側の変更 file）と
  `git diff --name-only base <head>`（task 側の変更 file）が重なるかを見る。重ならなければ **disjoint**、重なれば **overlap**
  （同じ file の別の hunk でも overlap とする。reviewer 再利用の規則 ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse D2 と同じ）。
- 結果は `Disjoint` / `Overlap { files }` / `Conflict { files }` / `Unknown { reason }`（git 失敗。`Unknown` は何もせず次の周期で再判定）。

### D3. 取り込み方: 既存の rereview 経路に乗せる（rebase も直接 merge もしない）

`Disjoint` と `Overlap`（clean のもの）は、done の task を `Trigger::Rereview`（done → reviewing。`transition.rs` が許す）で
reviewing に戻し、`ReviewTargetAdvanced` ではなく新 event `DoneReintegrationStarted` を同じ transaction で追記する。以後は
ADR-0118 の review 前同期が走る。つまり新しい仕組みは作らず次の既存部品を使う。

- review 前の target 同期（task branch へ main を merge。衝突は ADR-0120 の IntegrationRepair）。
- 必須の決定的 check は新しい merge candidate で再実行される（ADR 2026-10-23-required-checks-bound-to-candidate）。
- reviewer は、main 側の変更 file が task の差分と重ならなければ省かれ（`reviewer_reuse` / `ReviewerPassReused`）、重なるときだけやり直す。
  つまり D2 の `Disjoint` は決定的 check だけ、`Overlap` は reviewer もやり直しになる。
- 合格後の取り込みは配送の `merge --ff-only`（`merge_reviewed`）。stale なら配送が再 review に戻す。

**却下した案。** (a) rebase して check を再実行: head が書き換わり、reviewer の合格（`(repo, target_sha, reviewed_sha)` に束縛）と
人の承認が使えなくなり、task branch の履歴も変わる。(b) `stranded::merge` の no-ff を自動で呼ぶ: check を走らせないので、
main との結合で壊れても検出できない。check を走らせる別の仕組みを足すことになり、既存の再 review 経路と二重になる。
`Conflict` は D4。

### D4. 衝突・失敗は受信箱へ出し、無言で残さない

- `Conflict`: 自動では触らず、行の理由を `auto reintegrate: merge conflict in <files>` に更新して既存の受信箱 `unintegrated` に出す
  （選択肢は既存の merge / hold / discard / reopen）。`stranded::merge` が失敗時に残す `integration_request`（origin = delivery）も作り、
  衝突 file を `conflict_files` に入れる。
- reviewing に戻した後の失敗（check 不合格・IntegrationRepair 上限・stale 上限）は、それぞれ既存の受信箱 kind
  （`review_resync_halted`・needs-human・task failed の通知）に出る。task が failed になった場合も `unintegrated` 行は残るので、
  D5 の上限を超えたものは自動では再試行されず受信箱だけが残る。
- sweep が `Unknown` を返し続けても行の理由は変えない（次の周期で再判定。D5 の回数には数えない）。

### D5. 回数と周期

- 周期は既存の stranded sweep の 300 秒（single-flight の guard の内側）で行う。新しい timer・daemon task は作らない。
- 1 回の sweep で再取り込みを始める task は repo ごとに高々 1 件（並列取り込みの競合を増やさない。順は `done_at` の古い順）。
- 1 つの task の自動の再取り込みは **2 回まで**（`DoneReintegrationStarted` の数。人の `reopen` / `rereview` で数え直す。
  `target_restale_count` と同じ数え方）。2 回とも main が先に進んだ場合は、行の理由を
  `auto reintegrate: limit reached after 2 attempts` にして受信箱に残し、人の操作を待つ。
- 同じ sweep で同じ task を二重に始めないよう、`DoneReintegrationStarted` の追記と `Trigger::Rereview` は同じ transaction
  （`apply_transition_with_events`）で行い、遷移が失敗したら event も残らない。

### D6. event・監査

- task-core の event に追加（追記専用）: `DoneReintegrationStarted { repo_id, head_sha, target_sha, class: "disjoint"|"overlap", attempt }`、
  `DoneReintegrationDeferred { repo_id, head_sha, target_sha, reason }`（`Conflict` と上限到達のとき。同じ理由では 1 回だけ追記）。
  `Event` の定義は `crates/task-core/src/model/events.rs`。追加に伴い `crates/task-api/src/query.rs` の kind 名、`console.rs`、
  web の `event-kinds.ts` / `invalidation-map.ts`、schema 再生成（`UPDATE_SCHEMA=1`、`web/scripts/gen-types.mjs`）が要る。
- 行の `actor` は `"auto-reintegrate"`。通知は出さない（成功は通常の done 通知で足りる。失敗は D4 の受信箱）。

### D7. 消す旧経路

- 「人が気づいて押すまで残る」前提の記述: ADR 2026-10-11-stranded-done-branches D2 の「衝突・権限不足など自動処理不能なら integration request」
  の節に本 ADR への付記を足す。コード経路で消すものは無い（`stranded::merge` は手動の経路として残す。D4 の `Conflict` の人の操作で使う）。
- 互換 flag は作らない。`auto_integrate = false` の案件は従来どおり対象外（人の操作のみ）。

## 実装の置き場所（次の WorkUnit）

- `crates/task-ops/src/stranded/reintegrate.rs`（新規）: `classify`（D2）と `plan`（D1 の条件・D5 の回数・repo ごとの 1 件を決める純粋関数）、
  `apply`（`Trigger::Rereview` と event の追記、`Conflict` の行更新と integration request）。`stranded.rs` は `mod reintegrate;` と再 export だけ。
- `crates/celeris/src/daemon/stranded_sweep.rs` の `run` が `task_ops::stranded::sweep` の後に `reintegrate::run_all` を呼ぶ
  （同じ single-flight の中。store ロックを持ったまま git を呼ばない）。
- 試験: `crates/task-ops/src/stranded/tests.rs` に実 git の一時 repo で `classify` の 4 結果、`plan` の条件（auto_integrate なし・held・
  配送走行中・上限）、`apply` の同時二重適用。`crates/task-dispatch/src/dispatcher/tests/target_sync.rs` に、done の task を main が
  disjoint に進めた後に sweep → reviewing → reviewer の run 0 回・必須 check 再実行 → 配送で ff 取り込み、overlap で reviewer 1 回、の試験
  （`test_now` と一時 git で決定的に）。`crates/task-api/tests/` に `Conflict` が受信箱 `unintegrated` に理由つきで出る試験。
- ADR-0118 D4 付記・`docs/architecture-map.md`・`docs/api/v1/api.md`（event 名）を更新する。

## Consequences

- 並列取り込みで done の直後に main が進んでも、衝突しなければ人の操作なしで main に入る。check は必ず新しい候補で再実行される。
- reviewer は main 側と重ならない進みでは走らない。重なる場合だけ reviewer の費用がかかる。
- 衝突・上限到達は受信箱に理由つきで残り、無言で止まらない。
- 自動の再取り込みは task あたり 2 回・repo あたり 1 件／周期に抑えるので、取り込みが続く日でも再 review の連鎖は起きない。
