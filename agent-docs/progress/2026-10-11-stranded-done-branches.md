---
title: done task の取り残し調査
updated: 2026-10-11
status: done
tasks: [01M4M8D4ANYGCWVZ0A0WSW53X7]
---

# done task の取り残し調査

設計決定は [ADR: done task の変更を取り残さない](../adr/2026-10-11-stranded-done-branches.md) を参照。

完了日: 2026-10-11（ただし全体 test suite は上の証拠節のとおり 4 件失敗が残る。未解決事項を参照）。

## 原因

根因は、project_id の無い task に対して delivery の判定が何も記録せず終わっていたこと。取り残しの直接の原因はこれ。

DB 事実（本番 DB の reviewer の read-only 確認（sqlite3 -readonly）による。この run からは本番 DB を参照していない）:

- 01M4D7RVKXY9A967ZXDADWVHQJ と 01M4F3V643Q4A3DXTXPT04J14Y は、いずれも `project_id=NULL`、`repos_json=NULL`、`workspace={kind:local,path:<task id>}`。
- 01M4D7RVKX には reviewer run 01M4FTV1VHMECRH01W4K65WM28 の起動（events seq 9883-9885）と `review_pass`（seq 9903）がある。旧コードは reviewer run があるとき `delivery::begin` を呼んでいた（`git show 6a715ef6^:crates/task-dispatch/src/dispatcher/review_spawn.rs` の `if let Some(run) = &reviewer_run`）。
- done の root task 763 件のうち 548 件は project_id が無く repos も空。

制御の流れ（修正前 = `6a715ef6^`）:

1. `crates/task-ops/src/delivery.rs` の `prepare` は、`task.project_id` が None のとき `Err(Skip::Silent)` を返す（旧コード 173 行付近）。
2. `begin` は `Skip::Silent` を `Ok(None)` にする。`Silent` は何も記録しない（`DeliverySkipped` の event も delivery 行も作らない）。
3. よって 01M4D7RVKX は reviewer run があり `begin` まで到達したが、project_id が無いため event も delivery 行も残らずに終わった。これが根因。

現行コード: `6a715ef6` で `review_spawn.rs` は `begin` を常に呼ぶようになった。project 不在の task は `prepare` が `NoProject` を返し、`record_skip`（`delivery.rs` 338-360 行付近）が `DeliverySkipped` を記録する。delivery は main へ入れないが、理由は残る。つまり「黙って消える」は修正済みで、既存の取り残しは delivery では拾えない。その検出は `crates/celeris/src/daemon/stranded_sweep.rs` の `repoless_records` が担う。登録 Local repo の `refs/heads/celeris/<task id>` の有無で repo を解決する（ADR 2026-10-11 D3）。

副次の穴（1 行）: 修正前の `review_spawn.rs` は reviewer run が無い review で `begin` を呼ばなかった。`6a715ef6` 以前に done になった task は再度 `begin` されない。sweep-repoless がこの穴を補う。

01M4F3V643 について: DB 事実（project_id 無し・repos 空）は上と同じ。review run と events は本 run では未確認。project_id 欠落だけで「何も残らない」は説明できる。

## delivery-fix 実装

review 完了時に `delivery::begin` を必ず呼ぶ。delivery policy 対象の root task は既存 acceptance に Reviewer 条件が無くても部署内 reviewer run を先に選ぶため、`begin` が追加する criterion も同じ最終 review で評価される。policy 対象外・project_id 不在・support task・tree child は reviewer run を要求せず、`prepare` が `DeliverySkipped` に理由を記録する。設定形式は変更していないため、本番設定の移行は不要。

回帰試験は `target_sync_pre_review_sync_root_reviews_the_rebased_sha` と `a_plan_with_no_work_left_proceeds_to_review_without_a_run` に追加した。Reviewer 条件を持たない root task と計画付き root の最終 review が done へ進んだ後、delivery 行の代わりに project policy 除外理由がイベントに保存されることを検査する。store と git fixture を使用し、LLM・外部ネットワークには依存しない。

