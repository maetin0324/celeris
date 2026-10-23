---
title: main 統合前に、候補 SHA・基底 SHA と必須検査の結果を結び付け、変わったら証跡を失効させる
updated: 2026-10-23
completed: 2026-10-23
tasks: [01M5KWPP519YW28Q6FQRNP5184]
status: done
---
# 必須検査と候補 SHA の対応保証（健全性 5）

ADR: `agent-docs/adr/2026-10-23-required-checks-bound-to-candidate.md`
基準 SHA（着手時の main）: d7cb0fd459263ad52c432959b188c7abfbd81c2d（main の祖先なので rebase で変わらない）。
比較の基準（test-affected・変更範囲）は `git merge-base HEAD main`（最後の target 同期の基底。main の進行で動く）。
起票直前の main は根拠の鮮度の参考値で、比較基準ではない。
変更範囲: `git diff "$(git merge-base HEAD main)"..HEAD`（celeris/delivery.rs、task-dispatch の review/review_spawn/review_verdict、task-core の required_checks と event、task-worker の browser launcher 試験、docs/api・web/api の生成物、workspace.toml・ADR・guide、scripts/dev/tests/test-affected-list.sh）。

## タスク 1 の health gate との接続状態: 接続済み（着手時の基準の時点で既に）
- 着手時の基準の `scripts/dev/test-affected.sh` は `--list` 以外で最初に `bash scripts/dev/check-health.sh` を必須実行する（同 22 行）。
- 同じく `.config/celeris/workspace.toml` の `check` 3 本の 1 本目が `bash scripts/dev/test-affected.sh` で、コメントにも health gate の必須実行が書かれている。
- よって health gate は workspace.toml の check を通して必須検査に含まれ、本 task の `RequiredChecksEvaluated` 証跡の対象（候補・基底・定義 digest に束縛）になる。本 task で追加の接続作業は不要だった。

## 棚卸し（統合経路 → 適用される検査 → 証跡の照合箇所）
| 経路 | 必須検査 | 照合箇所 |
|---|---|---|
| atomic / root task → main（delivery） | review の acceptance **＋** `workspace.toml` `check`（今回: acceptance で置換不可） | review が `ReviewTargetSynced`（基底・候補）と `RequiredChecksEvaluated`（結果）を記録 → `celeris/delivery.rs` の `check_candidate`（SHA）＋ `check_required_evidence`（結果）→ `validate_candidate` → `merge --ff-only`。release gate は main 更新後（全体試験はここだけ） |
| WU の run | WU の `checks`（`CELERIS_WU_BASE` 付き） | `dispatcher/work_units.rs`。統合では env 付き check を流さない（ADR 2026-10-22-wu-env-checks-skip-integration） |
| phase 統合（WU → Task ブランチ） | 統合後の tree で非 WU 専用 check ＋ `default_checks` | `phase_integration::integration_checks_for_phase`（試験 `scope_checks.rs`）。基準は env 無しの `git merge-base HEAD main` |
| 親子統合（子 task → 親ブランチ） | 子の最終 review（repo check も必須に） | `integration::integrate` が `MergeCandidate`（ADR-0118 D5）で子ブランチ HEAD を照合、違えば stale |
| 配送の局所修復・IntegrationRepair | 修復後の再 review | 修復後は候補が NULL/別 SHA になり、証跡も無いので `request_rereview` へ戻る |

## 実装
- 必須検査の置換を禁止: `review::review_task` が `workspace.toml` check を常に足す（acceptance の同一 cmd は二重に走らせない）。旧分岐 `task_has_command` は削除。
- `Event::RequiredChecksEvaluated`（task-core）。束縛は既存の `ReviewTargetSynced` の `target_sha`/`reviewed_sha` を再利用（SHA の仕組みは増やしていない）。
- `task-core::required_checks`: `verify_required_checks`（証跡なし・別束縛・不合格・必須 cmd 欠落を拒否）、`reusable_passes`（同一束縛の合格を再利用）。
- review_spawn が再利用集合を渡し、review_verdict が証跡を追記。delivery が ff 前に照合（失敗時は再 review、上限は既存の `MAX_TARGET_RESYNCS`）。
- schema 再生成: `docs/api/v1`・`web/api/generated`。ADR-0043 D4 の記述・`workspace.toml` コメント・`docs/guides/workspace.md` を新規則に更新。

