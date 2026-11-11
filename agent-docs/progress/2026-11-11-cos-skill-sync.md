---
title: CoS の skill を昇格で KB に写し、版が config と違えば CoS run を起こさない
tasks: [01M75722RHE7ZQ6KS09FMSXD3V]
status: running
updated: 2026-11-11
---
# CoS の skill を昇格で KB に写し、版が config と違えば CoS run を起こさない

実装完了日: 2026-11-11。attempt 2 の再検査で既存試験が失敗したため、受け入れ検査は未完了。本番への配備は運用者が行う。

設計判断は [ADR 2026-11-11 cos-skill-sync](../adr/2026-11-11-cos-skill-sync.md)。基準は `git merge-base HEAD main`。

## 変えたこと

- 照合（ADR D3）: `crates/task-ops/src/cos_skill_gate.rs` に `check`・`SkillGate`・`CORE_SKILL_FILES`・試験用の `write_fixture` を置いた。
  dispatcher は `tick_cos_chat_launch` の claim の条件に `SkillGate::ready` を足しただけ（dispatcher module の行は増やさない）。
  claim の後の「SKILL.md が無ければ prelaunch で `SkillsUnavailable`」の判定は二重なので消した。
- 写し（ADR D2）: `scripts/selfdeploy/cos-skills.txt`（cos-operator・cos-inbox-triage）、`sync-skills.sh`、
  `lib.sh` の `sd_bundle_cos_skills`・`sd_knowledge_root`・`sd_sync_cos_skills`。`release.sh` が release の `skills/` に入れ、
  `promote.sh`・`rollback.sh` が切り替えの記録の後で KB に写して KB の git に commit する（失敗は exit 4）。
- 試験:
  - `dispatcher::tests::cos_launch_wire::skill_drift_holds_the_claim_and_notices_once`: 版 10 と config 1 では 3 tick しても run が無く知らせは 1 件、
    consult.md を消すと知らせが 1 件増えて run は無いまま、直すと次の tick で run が起きる。旧い
    `missing_skills_close_prelaunch_as_skills_unavailable` と `cos_chat_run_launch_missing_required_skill_is_unavailable`（claim の後の判定）はこの試験に置き換えた。
  - `task_ops::cos_skill_gate` の単体試験 3 本（repo の skill が既定の版で通る、版違い・無い file・link 先、front matter の読み方）。
  - `task-worker` の `core_skill_files_follow_the_core_text`（Core が名を挙げる `.md` と `CORE_SKILL_FILES` が一致）。
  - `crates/celeris/tests/cos_skill_sync.rs` → `scripts/selfdeploy/tests/sync_skills.sh`（release の束ね、KB の置き換えと 1 commit、
    curation の commit 済み・変更中・stage 済みのページと他の skill は変わらない、2 回目は commit しない）。
  - CoS run を起こす既存の試験の KB を `write_fixture` に替えた（policy_version を上げる 3 本は、同時に skill も上げる）。

## 前回提出時の検査（今回の再検査結果は後述）

- `bash scripts/dev/test-affected.sh` → exit 0。Rust 8266 passed / 23 skipped、Web 104 files / 712 tests 合格と server 試験合格、
  TS 自己試験 20 passed、対象 doc test 合格、script bundle SKIP 0。健全性検査も合格。
  ログ: run の成果物 `test-affected-run4-final.log`。終了時に一時 dir の削除権限の警告が出たが exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0（差分無し）。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- 版違い・Core の file 欠落で claim せず通知を重複させないこと、一致後に起動すること、release 同期が curation のページを変えないことは、
  上記の対象試験に含まれる（試験名は「変えたこと」節）。

## 本番への移行

運用者が行う。本番の KB・config・daemon には worker は触れていない。

1. この変更を含む release を `release.sh` で作り、`promote.sh <sha12>` で昇格する。昇格の最後に `sync-skills.sh` が
   release の `skills/cos-operator`・`skills/cos-inbox-triage` を `~/.local/share/celeris/knowledge/skills/` に写し、
   KB の git に `selfdeploy: sync CoS skills from release <sha12>` を 1 件 commit する。
   これで KB の cos-inbox-triage は版 11 になり、無かった `cos-operator/consult.md` と `cos-operator/project-controller.md` が入る。
2. 確認: `git -C ~/.local/share/celeris/knowledge log -1 --name-only` が `skills/cos-*` だけを挙げる。
   `grep -A2 '^metadata:' ~/.local/share/celeris/knowledge/skills/cos-inbox-triage/SKILL.md` が `version: "11"`。
   本番 config の `[cos.triage] policy_version` は "11" のままでよい（書き換え不要）。
3. 昇格から写しまでの数秒、または写しが失敗したとき（promote.sh が exit 4）は、受信箱に
   「CoS の skill が config と合わないので、CoS の run を止めています」が 1 件出て、CoS run は起きない。
   `bash ~/.local/celeris/releases/<sha12>/scripts/sync-skills.sh ~/.local/celeris/releases/<sha12> ~/.local/share/celeris/knowledge`
   を手で実行すれば、次の tick から CoS run が起きる。
