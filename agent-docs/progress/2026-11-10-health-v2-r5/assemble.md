---
title: 健全性v2 R5 assemble — contracts.json を組み立てて検査し、候補を要約する
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 assemble — contracts.json を組み立てて検査し、候補を要約する

完了日: 2026-11-10

WorkUnit `assemble`。map leaf 5 件の [`mapping/*.json`](mapping/) と unit `old-impl` の [`candidates/old-impl.json`](candidates/old-impl.json) から、
台本 [`contracts.py`](contracts.py) の `--build` で [`contracts.json`](contracts.json) を組み立て、`--check` を exit 0 にした。
fragment・`contracts.py`・試験（crates/ の下）は変えていない（`--build` が初回から不備 0 で通ったので、fragment の手直しは要らなかった）。基準は `$CELERIS_WU_BASE`。

## 結論
- `contracts.json` に task-api・celeris・celerisctl の試験関数 1864 件が全部、capability と契約（1 行）付きで載る。excluded は 2 件（理由はどちらも `unreachable`）、git grep の 1866 行と一致する。
- 不変条件の試験（`invariant=true`）は 874 件。どの候補の members にも入っていない（`--check` が確かめた）。
- 候補は `duplicate_setup` 49 件、`equivalent_scenario` 4 件、`old_impl` 3 件。全件に残す試験・案・mutation の対象 module がある。走査が見つけた組 87 件のうち 34 件は map leaf が採らなかった（`rejected_groups` に理由付きで載る）。
- `extra_capabilities` は 37 件（fragment をまたいで同じ id を写した 10 件は中身が一致し、衝突は無い）。B0 の capability-map には、3 組を統合し 1 件を除いた上で足すことを提案する（「extra_capabilities」の節）。

## 組み立てと検査（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --build > agent-docs/progress/2026-11-10-health-v2-r5/contracts.json` — exit 0、stderr の不備 0 件。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --check agent-docs/progress/2026-11-10-health-v2-r5/contracts.json` — exit 0、`check: tests=1864 excluded=2 git_grep=1866 ok`。
- `contracts.json` の `scan` は走査した commit（map 段の統合 `integrate wu/old-impl (phase map)`）の crates/ の tree id を記録する。この commit の後に crates/ は変えていないので、提出時の HEAD でも同じ tree を走査する。
- 範囲などの検査の結果は「検査」の節に書く。

## crate ごとの試験数
| crate | 試験 | うち src の試験 module | うち tests/ | invariant=true | excluded |
|---|---|---|---|---|---|
| task-api | 855 | 104 | 751 | 354 | 0 |
| celeris | 804 | 537 | 267 | 494 | 2 |
| celerisctl | 205 | 141 | 64 | 26 | 0 |
| 計 | 1864 | 782 | 1082 | 874 | 2 |

fragment ごと: `api-a` 320・`api-b` 320・`api-c` 215・`celeris-src` 537・`cli` 472。

## capability ごとの件数
試験に付いた capability は 74 種（B0 の 37 種、extra の 37 種）。B0 の 41 種のうち `web-gateway`・`web-spa`・`orphan-takeover`・`required-rules-delivery` の 4 種には、この 3 crate の試験が 1 件も当たらない（ほかの crate の試験が持つかは未確認）。

| capability | 出所 | 試験 | うち invariant |
|---|---|---|---|
| `script-runtime` | B0 | 153 | 110 |
| `cos-inbox-triage-operations-audit` | B0 | 120 | 27 |
| `release-selfdeploy-wiring` | B0 | 105 | 70 |
| `notifications` | B0 | 84 | 61 |
| `task-lifecycle` | B0 | 80 | 26 |
| `cos-chat-threads-runs` | B0 | 79 | 48 |
| `project-controller` | B0 | 73 | 9 |
| `http-api-auth-principals` | B0 | 67 | 31 |
| `campaign-eval` | B0 | 65 | 8 |
| `pre-review-sync-auto-resolve-integration-repair` | B0 | 48 | 32 |
| `delivery-merge-candidate` | B0 | 45 | 35 |
| `knowledge-curation-jobs` | extra | 45 | 40 |
| `accounts-quota` | B0 | 43 | 33 |
| `worker-protocol-adapters` | B0 | 42 | 20 |
| `browser-site-policy-trusted-devices` | B0 | 36 | 18 |
| `org-projects-reports` | extra | 36 | 21 |
| `routing-lane-route` | B0 | 34 | 13 |
| `jev-classifier` | B0 | 33 | 14 |
| `decisions-plan-gate` | B0 | 31 | 19 |
| `browser-policy-broker-approval` | B0 | 27 | 20 |
| `cron-jobs` | B0 | 27 | 0 |
| `routing-eval` | B0 | 27 | 7 |
| `sandbox-db-guard` | B0 | 26 | 2 |
| `scratch-build-cache-disk-guard` | B0 | 26 | 11 |
| `execution-plan-adoption` | B0 | 24 | 18 |
| `task-read-views` | extra | 24 | 14 |
| `cluster-admin-connect` | extra | 23 | 17 |
| `task-genres-roles` | extra | 23 | 8 |
| `cluster-ssh-session` | extra | 21 | 13 |
| `cos-authority-ledger` | extra | 21 | 0 |
| `browser-live-view-control` | B0 | 20 | 4 |
| `api-secret-administration` | extra | 19 | 0 |
| `model-catalog` | extra | 19 | 15 |
| `console-feed` | extra | 17 | 0 |
| `recursive-task-tree` | B0 | 17 | 14 |
| `config-load-validation` | extra | 15 | 14 |
| `knowledge-base` | extra | 15 | 9 |
| `approvals-standing-rules` | extra | 14 | 0 |
| `db-backup-maintenance` | extra | 14 | 0 |
| `project-repos-workspace` | extra | 14 | 11 |
| `celerisctl-cli` | B0 | 13 | 11 |
| `knowledge-curation-gc` | extra | 12 | 3 |
| `test-fixtures-shared` | extra | 12 | 6 |
| `config-runtime-edit` | extra | 11 | 0 |
| `project-docs` | extra | 11 | 0 |
| `celeris-mcp` | B0 | 10 | 6 |
| `repository-docs-artifacts` | extra | 10 | 0 |
| `task-files-artifacts` | extra | 10 | 0 |
| `local-machine-settings` | extra | 9 | 4 |
| `routing-audit-outcome` | B0 | 9 | 4 |
| `terminal-sessions` | extra | 9 | 0 |
| `budget-pricing` | B0 | 8 | 4 |
| `daemon-snapshot-config-api` | extra | 7 | 0 |
| `llm-proxy` | B0 | 7 | 3 |
| `org-conversation` | extra | 7 | 0 |
| `review-runs-final-review` | B0 | 7 | 2 |
| `browser-doctor` | extra | 6 | 0 |
| `browser-launcher` | B0 | 6 | 5 |
| `knowledge-pages-skills` | extra | 6 | 0 |
| `daemon-bootstrap-reload` | extra | 5 | 1 |
| `celeris-daemon-wiring` | extra | 4 | 1 |
| `skills-delivery` | extra | 4 | 4 |
| `worker-memory-read` | extra | 4 | 0 |
| `workunit-scheduling-stage-integration` | B0 | 4 | 0 |
| `credential-broker-resume` | B0 | 3 | 2 |
| `db-maintenance` | extra | 3 | 0 |
| `execution-metrics` | extra | 3 | 0 |
| `repo-health-checker-path` | extra | 3 | 0 |
| `doc-gardener` | extra | 2 | 2 |
| `event-replay` | extra | 2 | 0 |
| `worker-followup-tasks` | extra | 2 | 2 |
| `browser-egress-runtime` | B0 | 1 | 0 |
| `continuation-session-resume` | B0 | 1 | 1 |
| `run-claim-lease` | B0 | 1 | 1 |