## 受け入れ条件との対応
- 0 棚卸し: 上表。
- 1 拒否の試験: `required_checks::tests`（不在・不合格・欠落・候補/基底/repo 違い・最新優先）、`delivery::tests::merge_requires_passing_evidence_bound_to_the_candidate_and_base`（無し／不合格／別束縛を ff せず Reviewing へ、正常は ff）、`merge_rejects_evidence_missing_a_required_check_of_the_candidate`（acceptance 置換相当）、`review::tests::acceptance_command_does_not_replace_the_repo_checks`、既存 `target_advanced_*`（main の進行・候補の変更）。
- 2 再利用: `review::tests::bound_passes_are_reused_without_running_the_check_again`、`reusable_passes` の試験。reviewed/merge_candidate/target 照合と ff-only は無変更で再利用。
- 3 基準: `scripts/dev/tests/test-affected-list.sh` (d)（CELERIS_WU_BASE なしの統合で commit 済み変更が空 diff にならない）。`scope_checks.rs`（01M5HPMDFY）、Cargo.lock 再生成（01M5GE1BGE）、検査 timeout の孤児（01M5D93NQD）の試験は変更なしで通る（test-affected の対象）。
- 4 全体試験（全 workspace・web e2e・本物の browser/userns）は追加していない。全体試験は既存の release gate（`scripts/selfdeploy/release.sh`）だけ。タスク 1 の health gate は上記のとおり接続済み。

## 前回実装時の記録（今回の検証結果は後述）
- `cargo fmt --all -- --check` exit 0 / `cargo clippy --workspace -- -D warnings` exit 0
- `cargo test -p celeris --lib delivery` 37 passed、`cargo test -p task-dispatch --lib review::` 57 passed、`... target_sync` 23 passed、`git_workspace` は新規則に合わせて更新
- `sh scripts/dev/tests/test-affected-list.sh`: 当時は fixture に health gate が無く exit 1（FAIL: (c) docs only exit）。shell-test-records で修正（後述）。
- `bash scripts/dev/test-affected.sh`: 1 回目は `celeris::root_done_summary restart_does_not_duplicate` が負荷で 1 回落ち（単独再実行 2 回とも 4 passed、本変更と無関係の既存の時間依存）、2 回目は exit 0（全 crate の nextest ＋ doc test 通過）

### 最終 review の不合格（test-affected exit 100）への対応（WU fix-live-limit-race）
- 原因: `browser_launcher::live_tests::browser_launcher_live_frame_other_session_rejected`（live_tests.rs:687, `ErrorCode::Limit`）の試験の競合。本変更は task-worker に触れていない。拒否された接続（LeaseMismatch・Limit）を client が閉じても、server の接続 thread が `connections`（上限 `max_connections`=4）を減らす前に次の接続を張ると上限超過で Limit になる。live a を開く前（拒否 2 本が残ると a, b と合わせて 4）と raw を張る前（拒否 1 本が残ると 5 本目）の 2 か所に窓があった。
- 修正（commit「browser launcher live 試験の接続数競合を接続回収の出来事待ちで決定的にする」）: `ServerHandle::wait_connections_at_most`（`#[cfg(test)]`、`conn_exit` Condvar で数えている接続が n 本以下になるまで待つ。30 秒は保険）を足し、試験は live a の前に 2、raw の前に 3 まで戻るのを待つ。sleep・上限・assert は変えていない。production の経路は不変。同じ形（拒否直後に上限近くで新しい接続）の他試験は無い（`live_view_rejects_input` は最大 3 本、`session_and_connection_limits` は Limit を期待する側）。
- `cargo test -p task-worker --lib browser_launcher::` exit 0（54 passed）、`browser_launcher::live_tests::` 5 回連続 10 passed。注: この run の TMPDIR は長く unix socket の SUN_LEN を超えるため、試験は `TMPDIR=/dev/shm/...` で回した（評価器の環境では既存どおり）。
- `bash scripts/dev/test-affected.sh` exit 0（4462 passed / 16 skipped）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 本番への移行
config・DB の変更なし。昇格時に MergeQueued で止まっている配送は証跡が無いので 1 回だけ再 review になる。

## 残件・提案
- 既に最新 main で解消済みだった項目: SHA の照合・ff-only・phase 統合の基準（ADR-0118、01M5HPMDFY）は既存で、重複実装していない。
- 複数 repo の定義・実行・証跡の分離は evidence-gaps WU で修正（下記）。
- 本番昇格は人の手順（release gate → verify → promote）。本 task は本番 host を操作していない。
- 残件: なし（下記「最終記録」の残件を参照）。

