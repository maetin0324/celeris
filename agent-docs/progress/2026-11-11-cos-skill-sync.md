---
title: CoS の skill を昇格で KB に写し、版が config と違えば CoS run を起こさない
tasks: [01M75722RHE7ZQ6KS09FMSXD3V]
status: running
updated: 2026-11-11
---
# CoS の skill を昇格で KB に写し、版が config と違えば CoS run を起こさない

未完了（run 1 で区切った）。

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

## 証拠（提出時の HEAD で実行）

報告本文に書く（`bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`、
`sh scripts/dev/progress-index.sh --check`）。

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

## 未解決事項

- run 3 の test-affected（8266 件、4 失敗・1 TIMEOUT）の後に直したもの: ADR の link（ADR-0127 へ）、`actor_trace_flow4`・`deployment_audit`
  （試験の偽 release に `skills/` が無く promote が exit 4。`skills/` の無い古い release では KB を変えずに exit 0 にした。ADR D2 に追記）、
  `tick_scale_baseline`（script_compare の Env の KB を一時 dir にし照合を通る skill を置いた）。この 3 本は個別に合格。
- 残る 2 本は未確認: `task-worker browser::post_login_tests::daemon_post_login_unconfirmed_keeps_observation_stopped_and_names_the_reason`（FAIL）と
  `celeris-campaign::k4_cells_validity real_k4_archive_grades_thirty_of_thirty_two`（600 秒 TIMEOUT）。どちらも CoS の照合と関係の無い経路で、負荷の影響と見ているが未確認。
- 直した後の `bash scripts/dev/test-affected.sh` の再実行は未了。

- KB の git の commit の author は `celeris selfdeploy <selfdeploy@celeris.invalid>`（`SD_KB_GIT_NAME`・`SD_KB_GIT_EMAIL` で変えられる）。
  curation の commit と同じ author にするかは未確認。

## 提案

- `cos_chat_skills` は triage の skill の名を `"cos-inbox-triage"` と書き、`[cos.triage] policy_skill` を読まない。
  policy_skill を変えると照合と mount が別の skill を見る。policy_skill の欄を消すか mount で読むかを決める task を提案する。