## invariant=true の試験
874 件（task-api 354・celeris 494・celerisctl 26）。根拠は B0 の invariants の `evidence_path` と architecture-map の関連試験の列（各試験の `invariant_sources`）。`--check` は候補の members と invariant の重なりを 0 件と確かめた。`duplicate_setup` の members は helper で、`keep_tests` に invariant の試験が入るのは許している（残す側なので）。

## 候補 1: duplicate_setup（49 件）
同じ本体の helper を 2 つ以上の file が写している組。47 件が merge（helper を共通の module へ移す）、2 件が delete（局所の写しを消して既存の `common::` の helper を呼ぶ）。持ち主の fragment ごと: `api-a` 25・`api-b` 6・`api-c` 1・`celeris-src` 4・`cli` 13。

| id | helper | 写しの file（件数） | 残す試験 | 案 | mutation の対象 |
|---|---|---|---|---|---|
| `dup-005f4a8d075c` | `count` | cos_live_fix_d1.rs・cos_live_fix_d2.rs・general_cos_consult.rs・unblocks.rs（4） | `task-api/tests/cos_live_fix_d1.rs::cos_live_fix_d1_cos_task_create_pins_and_audits`<br>`task-api/tests/cos_live_fix_d2.rs::cos_live_fix_d2_cos_operation_records_audit_row` | merge: count を tests/common/cos_ops.rs の db の隣へ移し、4 file の写しを消す | `task-api/src/cos/operations.rs` |
| `dup-0448468e88f5` | `browser` | browser_restore_deliver.rs・browser_restore_live_session.rs（2） | `task-api/tests/browser_restore_deliver.rs::restore_enters_observation_stop_until_session_end`<br>`task-api/tests/browser_restore_live_session.rs::restore_http_binds_to_real_isolated_session_and_never_opens_on_refusal` | merge: browser() を tests/common の browser 用 module へ移し、2 file はそれを呼ぶ | `task-api/src/browser_identity.rs` |
| `dup-11887d722d5a` | `g` | approvals.rs・changes.rs・console_instruct.rs・conversation.rs・docs.rs・execution.rs・knowledge.rs・llm_sources.rs・mcp_admin.rs・memory.rs・organization.rs・project_plan.rs・repos.rs・skills.rs（14） | `task-api/tests/approvals.rs::approvals_are_listed_oldest_first_and_can_be_filtered`<br>`task-api/tests/conversation.rs::threads_are_separated_by_project` | merge: 各 file の g を消して common::get_admin を呼ぶ | `task-api/src/middleware.rs` |
| `dup-19115125709e` | `op_body` | cos_ops.rs・cos_operations.rs（2） | `task-api/tests/cos_operations.rs::cos_chat_ops_api_task_create_writes_domain_audit_and_card_together` | merge: cos_operations.rs の op_body を消して common::cos_ops::op_body を use する | `task-api/src/cos/operations.rs` |
| `dup-2700a8f423b6` | `admin`・`admin_headers` | mod.rs・integration_api.rs・routing.rs・task_management.rs（4） | `task-api/tests/chat_api.rs::chat_api_missing_resources`<br>`task-api/tests/integration_api.rs::integration_api_disposition_requires_human_auth_and_nonempty_reason` | merge: integration_api.rs・routing.rs・task_management.rs の admin を消して common::admin_headers を呼ぶ | `task-api/src/middleware.rs` |
| `dup-2a9db82137a1` | `decision_answered_by` | cos_triage.rs・relayed_instructions.rs・single_human_window.rs（3） | `task-api/tests/cos_triage.rs::cos_chat_inbox_thread_instructed_by_relays_the_human_answer` | merge: decision_answered_by を tests/common/cos_ops.rs へ移し、3 file の写しを消す | `task-ops/src/decision.rs` |
| `dup-2e683b8ff41a` | `git` | changes.rs・docs.rs（2） | `task-api/tests/changes.rs::a_conflicting_merge_records_a_conflict_and_creates_the_child_task`<br>`task-api/tests/docs.rs::maintenance_approved_plan_attaches_to_review_task_and_stale_plan_is_refused` | merge: git helper を tests/common/mod.rs へ移し、2 file の写しを消す | `task-ops/src/changes.rs` |
| `dup-2f688511d2b8` | `cos_bearer` | cos_operations.rs・cos_triage.rs（2） | `task-api/tests/cos_operations.rs::cos_chat_ops_api_idempotency_same_hash_same_operation_different_hash_409` | merge: cos_operations.rs と cos_triage.rs の cos_bearer を消して common::cos_ops::cos_bearer を use する | `task-core/src/chat/credential.rs` |
| `dup-44d491987033` | `env_with_attachments` | cos_chat_attach_handoff_kb.rs・cos_chat_attach_pin.rs・cos_live_fix_d1.rs（3） | `task-api/tests/cos_chat_attach_pin.rs::cos_chat_attach_handoff_cos_pins_attachment_to_created_task_and_kb_candidate`<br>`task-api/tests/cos_chat_attach_handoff_kb.rs::cos_chat_attach_handoff_kb_candidate_keeps_provenance` | merge: env_with_attachments を tests/common/mod.rs へ移し、3 file の写しを消す | `task-api/src/chat/attachments.rs` |
| `dup-4841119402c0` | `ago` | cos_plan_conforms.rs・cos_plan_conforms_guards.rs（2） | `task-api/tests/cos_plan_conforms.rs::plan_conforms_approves_a_plan_whose_sensitive_stages_are_guarded` | merge: ago を conforms・ingest_plan_gate と一緒に tests/common の plan_conforms 用 module へ移す | `task-api/src/cos/basis/plan_conforms.rs` |
| `dup-547d49a44132` | `ingest_plan_gate` | cos_plan_conforms.rs・cos_plan_conforms_guards.rs（2） | `task-api/tests/cos_plan_conforms.rs::plan_conforms_approves_a_plan_whose_sensitive_stages_are_guarded`<br>`task-api/tests/cos_plan_conforms_guards.rs::a_failed_plan_check_stays_with_a_person_and_names_its_kind` | merge: ingest_plan_gate を tests/common の plan_conforms 用 module へ移し、2 file の写しを消す | `task-api/src/cos/basis/plan_conforms.rs` |
| `dup-584649db1b43` | `job_body` | cos_ops_projects_cron.rs・cron_jobs.rs（2） | `task-api/tests/cron_jobs.rs::create_rejects_duplicates_and_invalid_input`<br>`task-api/tests/cos_ops_projects_cron.rs::cos_ops_projects_cron_job_lifecycle_is_audited` | merge: job_body を tests/common/mod.rs へ移し、2 file の写しを消す | `task-api/src/cron_jobs.rs` |
| `dup-5b922c072fba` | `env` | authority_ledger.rs・relayed_instructions.rs・single_human_window.rs（3） | `task-api/tests/relayed_instructions.rs::operator_relay_grounds_the_project_cos_answer_and_separates_decider_from_executor` | merge: env を EnvOptions の既定の組として tests/common/mod.rs へ移し、3 file の写しを消す | `task-api/src/principal.rs` |
| `dup-69c83432940f` | `d` | approvals.rs・docs.rs・organization.rs・repos.rs・skills.rs（5） | `task-api/tests/approvals.rs::standing_rules_can_be_listed_created_and_deleted` | merge: common::get_admin の隣に delete_admin を置き、5 file の d を消す | `task-api/src/middleware.rs` |
| `dup-723b5c06a88c` | `admin_env`・`env`・`env_with_token`・`env_without_llm_sources`・`env_without_memory` | approvals.rs・auth_and_guards.rs・decisions.rs・llm_sources.rs・mcp_admin.rs・memory.rs・project_plan.rs・reports.rs（8） | `task-api/tests/approvals.rs::admin_operations_require_a_token_when_one_is_configured`<br>`task-api/tests/auth_and_guards.rs::malformed_and_unknown_fields_are_bad_requests` | merge: 8 file の env_with_token・admin_env 等を消して common::admin_env を呼ぶ | `task-api/src/middleware.rs` |
| `dup-8656e82c87d0` | `g`・`get` | browser_site_policy_db.rs・notify.rs（2） | `task-api/tests/browser_site_policy_db.rs::browser_site_policy_db_put_validates_and_lists` | merge: browser_site_policy_db.rs と notify.rs の写しを消して common::get_admin を呼ぶ | `task-api/src/browser_site_policies.rs` |
| `dup-882fae55cbf2` | `admin`・`auth` | auth_and_guards.rs・organization.rs・project_auto_integrate.rs（3） | `task-api/tests/auth_and_guards.rs::malformed_and_unknown_fields_are_bad_requests`<br>`task-api/tests/organization.rs::org_admin_endpoints_require_a_token` | merge: auth_and_guards.rs・organization.rs・project_auto_integrate.rs の写しを消して common::admin_headers を呼ぶ | `task-api/src/middleware.rs` |
| `dup-8bc1cc37bfe1` | `p` | campaign_ident.rs・campaign_variant_atomic.rs（2） | `task-api/tests/campaign_ident.rs::create_stores_campaign_identity_and_resend_is_idempotent` | merge: 2 file の p を消して common::post_admin("/api/v1/tasks", body) を呼ぶ | `task-api/src/handlers/tasks.rs` |
| `dup-a61acf40f2e2` | `p` | approvals.rs・changes.rs・console_instruct.rs・conversation.rs・docs.rs・knowledge.rs・memory.rs・organization.rs・project_plan.rs・repos.rs・skills.rs（11） | `task-api/tests/approvals.rs::once_reopens_the_task_through_the_existing_answer_path`<br>`task-api/tests/conversation.rs::talking_to_a_node_returns_202_and_makes_one_ready_conversation_task` | merge: 各 file の p を消して common::post_admin を呼ぶ | `task-api/src/middleware.rs` |
| `dup-ccb520e5870b` | `db` | cos_ops.rs・cos_chat_triage_scenarios.rs・cos_job_wait_extend.rs・cos_operations.rs・cos_plan_conforms.rs・cos_plan_conforms_guards.rs・cos_review_rework.rs・cos_routine_answers.rs・cos_triage.rs（9） | `task-api/tests/cos_operations.rs::cos_chat_ops_api_task_create_writes_domain_audit_and_card_together`<br>`task-api/tests/cos_triage.rs::cos_chat_triage_escalate_claims_one_outbox_row` | merge: 8 file の db を消して common::cos_ops::db を use する | `task-api/src/cos/operations.rs` |
| `dup-dc90ec2eb7e4` | `post_op` | cos_chat_attach_pin.rs・cos_operations.rs（2） | `task-api/tests/cos_operations.rs::cos_chat_ops_api_idempotency_same_hash_same_operation_different_hash_409` | merge: post_op を tests/common/cos_ops.rs へ移し、cos_chat_attach_pin.rs と cos_operations.rs の写しを消す | `task-api/src/cos/operations.rs` |
| `dup-e37d37ba637d` | `bridge_binary` | browser_e2e.rs・browser_h3_injection.rs（2） | `task-api/tests/browser_e2e.rs::denied_approval_fails_the_task_without_a_lease` | merge: bridge_binary を tests/common の browser 用 module へ移し、2 file の写しを消す | `task-worker/src/browser_credential.rs` |
| `dup-eb0a968e2ac9` | `raise_decision` | cos_operations_domains.rs・cos_triage.rs・relayed_instructions.rs・single_human_window.rs（4） | `task-api/tests/cos_operations_domains.rs::cos_chat_ops_domain_decision_answer` | merge: raise_decision を tests/common/cos_ops.rs へ移し、4 file の写しを消す | `task-ops/src/decision.rs` |
| `dup-ec52cf15de3e` | `admin_env`・`env_with_token` | mod.rs・routing.rs・task_management.rs（3） | `task-api/tests/chat_api.rs::chat_api_auth_and_input_errors`<br>`task-api/tests/browser_doctor.rs::browser_doctor_readiness_requires_token_and_uses_daemon_probe` | merge: routing.rs と task_management.rs の env_with_token を消して common::admin_env を呼ぶ | `task-api/src/middleware.rs` |
| `dup-f8949027f55a` | `conforms` | cos_plan_conforms.rs・cos_plan_conforms_guards.rs（2） | `task-api/tests/cos_plan_conforms.rs::plan_conforms_refuses_other_answers_stale_versions_and_unguarded_stages` | merge: conforms を ago・ingest_plan_gate と一緒に tests/common の plan_conforms 用 module へ移す | `task-api/src/cos/basis/plan_conforms.rs` |
| `dup-0629b21af39d` | `env` | docs.rs・execution.rs・knowledge.rs・repos.rs・skills.rs（5） | `task-api/tests/docs.rs::editing_a_page_commits_on_the_default_branch_and_checks_the_etag`<br>`task-api/tests/execution.rs::posting_without_a_token_is_unauthorized`<br>`task-api/tests/knowledge.rs::the_write_endpoints_are_admin_only`<br>`task-api/tests/repos.rs::changing_repos_needs_the_admin_token_but_reading_does_not`<br>`task-api/tests/skills.rs::put_skill_creates_and_updates_and_requires_admin` | delete: docs・execution・knowledge・repos・skills の局所の env() を消し、呼び出しを common::admin_env() に置き換える | `task-api/src/middleware.rs` |
| `dup-3b3dd18cd349` | `bearer_for` | project_brief.rs・project_controller.rs・project_roadmap.rs（3） | `task-api/tests/project_brief.rs::project_status_put_is_written_by_the_project_cos_only`<br>`task-api/tests/project_controller.rs::project_wake_api_cos_may_only_turn_it_off`<br>`task-api/tests/project_roadmap.rs::project_roadmap_cos_and_operator_cannot_decide` | merge: cos_bearer から thread id を受ける cos_run_bearer(env, thread_id, key) を common/cos_ops.rs に切り出し、3 file の bearer_for を消してそれを呼ぶ | `task-api/src/cos/mod.rs` |
| `dup-3dab5992ab4f` | `pa` | organization.rs・project_plan.rs・repos.rs（3） | `task-api/tests/organization.rs::org_can_be_created_listed_patched_and_deleted`<br>`task-api/tests/project_plan.rs::project_plan_endpoints_are_gone`<br>`task-api/tests/repos.rs::repos_can_be_created_listed_repointed_and_deleted` | delete: organization・project_plan・repos の局所の pa() を消し、呼び出しを common::patch_admin() に置き換える | `task-api/src/handlers/projects.rs` |
| `dup-6f1147b9302e` | `seed_secretary` | memory.rs・project_plan.rs（2） | `task-api/tests/memory.rs::existing_notes_and_a_projects_drawer_are_returned_in_full`<br>`task-api/tests/project_plan.rs::project_detail_hides_frozen_milestones_by_default` | merge: seed_secretary(app) を post_admin() で書き直して tests/common に置き、memory.rs と project_plan.rs の写しを消す | `task-api/src/handlers/org.rs` |
| `dup-8d936b305a9e` | `pu` | docs.rs・knowledge.rs・skills.rs（3） | `task-api/tests/docs.rs::editing_a_page_commits_on_the_default_branch_and_checks_the_etag`<br>`task-api/tests/knowledge.rs::writing_a_page_checks_the_etag_and_the_path`<br>`task-api/tests/skills.rs::put_skill_creates_and_updates_and_requires_admin` | merge: common に put_admin(path, body) を patch_admin() の隣に足し、3 file の局所の pu() を消してそれを呼ぶ | `task-api/src/knowledge.rs` |
| `dup-ad54b1ce2c16` | `run_bearer` | general_cos_consult.rs・single_human_window.rs（2） | `task-api/tests/general_cos_consult.rs::general_cos_consult_proposal_is_cos_words_and_never_an_instruction`<br>`task-api/tests/single_human_window.rs::project_escalation_is_answered_through_the_general_cos_and_relayed_to_the_project_cos` | merge: dup-3b3dd18cd349 で切り出す common/cos_ops.rs の helper に policy の版を引数で渡す bearer 発行を置き、2 file の run_bearer を消してそれを呼ぶ | `task-api/src/cos/mod.rs` |
| `dup-f48e0db4164e` | `seq` | job_wait_extend.rs・review_rework.rs（2） | `task-api/src/cos/basis/job_wait_extend.rs::tests::timed_out_wait_is_none_for_a_satisfied_wait_or_a_wait_after_the_question`<br>`task-api/src/cos/basis/job_wait_extend.rs::tests::timed_out_wait_reads_the_wait_before_the_question`<br>`task-api/src/cos/basis/job_wait_extend.rs::tests::timeouts_count_only_waits_sharing_a_job`<br>`task-api/src/cos/basis/review_rework.rs::tests::rejection_is_none_without_a_reviewer_verdict_or_after_a_reopen`<br>`task-api/src/cos/basis/review_rework.rs::tests::rejection_reads_the_failing_reviewer_verdicts_of_the_last_run` | merge: cos/basis配下のcfg(test)共通helperへ連番付与だけを移し、待ち延長とreview差し戻しの全5試験と固有event列は残す。 | `task-api/src/cos/basis/job_wait_extend.rs` |
| `dup-24ecb5f086e4` | `hook` | e2e.rs・credential_wait_notify.rs（2） | `celeris/tests/credential_wait_notify.rs::webhook_ignores_eof_before_complete_request` | merge: hook を既に両方が #[path] で読む tests/support/http_async.rs に webhook 受け口として移し、2 file の写しを消す | `celeris/src/notify.rs` |
| `dup-70424d48a190` | `load` | jev.rs・routing_mode.rs・script.rs（3） | `celeris/src/config/jev.rs::tests::every_field_round_trips`<br>`celeris/src/config/routing_mode.rs::tests::both_modes_parse`<br>`celeris/src/config/script.rs::tests::script_runtime_config_reads_values_and_defaults` | merge: load を config の cfg(test) の共通 helper（pub(super)）に 1 つ置き、jev.rs・routing_mode.rs・script.rs の試験はそれを呼ぶ | `celeris/src/config/mod.rs` |
| `dup-922d1bc127c1` | `git` | tests.rs・daily_curation.rs（2） | `celeris/src/knowledge_curation/tests.rs::knowledge_curation_apply_commits_and_pushes_without_approval`<br>`celeris/tests/daily_curation.rs::knowledge_curation_job_apply_without_approval_commits_pushes_and_terminal_task_maintenance` | merge: git helper を tests/support/ の file に置き、knowledge_curation/tests.rs と tests/daily_curation.rs は #[path] で読む | `celeris/src/knowledge_curation.rs` |
| `dup-e627cfc45028` | `write_executable` | adapters.rs・tests.rs（2） | `celeris/src/daemon/adapters.rs::harness_adapter_tests::pi_harness_declaration_reaches_pi_process`<br>`celeris/src/model_discovery/tests.rs::codex_pages_through_model_list_over_stdio` | merge: write_executable を celeris の cfg(test) の共通 helper 1 つにし、adapters.rs の試験 module と model_discovery/tests.rs はそれを呼ぶ | `celeris/src/model_discovery.rs` |
| `dup-0904376f7e74` | `dechunk` | campaign_acceptance_e2e.rs・campaign_lock_e2e.rs（2） | `celeris/tests/campaign_acceptance_e2e.rs::normal_path_post_tasks_to_independent_grade_to_final_export`<br>`celeris/tests/campaign_lock_e2e.rs::locked_measurement_runs_through_task_api_to_the_final_export` | merge: dechunk を 2 file が #[path] で読む campaign の試験の共通 module（crates/campaign/tests/support_cases など）に 1 つ置き、写しを消す | `campaign/src/celeris_exec.rs` |
| `dup-144d72381022` | `git` | stranded_default_branch.rs・stranded_integration.rs（2） | `celeris/tests/stranded_default_branch.rs::stranded_default_branch_null_with_main_is_integrated`<br>`celeris/tests/stranded_integration.rs::stranded_detects_inbox_merge_discard_hold_and_promotion_independently` | merge: git helper を crates/celeris/tests/support/mod.rs に移し、2 file は mod support から使う | `celeris/src/daemon/stranded_sweep.rs` |
| `dup-22354769b5d1` | `wait_until` | instance_handoff.rs・unpromoted_release.rs（2） | `celeris/tests/instance_handoff.rs::a_newer_release_takes_over_while_the_old_one_finishes_its_run`<br>`celeris/tests/instance_handoff.rs::a_stale_heartbeat_does_not_replace_a_live_active_until_its_process_is_gone` | merge: wait_until を crates/celeris/tests/support/mod.rs に移して 2 file から使う（出来事待ちへの置き換えは別の判断） | `celeris/src/instance.rs` |
| `dup-62cc1d4b470e` | `ev_started`・`started` | campaign_acceptance_e2e.rs・campaign_lock_e2e.rs・campaign_wiring_e2e.rs（3） | `celeris/tests/campaign_wiring_e2e.rs::post_tasks_to_export_connects_children_retries_review_cos_jev_and_interventions`<br>`celeris/tests/campaign_acceptance_e2e.rs::normal_path_post_tasks_to_independent_grade_to_final_export` | merge: ev_started を ev_finished（dup-712f03d8c8d3）と一緒に campaign の試験の共通 module に移し、3 file から使う | `task-dispatch/src/dispatcher/decision_log_wire.rs` |
| `dup-69e6476ee890` | `ctl` | mcp_client_scopes.rs・no_migrate.rs（2） | `celerisctl/tests/no_migrate.rs::celerisctl_refuses_an_older_db_and_leaves_its_schema_alone`<br>`celerisctl/tests/mcp_client_scopes.rs::scopes_replaces_the_scope_list_and_keeps_the_token` | merge: env を外した Command を返す 1 関数を crates/celerisctl/tests/common/mod.rs に置き、--db の有無は呼び出し側で足す（dup-f65355edd1ac と同じ関数） | `celerisctl/src/main.rs` |
| `dup-712f03d8c8d3` | `ev_finished` | campaign_acceptance_e2e.rs・campaign_lock_e2e.rs（2） | `celeris/tests/campaign_acceptance_e2e.rs::final_accounting_is_regenerated_from_the_router_tree_with_jev_cos_gaps`<br>`celeris/tests/campaign_lock_e2e.rs::locked_measurement_runs_through_task_api_to_the_final_export` | merge: ev_finished を ev_started と一緒に campaign の試験の共通 module に移し、2 file から使う | `task-dispatch/src/dispatcher/decision_log_wire.rs` |
| `dup-774ffe890ee1` | `write_executable` | cap_probe_real_daemon.rs・ops_r1_measure.rs（2） | `celeris/tests/cap_probe_real_daemon.rs::daemon_only_toolchain_and_variable_pass_through_the_real_worker_path`<br>`celeris/tests/ops_r1_measure.rs::ops_r1_measure_cos_path_with_stub_harness` | merge: write_executable を crates/celeris/tests/support/mod.rs に移して 2 file から使う | `campaign/src/cap_probe.rs` |
| `dup-843a2fd1e3a4` | `run_daemon` | script_compare_cos_digest.rs・script_compare_lane_rules.rs（2） | `celeris/tests/script_compare_lane_rules.rs::script_compare_daemon_injects_records_and_restores_rust`<br>`celeris/tests/script_compare_cos_digest.rs::script_compare_cos_digest_daemon_restart_without_bundle_restores_rust` | merge: run_daemon を crates/celeris/tests/support/script_compare.rs に移し、2 file の写しを消す | `celeris/src/daemon/script_layer.rs` |
| `dup-bc38edb0ba29` | `is_timeout` | script_compare_cos_digest.rs・script_generation_e2e.rs・script_compare.rs（3） | `celeris/tests/script_compare_cos_digest.rs::script_compare_cos_digest_daemon_restart_without_bundle_restores_rust`<br>`celeris/tests/script_compare_lane_rules.rs::script_compare_daemon_injects_records_and_restores_rust` | merge: 2 file の写しを消して support::script_compare::is_timeout を使う（script_generation_e2e には mod support を足す） | `celeris/src/daemon/script_layer.rs` |
| `dup-d504e49feb3f` | `status_kb` | long_run_soak.rs・project_controller_replay.rs（2） | `celeris/tests/project_controller_replay.rs::project_controller_replay_resource_v8_100_projects` | merge: status_kb・pct・threads を計測用の共通 module（crates/celeris/tests/support/ の下）に移し、2 file から使う | `task-dispatch/src/dispatcher/project_decider_ts.rs` |
| `dup-e49ce410e3fa` | `hook` | cos_chat_triage_scenarios.rs・root_done_summary.rs（2） | `celeris/tests/cos_chat_triage_scenarios.rs::cos_chat_triage_b_webhook_packet_and_stop_after_human_answer`<br>`celeris/tests/cos_chat_triage_scenarios.rs::cos_chat_triage_unified_only_cos_escalation_reaches_the_webhook` | merge: Hook と hook を crates/celeris/tests/support/mod.rs に移し、2 file から使う（credential_wait_notify の別の hook は本体が違うので含めない） | `celeris/src/notify.rs` |
| `dup-f65355edd1ac` | `ctl` | campaign_export.rs・eval_requests.rs・routing_cli.rs（3） | `celerisctl/tests/routing_cli.rs::routing_export_emits_one_training_row_and_supports_text_omission`<br>`celerisctl/tests/eval_requests.rs::renotify_and_restart_do_not_add_rows` | merge: dup-69e6476ee890 と同じ env を外した Command の関数を crates/celerisctl/tests/common/mod.rs に置き、3 file から使う | `celerisctl/src/main.rs` |
| `dup-fc9d9545a731` | `pct` | long_run_soak.rs・project_controller_replay.rs（2） | `celeris/tests/project_controller_replay.rs::project_controller_replay_resource_v8_100_projects` | merge: pct を status_kb・threads と一緒に計測用の共通 module に移し、2 file から使う | `task-dispatch/src/dispatcher/project_decider_ts.rs` |