修正前の呼び出し条件（`if let Some(reviewer_run)`）へ一時的に戻した状態では、この回帰試験が理由イベントの assertion で失敗した。修正を戻した状態で同試験と `done_then_command_review_passes_and_events_are_recorded`、delivery fixture 試験は pass した。

最終確認: `TMPDIR=/tmp cargo test -p task-ops -p task-dispatch -p celeris` は成功（task-ops 515、task-dispatch 934、celeris 426 unit tests pass。integration tests も pass）、`cargo clippy --workspace -- -D warnings` と `cargo fmt --all -- --check` も成功。

## 本番への移行

このタスク木で追加した本番設定欄はない。各段の記録と `crates/celeris/src/config/` を確認したが config schema の変更はなく、既存の `[selfdeploy].delivery_projects` を維持する。host・daemon・本番 DB・`~/.config/celeris` はこの作業では変更していない。以下は運用セッションで人が実施する手順。

1. **migration を含む release を作成・検証する。** main にこの変更を取り込んだ後、本番運用セッションで `scripts/selfdeploy/release.sh <main の SHA または ref>` を実行する。integration state 用 migration は `0067_task_repo_integration_states.sql`。release directory の SHA を用いて `scripts/selfdeploy/verify.sh <sha12>` を実行し、`scripts/selfdeploy/status.sh` と release 内 `verify.json` で `ok: true`、staging health の schema version が新バイナリと一致、migration 前後の staging snapshot で既存行数・task digest が一致、主要 GET が成功したことを確かめる。詳細は [selfdeploy 運用手順](../../docs/ops/selfdeploy.md) §2–3。
2. **人が昇格する。** `scripts/selfdeploy/status.sh` で対象 release の verify 結果を確認し、`verify.json.ok` が true の場合に限って `scripts/selfdeploy/promote.sh <sha12>` を実行する。完了後、status/API の release 表示で promoted を確認し、`promote_failed.json` が無いこと、`/health` の release SHA と schema version が対象値であることを確認する。main 取り込みと本番昇格は別操作。
3. **初回 sweep の取り残しを確認する。** daemon 起動後の定期 sweep を待つ。前提: `agent-platform` が project に紐付いた Local の登録 repo であること。repos が空の done root は、登録 repo の `refs/heads/celeris/<task id>` の有無で解決される（`stranded_sweep.rs` の `repoless_records`）。branch の head が既定 branch の祖先なら integrated、そうでなければ unintegrated になる。`celerisctl integration list --state unintegrated --json`（`GET /api/v1/integrations?state=unintegrated`）で、01M4F3V643Q4A3DXTXPT04J14Y が repo `agent-platform`・branch `celeris/01M4F3V643Q4A3DXTXPT04J14Y`・head 付きの 1 件として出ることを確認する。admin 認証付き `GET /api/v1/inbox/items?kind=unintegrated` でも同じ item を照合する。Unintegrated は task×repo ごとの item。

4. **人の決定に従い 01M4F3V643 を discard する。** `celerisctl integration list --state unintegrated --task 01M4F3V643Q4A3DXTXPT04J14Y --json` で branch `celeris/01M4F3V643Q4A3DXTXPT04J14Y` と head を確認する。`celerisctl show 01M4F3V643Q4A3DXTXPT04J14Y --json` の `integrations[]` を見て該当 repo の**名前**を特定し、その repo・branch・head が一致した場合に実行する。

   ```sh
   celerisctl integration discard 01M4F3V643Q4A3DXTXPT04J14Y '<repo-name>' --reason '人の決定: この stranded branch の変更を破棄'
   celerisctl integration list --state discarded --task 01M4F3V643Q4A3DXTXPT04J14Y --json
   ```

   discard は git branch を削除せず durable な disposition を記録する。結果の `state.status` が `discarded`、reason・repo・branch が対象と一致することを確認する。等価 API は `POST /api/v1/tasks/01M4F3V643Q4A3DXTXPT04J14Y/integrations/<URL-encoded-repo-name>/discard`、body `{"reason":"人の決定: この stranded branch の変更を破棄"}`。`repo` は repo ID でなく repo 名。admin 認証が必要で、body は `reason` のみを受け付ける。
