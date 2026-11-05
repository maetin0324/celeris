---
title: root_row: repo 付き案件は IntegrationMerged だけを取り込み済みに数える
tasks: [01M6MJEG72MD982T211EKAM74G]
status: done
updated: 2026-11-05
---
# root_row: repo 付き案件は IntegrationMerged だけを取り込み済みに数える

完了日: 2026-11-05（WorkUnit root-row。工程の統合は integrate-fix）

## 変更
- `root-row: repo 付き案件の取り込み済み判定を IntegrationMerged だけにする` — `crates/task-ops/src/project_snapshot.rs` の `root_row` から `DeliverySkipped`（`nothing_to_integrate()`）と `IntegrationNotApplicable` による `merged = true` を消した。`integrated` は `done && (repos 空 || IntegrationMerged)`。
- `crates/task-dispatch/src/dispatcher/project_wake.rs` の同種分岐（`DeliverySkipped`・`IntegrationNotApplicable` で `root_done` の wake を積む）を消し、`IntegrationMerged` だけにした。
- `crates/task-core/src/delivery.rs` の `DeliverySkipReason::nothing_to_integrate` は呼び出しが 0 になったので削除した（`grep -rn nothing_to_integrate crates` は 0 件）。
- `agent-docs/adr/2026-11-04-project-controller.md` 付記 A9 の 3 を「D1 で改訂」の文面に置き換え、旧文面（`not_git`・`no_commits` を取り込み済みに数える）を消した。
- 試験: `root_row_tests`（task-ops）の (a) NoCommits 偽、(b) NotGit 偽、(c) IntegrationMerged 真、(d) repo なし真、加えて IntegrationNotApplicable 偽。`project_controller_core_s5b_no_commits_root_is_not_achieved`（crates/celeris/tests/project_controller_e2e.rs、実 bundle）。inbox の対照試験に NotGit が 1 件出ることを追加。

## workspace と案件 tree の関係（調べ結果。コードを読んだ範囲）
- 案件の tree は `project_repos` に登録されたリポジトリ（`location` が local の path）。task の `repos` はその行への参照。
- task の workspace は、run のたびに `dispatcher/worker_task.rs` が `worktree.ensure()` で用意するタスク専用の作業ツリー（`<task_dir>/repos/<name>/`、git の repo ならブランチ `…<task id>`）。成果はまずここにだけ在り、案件の tree（登録 path）には入っていない。
- 案件の tree に載るのは、delivery が作業ブランチを登録 repo の既定ブランチへ merge し `IntegrationMerged` を記録したときだけ。`worktree.json`（marker）の `kind` が `git` でなければ `NotGit`、head が既定ブランチの祖先なら `NoCommits` で見送られ、どちらも成果は案件の tree に入っていない。
- 従来は snapshot がこの 2 つを「取り込み済み」に数えたため、repo を持つ案件で成果が workspace にしか無くても controller は到達と判断できた。今は数えない。`NotGit` は既存の inbox 経路（`inbox.rs` の `DeliverySkipped`、`NoCommits` だけ除外）で理由付きで受信箱に出る。
- repo を持たない案件（`repos` 空）は従来どおり done で取り込み済み。この場合の成果の置き場は pilot の作業場所で、pilot-git が git 化する（別 WorkUnit）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `cargo test -p task-ops root_row_` → exit 0（5 passed）
- `cargo test -p celeris --test project_controller_e2e s5` → exit 0（2 passed、control-ts を pnpm install --offline した後で実バンドルを使用）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 7611 passed、21 skipped、ts-verify 20 passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → ok

## 未解決
- 赤の確認: 修正前のコードに対する試験の失敗は実行していない（試験を先に足す手順を踏めず、修正と同時に書いた）。(a)(b) は修正前の分岐が `merged = true` にしていたことをコードで確認。
- 4（reviewer が成果の無い root を合格にする）は別 WorkUnit review-check の担当。
- IntegrationNotApplicable（遠隔 repo など sweep 対象外）の root は、repo を持つ案件では今後 integrated にならず controller は続行する。遠隔 repo の案件で影響が出る場合は提案を参照。

## 提案
- 遠隔 repo の案件で `IntegrationNotApplicable` を到達に数えるかは、人の判断（ADR 付記の D1 は数えない側）。