## 候補 2: equivalent_scenario（4 件）
同じ file の中で、literal を伏せた本体が同じ試験の組。4 件とも merge（入力を行に持つ表駆動の 1 本に束ね、どの入力も残す）。

| id | members | 残す試験 | 案 | mutation の対象 |
|---|---|---|---|---|
| `eq-22eed0cc9a76` | `celerisctl/src/commands/query_tests.rs::run_show_found_task_succeeds`<br>`celerisctl/src/commands/query_tests.rs::run_show_json_prints_task_detail_with_actions_and_matches_task_id` | `celerisctl/src/commands/query_tests.rs::run_show_found_task_succeeds` | merge: json の false・true の 2 行の表で回す 1 本にし、JSON の本文は task_detail_json_is_parseable_and_contains_task_id_and_actions に任せる | `celerisctl/src/commands/query.rs` |
| `eq-8b8e2eda5104` | `celerisctl/src/commands/query_tests.rs::run_show_json_missing_task_errors`<br>`celerisctl/src/commands/query_tests.rs::run_show_missing_task_errors` | `celerisctl/src/commands/query_tests.rs::run_show_missing_task_errors` | merge: json の false・true の 2 行の表で回す 1 本にする | `celerisctl/src/commands/query.rs` |
| `eq-f044c8f087db` | `celerisctl/tests/worker_run.rs::error_with_provider_failure_exits_four_and_preserves_provider_failure`<br>`celerisctl/tests/worker_run.rs::question_exits_three` | `celerisctl/tests/worker_run.rs::error_with_provider_failure_exits_four_and_preserves_provider_failure` | merge: script の出力・期待 exit code・stdout の断片の表で回す 1 本にし、question と provider_failure の 2 行を残す | `celerisctl/src/commands/worker.rs` |
| `eq-f91adbda0b9c` | `task-api/tests/chat_attachments.rs::chat_attach_api_rejects_duplicate_file_fields`<br>`task-api/tests/chat_attachments.rs::chat_attach_api_requires_client_upload_id` | `task-api/tests/chat_attachments.rs::chat_attach_api_requires_client_upload_id` | merge: 2 つの multipart 本文を行に持つ表駆動の 1 本に束ね、どちらの入力も残す | `task-api/src/chat/attachments.rs` |