4. 消した config 欄は無い。

## run 4 の検査と待機修正

- 最初の `test-affected` は共有 script-runner-cache の sandbox 書き込み制限で exit 2。制限を外した再実行では
  健全性・Web（104 files / 712 tests と server 試験）・TS 自己試験が合格。
- Rust は 8266 件中 8262 passed、2 failed、2 timed out、23 skipped（exit 100）。
  git_workspace の `the_repository_check_commands_are_the_reviewers_default` と
  `review_records_each_repositories_own_definition_and_worktree_results` は固定 60 tick 後に判定していた。
- 実装前 brief（待機修正）: 根本原因は review の非同期処理を tick 回数で待つこと。owner は git_workspace の試験。
  既存の `run_until_state` と、commit「phase 1: wait for terminal state and evidence in git_workspace review tests」を再利用した。
  固定回数を増やす案ではなく、終端状態と証拠を待つ既存修正を採用。製品の状態・分岐・fallback・port の追加は無し。
  撤去対象は同じ試験の固定回数待ち。不変条件は repo ごとの必須検査結果・定義指紋・review verdict を検査することで、
  assert は維持する。新しい試験は追加しない。ADR-0125 の出来事待ちに従う。
- campaign の `k4_cells_validity::real_k4_archive_grades_thirty_of_thirty_two` と
  `k_cases_grade::k3_k4_recompute_rejects_a_wrong_report_and_accepts_the_raw_derived_one` は各 600 秒 TIMEOUT。
  当時の host load average は約 114。原因は未確定で、campaign のコード・試験は変更していない。
- 修正後に `cargo clippy --workspace -- -D warnings` は exit 0。再実行は exit 0（8266 passed）で完走。前回の 2 失敗と 2 TIMEOUT は解消した。

## commit（件名）

- phase 1: CoS の skill を昇格で KB に写し、版や Core の file が config と合わなければ CoS run を claim しない
- phase 1: 進捗に run 1 の区切りと落ちる試験 3 本を書く
- phase 1: CoS を有効にする credential_e2e・project_wake の試験の KB に照合を通る skill を置く
- phase 1: ADR の skill 配送の参照を ADR-0127 に直す
- phase 1: skills の無い古い release では KB を変えずに昇格を終え、script_compare の試験の KB を一時 dir にする
- phase 1: wait for terminal state and evidence in git_workspace review tests

- phase 1: CoS skill 同期の最終検査結果を記録する

## 未解決事項

- attempt 2 の `test-affected` は exit 100。受け入れ条件 0 は未達。本番の KB・config・process は変更していない。

## 提案

- `cos_chat_skills` は triage の skill の名を `"cos-inbox-triage"` と書き、`[cos.triage] policy_skill` を読まない。
  policy_skill を変えると照合と mount が別の skill を見る。policy_skill の欄を消すか mount で読むかを決める task を提案する。

## attempt 2 の再検査（提出時の HEAD で実行）

基準は `git merge-base HEAD main`。製品コード・試験・設計の追加変更は無し。
前回 review は `test-affected` の 1200 秒制限で時間切れだった。

- `bash scripts/dev/test-affected.sh`（sandbox 内）: exit 2。共有 script-runner-cache が読み取り専用で TS 検証を開始できなかった。
- 同じコマンドを sandbox 外で再実行: exit 100。Rust 8264 passed / 1 failed / 1 timed out / 23 skipped（843.741 秒）。
  健全性 pass、Web 104 files / 712 tests、server 113 tests、TS 自己試験 20 件は合格。
  ログ: run 成果物 `test-affected-attempt2-final.log`。
- 失敗は今回変更していない task-worker の
  `launcher_credential_post_login_presses_the_pinned_consent_once_and_resumes`
  （`launcher_authenticate_rejected`）と campaign の
  `real_k4_archive_grades_thirty_of_thirty_two`（600 秒 TIMEOUT）。ホストの load average は約 88。
  負荷との因果関係は未確定。
- `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0（記録更新後にも確認）。

- 単独再実行: `cargo nextest run -p task-worker -p celeris-campaign -E 'test(launcher_credential_post_login_presses_the_pinned_consent_once_and_resumes) | test(real_k4_archive_grades_thirty_of_thirty_two)'`
  → exit 0、2 passed / 2275 skipped、513.939 秒。campaign の試験は 513.924 秒。
  ログ: `isolated-attempt2.log`。単独合格は `test-affected` の合格に代えない。
- 残作業: `test-affected` を再実行して exit 0 を確認し、進捗を done にする。
  run の残り予算では再検査の完走を保証できないため checkpoint を残す。
- commit: `phase 1: CoS skill 同期の再検査と未完了事項を記録する`。
