---
title: 健全性v2 R5 map-celeris-src — celeris src の試験 module 537 件に契約を付け、組を判断する
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
---
# 健全性v2 R5 map-celeris-src — celeris src の試験 module 537 件に契約を付け、組を判断する

## 結果
- commit 済みの雛形 [`mapping/celeris-src.json`](mapping/celeris-src.json)（fragment `celeris-src`、scope `^crates/celeris/src/`）をそのまま埋めた。`--scaffold` は走らせていない。`scan` 欄は変えていない（走査の commit は scanner の commit、tree は雛形のまま）。
- 対象は 60 file・537 試験（invariant 353、invariant でない 184）。objective の 539 件は scaffold-fix の数え直しで 537 件になっている（[`scaffold-fix.md`](scaffold-fix.md)）。
- 全 537 試験に capability と契約（1 行）が付いた。file の値が 60 件、試験ごとの上書きが 265 件。
- この fragment が持つ組は 9 件で、全部 `duplicate_setup`。4 件を採り、5 件を採らなかった。`equivalent_scenario` の組は 0 件（下の「config/ の扱い」を参照）。
- 試験・`crates/`・`contracts.py`・ほかの fragment は変えていない。

## capability ごとの件数
| capability | 出所 | 試験 | うち invariant |
|---|---|---|---|
| `release-selfdeploy-wiring` | B0 | 66 | 52 |
| `knowledge-curation-jobs` | extra | 45 | 40 |
| `script-runtime` | B0 | 35 | 27 |
| `delivery-merge-candidate` | B0 | 35 | 35 |
| `jev-classifier` | B0 | 33 | 14 |
| `worker-protocol-adapters` | B0 | 25 | 20 |
| `sandbox-db-guard` | B0 | 24 | 0 |
| `cluster-ssh-session` | extra | 21 | 13 |
| `routing-lane-route` | B0 | 21 | 13 |
| `accounts-quota` | B0 | 20 | 18 |
| `cos-chat-threads-runs` | B0 | 19 | 16 |
| `pre-review-sync-auto-resolve-integration-repair` | B0 | 16 | 12 |
| `scratch-build-cache-disk-guard` | B0 | 15 | 8 |
| `config-load-validation` | extra | 15 | 14 |
| `db-backup-maintenance` | extra | 14 | 0 |
| `model-catalog` | extra（api-b と同じ定義） | 13 | 11 |
| `org-projects-reports` | extra（api-b と同じ定義） | 13 | 8 |
| `notifications` | B0 | 13 | 13 |
| `config-runtime-edit` | extra | 11 | 0 |
| `routing-eval` | B0 | 11 | 0 |
| `cron-jobs` | B0 | 8 | 0 |
| `task-genres-roles` | extra（api-b と同じ定義） | 8 | 8 |
| `browser-doctor` | extra | 6 | 0 |
| `browser-launcher` | B0 | 6 | 5 |
| `budget-pricing` | B0 | 5 | 2 |
| `daemon-bootstrap-reload` | extra | 5 | 1 |
| `browser-site-policy-trusted-devices` | B0 | 4 | 1 |
| `project-repos-workspace` | extra（api-b と同じ定義） | 3 | 3 |
| `llm-proxy` | B0 | 3 | 3 |
| `repo-health-checker-path` | extra | 3 | 0 |
| `cos-inbox-triage-operations-audit` | B0 | 2 | 2 |
| `project-controller` | B0 | 2 | 0 |
| `review-runs-final-review` | B0 | 2 | 2 |
| `recursive-task-tree` | B0 | 2 | 2 |
| `routing-audit-outcome` | B0 | 2 | 2 |
| `doc-gardener` | extra | 2 | 2 |
| `local-machine-settings` | extra（api-b と同じ定義） | 2 | 0 |
| `credential-broker-resume` | B0 | 1 | 0 |
| `http-api-auth-principals` | B0 | 1 | 1 |
| `continuation-session-resume` | B0 | 1 | 1 |
| `celeris-mcp` | B0 | 1 | 1 |
| `decisions-plan-gate` | B0 | 1 | 1 |
| `execution-plan-adoption` | B0 | 1 | 1 |
| `run-claim-lease` | B0 | 1 | 1 |