## 候補 3: old_impl（3 件）
消した仕組み・旧い API や挙動を固定する試験。DB に残る旧い shape の event を読む replay の契約の試験（`task-api/tests/legacy_routing_events.rs`・`celeris/tests/legacy_event_startup.rs`）は入れていない。詳細は [`old-impl.md`](old-impl.md)。

| id | members | 根拠 | 残す試験 | 案 | mutation の対象 |
|---|---|---|---|---|---|
| `old-incremental-backup-measurement` | `celeris/src/db_maintenance/tests.rs::incremental_backup_progress_restarts_after_other_connection_writes` | 件名「定期DBバックアップの再開始ループと中断遅延を解消」: 定期 backup の旧方式（rusqlite の online backup を 1 page ずつ step する）。VACUUM INTO に置き換えた | `celeris/src/db_maintenance/tests.rs::vacuum_backup_finishes_with_a_concurrent_writer_and_integrity_ok`<br>`celeris/src/db_maintenance/tests.rs::vacuum_progress_handler_interrupts_an_active_copy` | delete: 旧方式の rusqlite Backup::step の再開始を計測するだけで celeris の code を呼ばない試験を消し、置き換えの理由は backup_once の注釈と commit に残す | `celeris/src/db_maintenance.rs` |
| `old-celerisctl-plan-legacy-path` | `celerisctl/src/commands/plan.rs::tests::run_inserts_plan_task`<br>`celerisctl/src/commands/plan.rs::tests::run_with_blank_goal_returns_error` | `agent-docs/adr/0079-recursive-task-decomposition.md`: kind=plan の分解 task を作る入口。POST /plans は R5a で 410、celerisctl plan は自ら legacy path と注記して残る | `celerisctl/src/commands/execution_tests.rs::set_adopts_a_v3_plan_with_the_configured_tree_limits`<br>`celerisctl/src/commands/execution_tests.rs::set_rejects_an_invalid_plan` | delete: celerisctl plan（plan.rs と main.rs の Plan 枝）を消す変更で 2 試験も消す。新しい仕事は root task と execution plan set（plan/3）で作る | `celerisctl/src/commands/execution.rs` |
| `old-console-milestone-review-block` | `task-api/tests/console.rs::approvals_milestones_and_questions_carry_their_state` | `agent-docs/adr/0079-recursive-task-decomposition.md`: 途中目標の判定の対話（ADR-0038）。R5a 9 は Console の milestone_review の読み手を R5b/R6 の回収で外すと決めた | `task-api/tests/project_plan.rs::project_detail_hides_frozen_milestones_by_default` | merge: console.rs の途中目標の節（proposed_milestones と ConsoleBlock::Milestone）を消す変更で、この試験から milestone の assert を外し、認可・質問のブロックの assert は残す | `task-api/src/console.rs` |