## evidence-gaps WU（run 01M5M4X70353ZH5HQMCVQ2QHDZ）

- 作業開始時の HEAD は commit「進捗に live 試験の接続数競合の修正を記録」。基準は `git merge-base HEAD main`、WU の比較基準は `CELERIS_WU_BASE`。
- delivery と review は候補の定義を共通の candidate_required_checks で取得する。
  ファイル不在のみ空定義にし、git 取得不能・parse 失敗は拒否する。
- repo ごとに候補の定義、worktree での実行結果、既存 review snapshot、定義 digest を保持して証跡を残す。
  他 repo の結果の複製と定義網羅の免除を削除した。
- verify_required_checks と reusable_passes は定義 digest の一致を必須にした。
  必須検査を先に実行し、acceptance の同一 cmd（exit 0）は結果を共有する。
  再 review の合格再利用も acceptance に適用し、別 repo の結果は共有しない。
- 回帰試験: shared definition loader（git show 失敗・cat-file/ls-tree の確認・parse 失敗・不在）、
  delivery の取得不能拒否、task-core の 2 repo 欠落/不合格・digest 失効、review の 1 回実行/再利用、
  dispatcher の実 worktree 2 repo で各定義と各合否を記録する試験。
- 全 workspace/e2e/browser/userns の試験経路は追加していない。API schema は UPDATE_SCHEMA=1 で再生成する。

### 既存の Git 取得不能試験の期待値修正

- commit「fix: bind required checks and reuse to each repository and definition」の時点で test-affected は exit 100
  （6315 passed / 1 failed / 17 skipped）。失敗は write_set_record_git_failure_keeps_the_run_and_records_unavailable。
- この試験は worker が .git を削除しても task が Done になることを期待していた。
  必須定義が取得不能なら完了を拒否する今回の契約と矛盾するため、失敗した ReviewVerdict を出来事待ちし、
  Done でないこと・合格証跡がないことを検査するよう更新した。run と write-set の unavailable/理由保持の検査は維持。
- `cargo test -p task-dispatch --lib write_set_record_git_failure_keeps_the_run_and_records_unavailable`:
  exit 0（1 passed）。production の迂回・例外は追加していない。必須3検査は修正後の SHA で再実行する。

### evidence-gaps の確定した検証結果