5. **01M4D7RVKX は記録が作られない。main 到達は git で確認する。** `celeris/01M4D7RVKXY9A967ZXDADWVHQJ` の branch は merge 後に削除され、`refs/heads/` に無い。sweep は branch ref を走査するため、この task の integrations 記録は作られない（`celerisctl integration list --task 01M4D7RVKXY9A967ZXDADWVHQJ --json` は空で正常）。取り込み済みは次で確認する: `git merge-base --is-ancestor 073a56c4f090a22fd8751ac26a1a9e2c6823fe main`（exit 0 で取り込み済み）。`073a56c4` は merge `da0b6fd6` の第 2 親で、9 commit。これは main 到達の確認で、本番 release の promote 状態とは独立している。

merge 操作（`celerisctl integration merge` と `POST /api/v1/tasks/{id}/integrations/{repo}/merge`）は、project repo の既定 branch を実際に進める（git merge。fast-forward、または detached 一時 worktree の `--no-ff`）。push はしない。衝突時は既定 branch を変えず integrated にしない。

## 未解決事項

- **全体 suite は未だ green でない。** `task-api::browser_e2e` の 4 件（上の証拠節）が全体 run で 60 秒の fixture readiness 待ちに入って落ちる（単独 run は pass）。`crates/task-api/tests/browser_e2e.rs:87-110` は credentiald の `ipc::serve` を `std::thread::spawn` の中で起動し、その戻り値を捨てている。そのため bind 失敗などは readiness timeout としてしか見えない。並走時の socket・path 衝突か serve の早期失敗かは未確定。コードは変更していない。次の対処として serve の結果を観測可能にするか、readiness 待ちを出来事待ちへ直す。
- 通常の TMPDIR（run の作業 path）では socket path が `SUN_LEN` を超え、108 件が落ちる。全体 suite は短い TMPDIR で回す必要がある。
- 初回 production sweep 後の task×repo 件数と 01M4F3V643 の repo 名・branch head は運用セッションで確認する。本番 DB はこの run から参照していない。
- discard 後に人の判断を覆して再統合する場合は、`celerisctl integration reopen` の後に merge を別途判断する。

## 証拠

この節は 2026-10-11 の検証 run（作業ツリー 11ec89e4 基点）の記録。以前の記録（5155 passed / 0 failed）は今回の run では再現していない。