## 採らなかった組（34 件）
`duplicate_setup` 32 件、`equivalent_scenario` 2 件。理由（違う fixture を起動する・本体が数 token で束ねても変わらない など）は `contracts.json` の `rejected_groups[].reason` と各 map leaf の進捗にある。

## extra_capabilities（B0 capability-map に足す提案）
map leaf が足した 37 件。同じ id を複数の fragment が書いた 10 件（`cluster-admin-connect`・`task-genres-roles`・`knowledge-base`・`task-read-views`・`local-machine-settings`・`model-catalog`・`org-projects-reports`・`project-repos-workspace`・`knowledge-pages-skills`・`terminal-sessions`）は name・owner_paths・reason が一致する。
提案: 31 件はそのまま B0 に足す、3 件は重なる capability に統合する、2 件は owner の割り振りを決めてから足す、1 件（`test-fixtures-shared`）は足さない。B0 の capability-map は R5 の write set の外なので、ここでは変えていない。
ほかに、`task-api/src/handlers/clusters.rs` は 3 件（`cluster-admin-connect`・`daemon-snapshot-config-api`・`local-machine-settings`）の owner で、`task-core/src/model.rs` は B0 の `task-lifecycle` と `task-genres-roles` の owner。1 つの file に別の契約の handler が同居しているためで、統合の対象にはしない。