- 当時の検査は commit「test: reject completion when required definitions are unavailable」の時点の HEAD（worktree clean）で実行した。提出時の candidate での再実行は末尾の節。
- digest は v2 domain と長さ付き cmd 列を使う。旧実装が別 repo に複製した成功も失効させる。
- 対象単体試験: review 60、target_sync 23、delivery 34、定義 loader 2、task-core required_checks 9、実 worktree 2 repo の試験と既存 repo 必須検査の試験はすべて exit 0。
- `UPDATE_SCHEMA=1 cargo test -p task-core --lib event_row_schema_matches_committed`: exit 0。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema::tests::committed_schema_matches_generated`: exit 0。
- `node web/scripts/gen-types.mjs`: exit 0。
- `cargo test -p task-core --lib required_checks::`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- test-affected の集計: Summary [ 220.331s] 6316 tests run: 6316 passed (8 slow), 17 skipped。明示的な全 workspace/e2e/browser/userns コマンドは追加・実行していない。
- crates・docs/api・web/api の削除対象欄は検索結果 0 件。schema と web 生成型は再生成済み。
- 本節を記録する後続 commit は進捗文書のみを変更する。
- 任意の progress-index --check は既存の別進捗文書の front matter 不備を報告（本ファイルへの指摘なし）。必須3検査には影響しない。
- この WU の残件なし。shell fixture とタスク全体の最終記録は後続 WU shell-test-records に残る。

## evidence-gaps 再試行（範囲 check の不合格への対応）
- 不合格: WU の範囲 check が `crates/task-ops/src/changes.rs`・`crates/task-ops/src/changes/required_checks.rs` を範囲外と判定（exit 1）。
- 対応: 候補の定義読み取り（file 不在の確認だけ空定義、git 失敗・parse 失敗は Err）を `task_core::required_checks::candidate_required_checks(candidate, git)` へ移した。git の実行は delivery（`git_text`）と review_spawn（`task_ops::changes::git`）が closure で渡す。task-ops の差分は base と同一に戻した。試験 `only_confirmed_absence_means_no_required_checks`・`malformed_definition_is_rejected` は `crates/task-core/src/required_checks/tests.rs` へ移した。
- 範囲 check（同じ cmd、当時の `CELERIS_WU_BASE`）: 範囲外 0 件、exit 0
- `cargo test -p task-core --lib required_checks::`: 11 passed
- `bash scripts/dev/test-affected.sh`: exit 0（6316 passed / 17 skipped）
- `cargo clippy --workspace -- -D warnings`: exit 0、`cargo fmt --all -- --check`: exit 0

## shell-test-records WU（最終記録）

### 修正
- `scripts/dev/tests/test-affected-list.sh`: fixture に最小の `scripts/dev/check-health.sh`（呼ばれたことを記録 file に残す）を置いた。期待は弱めず、次を足した: `--list` では health gate を呼ばない、実行時は必ず呼ぶ、health gate が失敗すると test-affected.sh も失敗する。これで (c) docs only と (d) 統合基底（CELERIS_WU_BASE なしで commit 済み変更が空 diff にならない）まで到達する。test-affected.sh 本体は変更していない。
- 既存の進捗 8 件の front matter 欠落を補った（`progress-index --check` を通すため。別 branch の commit「docs: fill missing progress front matter fields」と同内容）。

### 本 task の commit（`git log --format=%s "$(git merge-base HEAD main)"..HEAD` の subject、古い順）
hash は rebase で変わるので書かない。
- phase: 必須検査を置き換えず、結果を基底・候補 SHA に束縛して main 取り込み前に照合する
- phase: 進捗に test-affected の結果を記録
- phase: browser launcher live 試験の接続数競合を接続回収の出来事待ちで決定的にする
- phase: 進捗に live 試験の接続数競合の修正を記録
- fix: bind required checks and reuse to each repository and definition
- test: reject completion when required definitions are unavailable
- docs: record required-check evidence verification（当時の実装 commit での検証記録）
- phase: 必須検査の定義読み取りを task-core へ移し、WU の範囲内に収める
- test: test-affected-list の fixture に最小 health gate を置き、(c)(d) まで通す
- docs: 進捗に基準 SHA・health gate 接続・実 commit・最終 SHA と今回の exit を記録
- 以降は本進捗文書だけを変える commit（hash を除き、提出時の HEAD での検査を記録）

### shell-test-records WU 当時に実行したコマンドと exit
- `sh scripts/dev/tests/test-affected-list.sh`: exit 0（`test-affected-list: ok`、(c)(d) まで到達）
- `sh scripts/dev/check-doc-links.sh`: exit 0
- `sh scripts/dev/check-adr-numbers.sh`: exit 0（283 files）
- `sh scripts/dev/progress-index.sh --check`: exit 0
- `bash scripts/dev/test-affected.sh`（当時の WU 基準 `CELERIS_WU_BASE`）: exit 0（health gate 合格、crate 差分なしで試験対象なし）
- `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh`（task 基準 = `git merge-base HEAD main`）: exit 0、6316 tests run: 6316 passed、17 skipped
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
- 全 workspace の試験・web e2e・本物の browser/userns 試験は実行・追加していない（release gate だけ）。

### 残件
- なし。本番への昇格は人が release gate・verify・promote の手順で行う。昇格時に MergeQueued で止まっている配送は証跡が無いので 1 回だけ再 review になる。

## 提出時の HEAD での検査（progress-hashfree WU）
検査は提出時の candidate（その時点の HEAD、最後の rebase の後）で再実行した。hash は rebase で変わるので書かない。直前の commit からこの節の追記までの差分は本進捗文書だけで、コードの tree は同じ。
- `bash scripts/dev/test-affected.sh`（WU 基準 `CELERIS_WU_BASE`）: exit 0（health gate・文書検査合格、docs のみの差分で Rust 試験対象なし）
- `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh`（task 基準 = `git merge-base HEAD main`）: exit 0、6336 tests run: 6336 passed、18 skipped
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 提出時の HEAD での検査（progress-base-sha WU）
着手時 main の基準 SHA を戻した後、提出時の HEAD（コードの tree は同じ、差分は本進捗文書だけ）で再実行した。
- `bash scripts/dev/test-affected.sh`（WU 基準 `CELERIS_WU_BASE`）: exit 0（health gate・文書検査合格、docs のみの差分で Rust 試験対象なし）
- `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh`（task 基準 = `git merge-base HEAD main`）: exit 0、6339 tests run: 6339 passed、18 skipped
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
- 全 workspace の試験・web e2e・本物の browser/userns 試験は実行・追加していない（release gate だけ）。