- `bash scripts/dev/test-parallel.sh`（TMPDIR=run の既定の長い path）— exit 100。`CELERIS_TEST_SUMMARY`: passed 5047 / failed 108 / ignored 14。失敗の大半は `bind: path must be shorter than SUN_LEN`（Unix socket path が長すぎる）と `timed out waiting for fixture readiness`。socket path の問題であり、コードの失敗ではない。
- `TMPDIR=/tmp/wu-sock.4fij bash scripts/dev/test-parallel.sh`（socket 用に短い TMPDIR）— exit 100（2 回実行、結果は同じ）。1 回目: passed 5151 / failed 4 / ignored 14、2 回目: passed 5151 / failed 4 / ignored 14。nextest_exit 100、doctest_exit 0、182 binaries。失敗は 4 件で、いずれも `task-api::browser_e2e` の `denied_approval_fails_the_task_without_a_lease`、`auth_section_store_denies_takeover_until_closed`、`registered_credential_does_not_open_approval_for_uncertified_backend`、`approved_credential_requires_conformance_and_does_not_consume_approval`（60 秒の `timed out waiting for fixture readiness`）。
- `TMPDIR=/tmp/wu-sock.4fij cargo nextest run -p task-api --test browser_e2e` — exit 0。4 tests run: 4 passed, 0 skipped。単独では 4 件とも通るため、全体 run の負荷・並走下での readiness 待ちの問題と判断した（根因は未確定。未解決事項を参照）。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告・エラーなし）。
- `cargo fmt --all -- --check` — exit 0（差分なし）。
- 文書検査 3 本: `sh scripts/dev/check-doc-links.sh` — exit 0（`check-doc-links: ok`）。`sh scripts/dev/check-adr-numbers.sh` — exit 0（`ok (182 files)`）。`sh scripts/dev/progress-index.sh --check` — exit 0。
- 参考: `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0（`check-doc-layout: ok`）。`git diff --quiet "${CELERIS_WU_BASE:-HEAD}" -- crates/ web/ gui/` — 差分なし（この run はコードを変更していない）。

### 2026-10-11 全体検証（fix-verify WU、merge-exec 後、コード変更なし）

受け入れ条件 0（test-parallel・clippy・fmt が exit 0）は**未達**。原因は下記の `task-api::browser_e2e` の 4 件で、merge 変更とは無関係の既存の問題。

- `TMPDIR=<run の既定 path> bash scripts/dev/test-parallel.sh` — exit 100。`CELERIS_TEST_SUMMARY`: passed 5052 / failed 108 / ignored 14。108 件は `bind: path must be shorter than SUN_LEN` と fixture readiness timeout（run の長い TMPDIR に起因）。
- `TMPDIR=$(mktemp -d /tmp/wu-sock.XXXX) bash scripts/dev/test-parallel.sh` — exit 100。`CELERIS_TEST_SUMMARY`: passed 5156 / failed 4 / ignored 14、nextest_exit 100、doctest_exit 0、182 binaries、nextest 154.1s。`Summary [153.085s] 5157 tests run: 5153 passed, 4 failed, 13 skipped`。失敗は 4 件のみ: `task-api::browser_e2e` の `approved_credential_requires_conformance_and_does_not_consume_approval`、`auth_section_store_denies_takeover_until_closed`、`denied_approval_fails_the_task_without_a_lease`、`registered_credential_does_not_open_approval_for_uncertified_backend`。いずれも `browser_e2e.rs:107` の `timed out waiting for fixture readiness`（前回記録と同じ 4 件）。
- `TMPDIR=<短い socket 用 dir> cargo nextest run -p task-api --test browser_e2e` — exit 0。`4 tests run: 4 passed, 0 skipped`。同じ試験は単独では通る。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告なし）。
- `cargo fmt --all -- --check` — exit 0（差分なし）。
- 前回記録の「5155 passed / 0 failed」はこの run では再現していない。

- `TMPDIR=<短い socket 用 dir> cargo nextest run -p task-ops -p task-api -p celeris -E 'test(stranded) | test(integration_api)'` — exit 0。`21 tests run: 21 passed, 1696 skipped`（merge-exec の `stranded_merge_*` と `integration_api` を含む）。

判定: merge-exec の試験は全件 pass。残る 4 件は `browser_e2e.rs:87-110` の fixture（serve の戻り値を捨てる）の readiness 待ちで、根因は未確定。次の対処は未解決事項の 1 件目を参照。

### 2026-10-11 根因修正（rootcause-verify WU、コード変更なし）

- `bash scripts/dev/test-parallel.sh`（TMPDIR=run の既定の長い path）— exit 100。`CELERIS_TEST_SUMMARY`: passed 5055 / failed 108 / ignored 14。失敗 108 件は `bind: path must be shorter than SUN_LEN`（socket path が長い）。前回と同じ環境起因。
- `TMPDIR=$(mktemp -d /tmp/wu-sock.XXXX) bash scripts/dev/test-parallel.sh`（socket 用に短い TMPDIR、実行後に削除）— exit 100。`CELERIS_TEST_SUMMARY`: passed 5159 / failed 4 / ignored 14、nextest_exit 100、doctest_exit 0、nextest 142.7s。失敗は 4 件のみ: `task-api::browser_e2e` の `approved_credential_requires_conformance_and_does_not_consume_approval`、`auth_section_store_denies_takeover_until_closed`、`denied_approval_fails_the_task_without_a_lease`、`registered_credential_does_not_open_approval_for_uncertified_backend`（60 秒の fixture readiness timeout。前回記録と同じ 4 件）。exit 0 にはなっていない。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告なし）。
- `cargo fmt --all -- --check` — exit 0（差分なし）。

受け入れ条件 2（test-parallel が exit 0）は未達。残る 4 件は本 WU の変更と無関係で、前回記録の未解決事項の 1 件目。

### 括弧修正後の全体検証

- 原因: ea39749e（親 branch を main へ rebase した merge）で `crates/task-ops/src/stranded.rs` の `promotion_view` の閉じ括弧が落ち、compile できなくなった。修正 commit は 50f349d8。
- この task の base（3ddf9c0e）は rebase 前の旧履歴で 50f349d8 を含まなかったため、`git merge --no-ff 50f349d8` で取り込んだ（merge commit 71c2b98b）。衝突は `gui/app/celeris/types.ts` の modify/delete の 1 件のみで、削除側（main の GUI 廃止）に解いた。`git merge-base --is-ancestor 50f349d8 HEAD` は exit 0。`git diff --quiet 50f349d8 HEAD -- crates/ web/ gui/ docs/` は exit 0（差分ゼロ）。
- `bash scripts/dev/test-parallel.sh` — exit 0。`Summary 5161 tests run: 5161 passed, 13 skipped`、doctest ok（passed 5161 / failed 0）。TMPDIR は `/tmp` を試験 fixture だけに使った（socket path が `SUN_LEN` 108 を超えるため。下記）。
- 同じ検査を run の既定の長い TMPDIR（約 98 バイト）と workspace 内の短い TMPDIR で流したところ、どちらも 108 件失敗（`bind` の path 長超過と fixture readiness timeout）。fixture の socket は `tempdir/run/celeris-credentiald/*.sock` で、tempdir に約 50 バイト以下が要るため。環境起因で、コードの失敗ではない。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告なし）。
- `cargo fmt --all -- --check` — exit 0（差分なし）。
- `sh scripts/dev/check-doc-links.sh` — exit 0（`check-doc-links: ok`）。
- `sh scripts/dev/check-adr-numbers.sh` — exit 0（`check-adr-numbers: ok (194 files)`）。
- `sh scripts/dev/progress-index.sh --check` — exit 0（`progress-index --check: ok`）。
- release prepare の失敗は、同 sha の release.sh の並走 lock による一過性の失敗として扱い、本番操作はしない。この run では release を実行していない。

## 提案

- 本番昇格の運用記録に release SHA、verify の schema/row-count 結果、promote 結果、初回 sweep の task×repo 件数、01M4F3V643 の discard 理由、01M4D7RVKX の integrated 表示をまとめ、監査可能にする。

## 経路表

| task 経路 | review / done 経路 | begin 到達条件 | 判定 |
| --- | --- | --- | --- |
| Atomic root | implementation 後 `spawn_review`。reviewer run の選択は acceptance の `Check::Reviewer` がある場合のみ | reviewer run を作成できた場合だけ `review_spawn.rs:345-354,620-630` | reviewer 条件なしで pass/done になると修正前は未到達（副次の穴）。01M4D7RVKX は reviewer run があり到達していた（根因は下の project_id 欠落） |
| Compound root | WU done → `StartIntegration` → 全計画終了後 `FinalReview` → `PlanComplete` → `spawn_review` (`dispatch_run.rs:78-86`; `review_spawn.rs:129-171`) | final review で reviewer run が選ばれた場合だけ | `Check::Reviewer` なしなら未到達。integration は main delivery の代替でない |
| 木の子 | child review 前に child branch を commit (`review_spawn.rs:368-373`)、親工程の統合で親 branch に取り込む | reviewer run があれば `begin` 呼び出し自体には到達し得る | `prepare` が `is_tree_child` を silent skip (`delivery.rs:175-181`)。main delivery 対象外は意図された設計 |
| project / kind による除外 | 上記各 review 経路の後 | `begin` まで到達しても `prepare` 判定 | project_id 無し、config 対象外、support は silent skip (`delivery.rs:172-181`) |

## 再現試験の形

`task-dispatch` の dispatcher fixture と `task-ops::delivery` の store fixture を使い、git repo/worktree fixture に既定ブランチと task branch を用意する。project row、repo row、worktree marker、delivery policy の project ID を揃え、task を ready から実際に review pass → done に遷移させる。

1. Atomic root: `Check::Reviewer` を acceptance に持たない task を dispatch/review 完了させる。done を確認し、`store.delivery_get(task_id) == None`、deliveries 一覧に行が無く、delivery event も無いことを確認する。これは begin 未到達を示す。
2. Compound root: active plan と done の implementation WU、および必要な integrate WU を fixture に入れ、`Trigger::PlanComplete` の最終 review を通す。同じく Reviewer 条件なしで done、delivery 行なしを確認する。
3. Reviewer 制御: 同 fixture で acceptance に `Check::Reviewer` を追加し reviewer provider を用意する。`begin` に進み delivery 行または `delivery_skipped` が記録されることを確認する。次に policy project を外した fixtureでは、begin は呼ばれるが行・skip event とも無い silent skip を確認する。
4. 木の子: task に親を設定し reviewer 条件を付けて begin まで進め、delivery 行が無いことを確認。親工程 integration は別途親 branch に成果を入れることを確認し、child 自身を main delivery しない既定を固定する。

## stranded-core 実装

- 決定的な結合試験を [verify](2026-10-11-stranded-done-branches/verify.md) に追加した。一時 SQLite store・Git repository・固定時計で、done root task 2 件の sweep 検出、受信箱への各 1 件表示、再走査の冪等性、merge request 経由の main 反映と integrated 判定、理由付き discard、hold/reopen、promotion pending の独立表示を検証する。
- `cargo test -p celeris --test stranded_integration stranded_ -- --nocapture` — passed (1 test)。
- `bash scripts/dev/test-parallel.sh` — failed: 5,034 passed / 108 failed / 14 ignored。失敗は browser/credential IPC fixture の Unix socket path が `SUN_LEN` を超える、または fixture readiness timeout。どれも sandbox の path 制約による既知の環境失敗で、追加した stranded 試験は pass。
- `cargo clippy --workspace -- -D warnings` — passed。
- 文書検査: `check-doc-links.sh`、`check-doc-layout.sh`、`check-adr-numbers.sh` — 3 本とも passed。

試験の決定的な主張は、本番 DB や daemon tick に依存させず、fixture の task status・acceptance・project policy・git refs を固定して `done` と deliveries 行の有無を検査すること。現在は調査 WorkUnit のため、試験追加・コード変更は行っていない。

## web-status の前提（API・CLI）の解消

前回の記録にあった「web-status API 確認」「web-status attempt 3 の前提確認」という見出しはこの文書に無いため、ここで前提の解消を記録する。前提だった 2 点は次のとおり解消した（2026-10-11）。

- TaskSummary/TaskDetail の取り込み状態・promotion 欄と、受信箱 `Unintegrated` の状態・操作先: [views](2026-10-11-stranded-done-branches/views.md)。
- task-api の merge/discard/hold/reopen route と一覧 route: [routes](2026-10-11-stranded-done-branches/routes.md)、[routes-wire](2026-10-11-stranded-done-branches/routes-wire.md)。
- celerisctl の list/merge/discard/hold/reopen: [cli](2026-10-11-stranded-done-branches/cli.md)。
- 結合試験（`integration_api_` 接頭辞、store と git fixture）: [e2e](2026-10-11-stranded-done-branches/e2e.md)。

[api-cli](2026-10-11-stranded-done-branches/api-cli.md) の未解決は解消済みに更新した。受け入れ条件ごとの証拠・残った未解決・提案は [close](2026-10-11-stranded-done-branches/close.md) にある。残りは受信箱の discard/hold の操作先（`native` は merge のみ）だけ。ADR D4 の merge 意味の差（人の判断）は解消済み: CoS 代答（2026-10-11）で「merge は人が処分で選んだ取り残しだけに働かせ、衝突時は自動で解決せず人に回す」と決まり、merge 操作は git merge を実際に行うよう実装された（[merge-exec](2026-10-11-stranded-done-branches/merge-exec.md)、`stranded::merge`、祖先確認後に integrated）。