| id | name | 定義した fragment | 試験 | owner_paths | 提案 |
|---|---|---|---|---|---|
| `api-secret-administration` | 管理APIによる秘密fileの保管と非公開metadata | api-c | 19 | `task-api/src/secrets.rs` | 足す |
| `approvals-standing-rules` | worker の認可要求（approval）への人の決定と standing rule | api-a | 14 | `task-api/src/approvals.rs`<br>`task-core/src/approval.rs`<br>`task-ops/src/approval.rs` | 足す |
| `browser-doctor` | browser 経路の本番前提（launcher・DNS・broker・台帳・鍵・site policy）の点検 | celeris-src | 6 | `celeris/src/browser_doctor.rs` | 足す |
| `celeris-daemon-wiring` | celeris daemon の起動順・配線と tick ループ（celeris::lib・daemon の bootstrap と tick_loop） | cli | 4 | `celeris/src/lib.rs`<br>`celeris/src/daemon/bootstrap.rs`<br>`celeris/src/daemon/tick_loop.rs` | `daemon-bootstrap-reload` に統合（owner の `lib.rs`・`daemon/bootstrap.rs` が重なる。`daemon/tick_loop.rs` を owner に足す） |
| `cluster-admin-connect` | クラスタの接続管理と作業ディレクトリ設定の API | api-a・cli | 23 | `task-api/src/handlers/clusters.rs` | 足す |
| `cluster-ssh-session` | クラスタへの ssh 接続（login session・ControlPath の点検・cluster 設定・最初の tick の接続） | celeris-src | 21 | `celeris/src/cluster_admin.rs`<br>`celeris/src/control_path.rs`<br>`celeris/src/config/cluster.rs` | 足す |
| `config-load-validation` | celeris.toml の読み込み・既定値・相対 path の解決・検証（消した欄の拒否を含む） | celeris-src | 15 | `celeris/src/config/mod.rs` | 足す |
| `config-runtime-edit` | 設定の実行中の変更（欄の分類・秘密の伏せ字・comment を保つ書き戻し・再起動なしの反映） | celeris-src | 11 | `celeris/src/config/edit.rs`<br>`celeris/src/config/fields.rs`<br>`celeris/src/daemon/config_admin.rs` | 足す |
| `console-feed` | console の feed（block 列・cursor・SSE）と console からの指示 | api-a | 17 | `task-api/src/console.rs`<br>`task-ops/src/console.rs` | 足す |
| `cos-authority-ledger` | CoS の操作の許可（grant）の台帳と分類ごとの拒否 | api-a | 21 | `task-core/src/authority.rs`<br>`task-api/src/handlers/authority.rs` | 足す |
| `daemon-bootstrap-reload` | celeris daemon の起動・reload・停止と背景の周期 task の配線（config → dispatcher・API・log） | celeris-src | 5 | `celeris/src/lib.rs`<br>`celeris/src/daemon.rs`<br>`celeris/src/daemon/bootstrap.rs`<br>`celeris/src/daemon/admin.rs`<br>`celeris/src/daemon/process_memory.rs` | 足す（`celeris-daemon-wiring` を統合して受ける） |
| `daemon-snapshot-config-api` | daemon の snapshot・watch stream・GET /config・/clusters・/schema の読み取り API | api-b | 7 | `task-api/src/handlers/system.rs`<br>`task-api/src/handlers/clusters.rs`<br>`task-api/src/schema.rs`<br>`task-api/src/sse.rs` | 足す |
| `db-backup-maintenance` | 本番 DB の backup・世代の保持・WAL checkpoint・VACUUM の保守 | celeris-src | 14 | `celeris/src/db_maintenance.rs` | 足す（`db-maintenance` を統合して受ける） |
| `db-maintenance` | DB の backup と integrity check（celerisctl db・celeris::db_maintenance） | cli | 3 | `celerisctl/src/commands/db.rs`<br>`celeris/src/db_maintenance.rs` | `db-backup-maintenance` に統合（owner の `db_maintenance.rs` が重なる。`celerisctl/src/commands/db.rs` を owner に足す） |
| `doc-gardener` | 案件の文書 repo の定期の文書整理（走査と review の予約） | celeris-src | 2 | `celeris/src/doc_gardener.rs` | 足す |
| `event-replay` | events からの表の再構築と照合（task_ops::replay・celerisctl replay） | cli | 2 | `task-ops/src/replay.rs`<br>`celerisctl/src/commands/replay.rs` | 足す |
| `execution-metrics` | 実行索引とeventに基づく使用量・continuation集計 | api-c | 3 | `task-api/src/stats.rs` | 足す |
| `knowledge-base` | 知識ベースの page・tree・候補（_inbox）の API と人の受け入れ（ADR-0068） | api-b・cli | 15 | `task-api/src/knowledge.rs`<br>`task-ops/src/knowledge.rs`<br>`task-core/src/knowledge.rs` | 足す |
| `knowledge-curation-gc` | 知識ベースの日次整理 job・curation plan の検証・GC（celeris::knowledge_curation 等） | cli | 12 | `celeris/src/knowledge_curation.rs`<br>`celeris/src/knowledge_gc.rs`<br>`celerisctl/src/commands/curation.rs` | `knowledge-curation-jobs` に統合（owner の `knowledge_curation.rs`・`knowledge_gc.rs` が重なる。`celerisctl/src/commands/curation.rs` を owner に足す） |
| `knowledge-curation-jobs` | 知識の日次抽出・整理 job・知識整理 run・知識 GC（KB への適用と受信箱） | celeris-src | 45 | `celeris/src/knowledge_curation.rs`<br>`celeris/src/knowledge_maint.rs`<br>`celeris/src/knowledge_gc.rs`<br>`celeris/src/config/knowledge.rs` | 足す（`knowledge-curation-gc` を統合して受ける） |
| `knowledge-pages-skills` | 知識pageとskillの管理・組織へのmount | api-c・cli | 6 | `task-api/src/knowledge.rs`<br>`task-api/src/skills.rs` | 足す前に割り振りを決める（案: page の部分は `knowledge-base`、`skills.rs` の CRUD と mount は skill 管理の capability に分ける） |
| `local-machine-settings` | ローカルマシン設定（CLI > store > 設定 file の優先順位・PUT の監査と反映） | api-b・celeris-src・cli | 9 | `task-core/src/local_machine.rs`<br>`task-core/src/store/local_machine.rs`<br>`task-api/src/handlers/clusters.rs` | 足す |
| `model-catalog` | model catalog の発見・人の上書き・routing への反映（ADR 2026-10-06） | api-b・celeris-src・cli | 19 | `task-core/src/model_catalog.rs`<br>`task-core/src/store/model_catalog.rs`<br>`task-api/src/model_catalog.rs` | 足す |
| `org-conversation` | 組織の node への話しかけと対話用 task・thread | api-a | 7 | `task-api/src/conversation.rs`<br>`task-ops/src/conversation.rs` | 足す |
| `org-projects-reports` | 組織・案件・途中目標・報告の API（DB が正本、ADR-0033） | api-b・celeris-src・cli | 36 | `task-core/src/org.rs`<br>`task-core/src/store/org.rs`<br>`task-core/src/store/projects.rs`<br>`task-core/src/report.rs`<br>`task-api/src/handlers/org.rs`<br>`task-api/src/handlers/projects.rs`<br>`task-api/src/reports.rs` | 足す |
| `project-docs` | 案件の文書 repo の page の読み書き・成果物の昇格・文書整理（ADR-0044） | api-b | 11 | `task-api/src/docs.rs`<br>`task-ops/src/docs.rs` | 足す |
| `project-repos-workspace` | 案件の repo と task の作業場所（案件 × repo の worktree、ADR-0043） | api-b・celeris-src | 14 | `task-core/src/repos.rs`<br>`task-core/src/store/repos.rs`<br>`task-api/src/repos.rs` | 足す |
| `repo-health-checker-path` | daemon が release に同梱された repo health 検査器（CHECKER_TREE）の基底式を引く | celeris-src | 3 | `celeris/src/daemon/repo_health_path.rs` | 足す |
| `repository-docs-artifacts` | repository文書と成果物の安全なfile配信 | api-c | 10 | `task-api/src/docs.rs`<br>`task-api/src/files.rs`<br>`task-api/src/tree.rs` | 足す前に割り振りを決める（案: `docs.rs` は `project-docs`、`files.rs`・`tree.rs` は `task-files-artifacts` に寄せる） |
| `skills-delivery` | skill の worker 作業場所への配送（task-worker::skills、ADR-0122） | cli | 4 | `task-worker/src/skills.rs` | 足す |
| `task-files-artifacts` | run の log と成果物の file API（path 検査・Content-Type 表・Range・sha256） | api-b | 10 | `task-api/src/files.rs` | 足す |
| `task-genres-roles` | task の genre・role の検証と既定の適用（[[genres]]・[[roles]]） | api-b・celeris-src・cli | 23 | `celeris/src/config/harness.rs`<br>`task-api/src/handlers/tasks.rs`<br>`task-core/src/model.rs` | 足す |
| `task-read-views` | task の一覧・詳細・events・graph の読み取り view（task-ops::view、ADR-0004） | api-b・cli | 24 | `task-ops/src/view.rs`<br>`task-api/src/query.rs` | 足す |
| `terminal-sessions` | 人のWeb端末sessionとcluster接続条件・開閉監査 | api-c・cli | 9 | `task-api/src/handlers/terminals.rs` | 足す |
| `test-fixtures-shared` | 試験の土台（一時 daemon の待ち・偽 HTTP の読み取り・実 CLI の解決） | cli | 12 | `celeris/tests/daemon_port/mod.rs`<br>`celeris/tests/support/mod.rs`<br>`celerisctl/tests/common/mod.rs` | 足さない（製品の capability でなく試験の土台。R5 の対応表の中だけで使う） |
| `worker-followup-tasks` | worker の run が宣言した後続 task（followups.json、ADR-0098） | cli | 2 | `task-ops/src/followup.rs`<br>`task-dispatch/src/dispatcher/followups.rs` | 足す |
| `worker-memory-read` | 組織 node の記憶 file の読み取り API（ADR-0033 D6） | api-b | 4 | `task-api/src/memory.rs`<br>`task-ops/src/memory.rs` | 足す |