## extra_capabilities
この fragment で新しく定義したもの（9 件）:

| id | 何か | owner_paths |
|---|---|---|
| `config-load-validation` | celeris.toml の読み込み・既定値・相対 path の解決・検証 | `crates/celeris/src/config/mod.rs` |
| `config-runtime-edit` | 設定の実行中の変更（欄の分類・伏せ字・comment を保つ書き戻し・再起動なしの反映） | `config/edit.rs`・`config/fields.rs`・`daemon/config_admin.rs` |
| `daemon-bootstrap-reload` | daemon の起動・reload・停止と背景の周期 task の配線 | `lib.rs`・`daemon.rs`・`daemon/bootstrap.rs`・`daemon/admin.rs`・`daemon/process_memory.rs` |
| `db-backup-maintenance` | DB の backup・世代の保持・checkpoint・VACUUM | `db_maintenance.rs` |
| `knowledge-curation-jobs` | 知識の日次抽出・整理 job・整理 run・GC | `knowledge_curation.rs`・`knowledge_maint.rs`・`knowledge_gc.rs`・`config/knowledge.rs` |
| `doc-gardener` | 案件の文書 repo の定期の文書整理 | `doc_gardener.rs` |
| `browser-doctor` | browser 経路の本番前提の点検 | `browser_doctor.rs` |
| `cluster-ssh-session` | クラスタへの ssh 接続（login session・ControlPath・cluster 設定） | `cluster_admin.rs`・`control_path.rs`・`config/cluster.rs` |
| `repo-health-checker-path` | release 同梱の repo health 検査器の基底式の解決 | `daemon/repo_health_path.rs` |

（path は `crates/celeris/src/` から。）

map-api-b の定義をそのまま写したもの（5 件）: `task-genres-roles`・`local-machine-settings`・`model-catalog`・`org-projects-reports`・`project-repos-workspace`。
map-api-b の branch はこの作業ツリーに入っていないので、`--check-fragment` が手元の `mapping/*.json` だけで通るよう同じ id・同じ中身で書いた。`--build` は id で重ねるので、中身が同じなら衝突しない。

## 組の判断（9 件。採る 4・採らない 5）
組の id（`dup-` で始まる）は mapping の `groups` にある。ここでは helper の名前で挙げる。

| helper | 判断 | 理由・案 | mutation_target |
|---|---|---|---|
| `hook`（classify_tick/e2e.rs・tests/credential_wait_notify.rs） | 採る（merge） | Discord webhook を受ける同じ 20 行の偽 server。両方が `#[path]` で読む `tests/support/http_async.rs` へ移す | `crates/celeris/src/notify.rs` |
| `load`（config/jev.rs・routing_mode.rs・script.rs） | 採る（merge） | db 行を前に置いて `Config::load` を呼ぶ同じ 5 行。config の cfg(test) の共通 helper 1 つにする | `crates/celeris/src/config/mod.rs` |
| `git`（knowledge_curation/tests.rs・tests/daily_curation.rs） | 採る（merge） | 試験の identity で git を走らせる同じ 15 行。`tests/support/` に置いて `#[path]` で読む | `crates/celeris/src/knowledge_curation.rs` |
| `write_executable`（daemon/adapters.rs・model_discovery/tests.rs） | 採る（merge） | ETXTBSY を避けて実行 file を書く同じ helper。同じ lib target なので cfg(test) の共通 helper 1 つにする | `crates/celeris/src/model_discovery.rs` |
| `input`（classify_stage/tests.rs・jev_key.rs） | 採らない | 7 行の `ClassifyInput` の literal で、まとめても build・読みやすさは変わらない | — |
| `at`（routing_guardrail_tests.rs ほか 2） | 採らない | 固定時刻を返す 1 行 | — |
| `now`（local_settings_wire.rs ほか 2） | 採らない | `now_utc()` を返す 1 行で、3 つの試験 target をまたいでまとめる益が無い | — |
| `lane_input`（script_ops_measure_tests.rs・tests/script_generation_daemon.rs） | 採らない | 1 本ずつが使う 9 欄の JSON literal | — |
| `org_node`（reports/tests.rs・task-api tests/approvals.rs） | 採らない | 2 crate に跨り、task-core に試験用の公開 constructor を足すことになる（汎用規約 3）。celeris 側の users も 0 件 | — |