## R5 の後の unit への提案（mutation・故障注入で確かめてから消す順）
前提が 2 つある。
- B0 slices.json の R5 は「試験関数の数が減る → 止めて replan」を止まる条件に持つ。試験の数が減るのは `equivalent_scenario` の 4 件（束ねると −4）と `old_impl` の delete 2 件（−3）で、段 dedupe の人の決定 `test-removal` が要る。`duplicate_setup` は helper を寄せるだけで試験の数は減らない。
- R5 の write set（B0 slices.json）は数字を含む file 名と試験でない src の file を含まない。`duplicate_setup` 49 件のうち 37 件は member の file が全部 write set に入る。残る 12 件（`dup-005f4a8d075c`・`dup-0904376f7e74`・`dup-24ecb5f086e4`・`dup-44d491987033`・`dup-62cc1d4b470e`・`dup-70424d48a190`・`dup-712f03d8c8d3`・`dup-774ffe890ee1`・`dup-bc38edb0ba29`・`dup-e37d37ba637d`・`dup-e627cfc45028`・`dup-f48e0db4164e`）は `cos_live_fix_d1.rs`・`campaign_acceptance_e2e.rs`・`browser_h3_injection.rs` のような数字を含む file か、`config/jev.rs`・`daemon/adapters.rs` のような試験でない src の file の inline の試験 module を変える。`old-celerisctl-plan-legacy-path`（`commands/plan.rs`・`main.rs`）と `old-console-milestone-review-block`（`task-api/src/console.rs`）も src の本体を変える。これらは R5 では扱えず、R6（旧経路の収束）か別の task に回す。

確かめ方（どの段でも同じ）: 変える前と後で、`mutation_target` の file に絞った mutation（cargo-mutants など。host に入っているかは未確認）を流し、捕まった mutant の集合を比べる。集合が減ったら変更をやめる。時間に依存する経路は、注入した時計・SIGSTOP/SIGCONT・試験専用の遅延フックで確かめ、CPU の負荷はかけない（`agent-docs/guides/testing.md`）。`mutation_target` の 11 件は task-api・celeris・celerisctl の外（task-dispatch 4・task-ops 3・campaign 2・task-core 1・task-worker 1）にあるので、その crate の mutant に対して task-api・celeris の試験を流す設定が要る。

順:
1. `duplicate_setup` の write set 内 37 件（試験の数は変わらない）。`mutation_target` が同じ組を 1 つの変更に束ね、多いものから進める: `task-api/src/middleware.rs` 8 件 → `task-api/src/cos/operations.rs` 3 件・`task-api/src/cos/basis/plan_conforms.rs` 3 件 → 2 件ずつの `task-ops/src/decision.rs`・`task-api/src/cos/mod.rs`・`celerisctl/src/main.rs`・`task-dispatch/src/dispatcher/project_decider_ts.rs` → 残りの 1 件ずつ。helper の本体の hash は同じなので集合は変わらないはずで、変わったら意味の違う fixture だった証拠として merge をやめる。
2. `equivalent_scenario` 4 件（`test-removal` の決定の後）。束ねる前に members の各試験が捕まえる mutant を記録し、表駆動の 1 本がその和集合を捕まえることを確かめてから元の 2 本を消す。対象は `celerisctl/src/commands/query.rs`（2 件）・`celerisctl/src/commands/worker.rs`・`task-api/src/chat/attachments.rs`。
3. `old-incremental-backup-measurement`（`test-removal` の決定の後、R5 の write set 内）。消す前に、この試験が `celeris/src/db_maintenance.rs` の mutant を 1 つも捕まえないこと（celeris の code を呼ばない）と、残す 2 試験（並行の書き込み・中断）が VACUUM の経路の mutant を捕まえることを確かめる。中断の経路は progress handler の試験用の hook で決定的に起こす。
4. R5 の write set の外の 12 件と `old_impl` の 2 件は R6 か別の task で扱う。`old-celerisctl-plan-legacy-path` は `celerisctl plan` を消す変更と同じ変更で、先に `celerisctl/src/commands/execution.rs` の mutant を残す 2 試験が捕まえることを確かめる。`old-console-milestone-review-block` は Console の途中目標の節を消す変更と同じ変更で、認可・質問のブロックの mutant が捕まったままであることを確かめる。

ほかの提案:
- B0 capability-map への extra_capabilities の取り込み（上の節）は R5 の write set の外。B0 の capability-map を直す task で扱う。
- architecture-map の関連試験の列が file を丸ごと挙げるので、死んだ経路を固定する試験も `invariant=true` になる（[`old-impl.md`](old-impl.md) の提案 1）。地図の行を試験関数の単位に絞ると、old_impl の候補を挙げられる範囲が広がる。

## 検査（提出時の HEAD で実行）
（後で埋める）

## 未解決事項
- B0 capability-map に extra_capabilities を足す変更・R5 の write set の外の候補 14 件は、この unit では扱っていない（上の提案）。
- mutation の道具が host に入っているか、`mutation_target` の各 module で mutation が何分かかるかは未確認。

## commit
- `R5 assemble: contracts.json を組み立て、--check を通し、候補の要約を進捗に書く`