採った組の `keep_tests` は雛形の `users` から選んだ（invariant の試験も残す側には入れてよい。members は helper の id で、試験ではない）。

## config/ の扱い
- `config/tests.rs` の 109 件と `config/cos_tests.rs` の 11 件は architecture-map の関連試験に挙がり、全件 invariant。走査が同値 scenario の組から外しているので、`equivalent_scenario` の組は無い。objective の「同じ欄・同じ入力の組に限って採る」は当てる組が無かった。
- 消した config 欄が読み込みエラーになる試験（`legacy_providers_rejected_with_new_config_guidance`・`model_routing_section_rejected_with_route_defaults_guidance`・`planner_adapter_is_rejected_with_candidate_guidance`・`proxy_tier_alias_rejected_in_celeris_config`・`removed_scratch_cache_sections_are_load_errors`・`selfdeploy_delivery_projects_is_removed_and_rejected`・`legacy_token_caps_are_rejected_with_the_field_name`・`config/project_controller.rs` の `legacy_token_cap_is_rejected_on_load`・`config/script.rs` の `config_active_bundle_sha256_is_rejected`・`notify/tests.rs` の `notify_rejects_retired_link_config`）は、「消した欄は読み込みエラーにする」という今の規則（`agent-docs/adr/2026-10-11-breaking-changes-by-default.md`）の契約として書いた。old_impl ではない。
- `config/tests.rs` は file の既定を `config-load-validation` にし、109 件の全部に試験ごとの契約を書いた。そのうち 95 件は capability も節ごとに上書きした（adapter・account・route・browser・budget・cluster・genre/role・selfdeploy・scratch・知識など）。

## instance/・releases/ と不変条件
- `instance/tests.rs` の 28 件は全件 invariant。出所は B0 `release-selfdeploy-wiring` の #0（lease_exclusion: 生きた active は 1 つだけ）と #2（fail_closed: 昇格していない release は本番を引き継がない。repo の不変条件 `unpromoted-release-no-takeover` と同じ内容）と architecture-map L150。promotion evidence を読む 10 件には `unpromoted-release-no-takeover` の契約を、残りには handoff の契約を書いた。
- `releases/tests.rs` の 21 件は全件 invariant（B0 #1 human_authority と architecture-map L150）。`releases/audit.rs` の 2 件は invariant でない。
- repo の不変条件 `lease-single-holder`（ADR 2026-10-03-ownerless-running-runs）の evidence はこの fragment には無い。instance の「active は 1 つ」は daemon の instance の排他で、run の lease ではない。
- どれも組の members に入っていない（組は duplicate_setup だけで、members は helper）。

## 申告（走査は直していない）
- `daemon/bootstrap_worker_db_guard_tests.rs` の 24 件（`sandbox-db-guard`、1 件は `browser-launcher`）は、worker run の中の daemon が本番 DB を開かないこと（repo の不変条件 `worker-db-read-only`、ADR-0095）を確かめる試験だが、B0 の `sandbox-db-guard` の tests・evidence にも architecture-map の関連試験にも無いため invariant=false になっている。今は組に入っていないので害は無い。後で整理の候補を作る段では、architecture-map の db_guard の行の関連試験にこの file を足すことを提案する（この unit の write set の外なので変えていない）。
- `db_maintenance/tests.rs::incremental_backup_progress_restarts_after_other_connection_writes` は、doc の 1 行目が「Measure the old 1-page stepping mechanism」で、Celeris の code を呼ばずに rusqlite の online backup を 1 page ずつ進める旧い方式を計測している。old_impl の候補になりうるので、unit `old-impl` の判断の材料として挙げる（この unit は `candidates/old-impl.json` を書かない）。

## 証拠（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --check-fragment agent-docs/progress/2026-11-10-health-v2-r5/mapping/celeris-src.json` → exit 0（`ok`）。
- 範囲の確認は result.json と報告に書く。

## commit
- 「R5 map-celeris-src: celeris src の試験 module 537 件に契約を付け、9 組を判断する」
