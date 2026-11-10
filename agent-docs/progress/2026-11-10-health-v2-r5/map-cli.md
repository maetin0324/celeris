---
title: 健全性v2 R5 map-cli — celeris tests と celerisctl の試験関数に契約を付け、組を判断する
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 map-cli — celeris tests と celerisctl の試験関数に契約を付け、組を判断する

完了日: 2026-11-10

WorkUnit `map-cli`。fragment `cli`（scope `^crates/(celeris/tests/|celerisctl/)`）の commit 済みの雛形 [`mapping/cli.json`](mapping/cli.json) を、[`fill-guide.md`](fill-guide.md) の手順どおりに埋めた。`--scaffold` は走らせていない。`scan` 欄・試験 id・組の members・users は変えていない。試験（crates/ の下）・`contracts.py`・ほかの fragment の `mapping/*.json` は変えていない。基準は `$CELERIS_WU_BASE`。

## 結論
- scope の 104 file・472 件の試験（celeris の tests/ 267 件、celerisctl 205 件）の全部に `capability` と契約（1 行、最長 120 字）を付けた。`--check-fragment` は exit 0。
- 472 件のうち 446 件は試験ごとの上書き（`fns`）で個別の契約を書いた。上書きの無い 26 件は試験が 1 件だけの file で、file の契約がその試験の契約。
- capability は 40 種（B0 の 24 種、extra の 16 種）。
- 不変条件の試験（`invariant=true`）は 165 件。どの組の members にも入っていない（`equivalent_scenario` の 5 組は走査の段階で invariant を除いてあり、`duplicate_setup` の members は helper）。
- cli が持つ組は 30 件（`duplicate_setup` 25 件、`equivalent_scenario` 5 件）。採用 16 件（13 件・3 件）、不採用 14 件（12 件・2 件）。

## 先に確かめた試験
- `worker_db_guard_refuse.rs` の 2 件は invariant（B0 `sandbox-db-guard` の invariant #1 の evidence と architecture-map L102）。
- `unpromoted_release.rs` の 10 件は invariant（architecture-map L150 の self-deploy の行）。
- `no_migrate.rs` の 4 件は invariant（architecture-map L164 の celerisctl の行）。
- `legacy_event_startup.rs` は invariant ではないが、旧い routing 実装が DB に残した `routing_decided` の event を起動と tick が読む replay の契約。契約にそう書き、どの組にも入っていない（old_impl の候補にもしない）。

## capability ごとの試験の数

| capability | 試験 | 出所 |
|---|---|---|
| `script-runtime` | 90 | B0 |
| `campaign-eval` | 61 | B0 |
| `notifications` | 49 | B0 |
| `release-selfdeploy-wiring` | 34 | B0 |
| `project-controller` | 19 | B0 |
| `task-lifecycle` | 19 | B0 |
| `worker-protocol-adapters` | 16 | B0 |
| `cos-inbox-triage-operations-audit` | 15 | B0 |
| `pre-review-sync-auto-resolve-integration-repair` | 15 | B0 |
| `browser-site-policy-trusted-devices` | 14 | B0 |
| `celerisctl-cli` | 13 | B0 |
| `knowledge-curation-gc` | 12 | extra |
| `test-fixtures-shared` | 12 | extra |
| `cron-jobs` | 10 | B0 |
| `scratch-build-cache-disk-guard` | 10 | B0 |
| `task-read-views` | 10 | extra |
| `routing-lane-route` | 9 | B0 |
| `knowledge-base` | 6 | extra |
| `execution-plan-adoption` | 5 | B0 |
| `org-projects-reports` | 5 | extra |
| `task-genres-roles` | 5 | extra |
| `terminal-sessions` | 5 | extra |
| `celeris-daemon-wiring` | 4 | extra |
| `routing-eval` | 4 | B0 |
| `skills-delivery` | 4 | extra |
| `db-maintenance` | 3 | extra |
| `local-machine-settings` | 3 | extra |
| `credential-broker-resume` | 2 | B0 |
| `event-replay` | 2 | extra |
| `model-catalog` | 2 | extra |
| `review-runs-final-review` | 2 | B0 |
| `routing-audit-outcome` | 2 | B0 |
| `sandbox-db-guard` | 2 | B0 |
| `worker-followup-tasks` | 2 | extra |
| `browser-egress-runtime` | 1 | B0 |
| `celeris-mcp` | 1 | B0 |
| `cluster-admin-connect` | 1 | extra |
| `decisions-plan-gate` | 1 | B0 |
| `knowledge-pages-skills` | 1 | extra |
| `recursive-task-tree` | 1 | B0 |

## extra_capabilities
cli で新しく足した 7 件:
- `celeris-daemon-wiring` — celeris daemon の起動順・配線と tick ループ（bin_smoke・legacy_event_startup・tick_prod_copy_steps・tick_scale_baseline）。
- `test-fixtures-shared` — 試験の土台（daemon_ready_wait・fake_http・celerisctl の cli_resolution）。B0 は製品の capability だけを持つ。
- `knowledge-curation-gc` — 知識の日次整理 job と curation plan の検証（daily_curation・curation_tests）。
- `skills-delivery` — skill の worker 作業場所への配送（ui_ux_skills_delivery）。
- `db-maintenance` — DB の backup と integrity check（celerisctl db）。
- `event-replay` — events からの表の再構築（celerisctl replay）。
- `worker-followup-tasks` — worker の run が宣言した後続 task（celerisctl の followups）。

ほかの map leaf が足した 9 件は、同じ id・同じ定義で写した（`--check-fragment` は作業ツリーの `mapping/*.json` しか読まず、この branch では兄弟の fragment が空の雛形のため）: map-api-a の `cluster-admin-connect`、map-api-b の `knowledge-base`・`task-genres-roles`・`org-projects-reports`・`local-machine-settings`・`task-read-views`・`model-catalog`、map-api-c の `knowledge-pages-skills`・`terminal-sessions`。`contracts.py` の `all_extra_caps` は同じ id を後勝ちで重ねるので、定義が同じなら組み立ての順によらず同じ結果になる。

## 組の判断
組は helper の名前と file で書く（組の id は [`mapping/cli.json`](mapping/cli.json) の `groups` にある）。

採用（16 件）:

| 組（kind・file） | action | 案 | mutation の対象 |
|---|---|---|---|
| `dechunk`（dup、campaign_acceptance_e2e・campaign_lock_e2e） | merge | campaign の試験の共通 module に 1 つ置く | `crates/campaign/src/celeris_exec.rs` |
| `git`（dup、stranded_default_branch・stranded_integration） | merge | `tests/support/mod.rs` に移す | `crates/celeris/src/daemon/stranded_sweep.rs` |
| `wait_until`（dup、instance_handoff・unpromoted_release） | merge | `tests/support/mod.rs` に移す | `crates/celeris/src/instance.rs` |
| `ev_started`・`started`（dup、campaign の 3 e2e） | merge | ev_finished と一緒に campaign の共通 module へ | `crates/task-dispatch/src/dispatcher/decision_log_wire.rs` |
| `ev_finished`（dup、campaign の 2 e2e） | merge | 同上 | `crates/task-dispatch/src/dispatcher/decision_log_wire.rs` |
| `ctl`（dup、mcp_client_scopes・no_migrate） | merge | env を外した Command を返す 1 関数を celerisctl の `tests/common/mod.rs` に置く | `crates/celerisctl/src/main.rs` |
| `ctl`（dup、campaign_export・eval_requests・routing_cli） | merge | 上と同じ関数を使う | `crates/celerisctl/src/main.rs` |
| `write_executable`（dup、cap_probe_real_daemon・ops_r1_measure） | merge | `tests/support/mod.rs` に移す | `crates/campaign/src/cap_probe.rs` |
| `run_daemon`（dup、script_compare_cos_digest・script_compare_lane_rules） | merge | 既に読む `support/script_compare.rs` に移す | `crates/celeris/src/daemon/script_layer.rs` |
| `is_timeout`（dup、script_compare_cos_digest・script_generation_e2e・support/script_compare） | merge | 写し 2 つを消して support の関数を使う | `crates/celeris/src/daemon/script_layer.rs` |
| `status_kb`（dup、long_run_soak・project_controller_replay） | merge | pct・threads と一緒に計測用の共通 module へ | `crates/task-dispatch/src/dispatcher/project_decider_ts.rs` |
| `pct`（dup、同上） | merge | 同上 | `crates/task-dispatch/src/dispatcher/project_decider_ts.rs` |
| `hook`（dup、cos_chat_triage_scenarios・root_done_summary） | merge | 偽 webhook の Hook を `tests/support/mod.rs` に移す | `crates/celeris/src/notify.rs` |
| `run_show` の在る task（eq、query_tests） | merge | json の false・true の表で回す 1 本に | `crates/celerisctl/src/commands/query.rs` |
| `run_show` の無い task（eq、query_tests） | merge | 同上 | `crates/celerisctl/src/commands/query.rs` |
| question と provider_failure の exit code（eq、worker_run） | merge | script の出力・exit code・stdout の断片の表で回す 1 本に | `crates/celerisctl/src/commands/worker.rs` |

不採用（14 件）:
- 短い 1〜4 行の helper で、まとめても build も読みやすさも変わらないもの 11 件: `campaign_root`・`repo`/`repo_root`・`general_thread`・`text`・`cand`・`sha`・`env_usize`・`config`（NotifyConfig）・`count`・`now`・`threads`（threads は status_kb・pct を束ねるときに一緒に移せば足りる）。
- 意味が違うもの 1 件: `ts`（actor_trace/support と ops_r1/trace_fixture）。本体は同じだが、呼ぶ `at` の基準時刻が違う（2026-10-28 と 2026-10-29）。
- 同値 scenario の 2 件: cos_ops_tests の reason の空白と header の注入（守る入力検証の枝が違う）、variant_propagation の spec-on と spec-off（既に共通の `normal_path` を引数で呼ぶ 2 本で、別の腕。束ねても本体は減らない）。

各組の `keep_tests` は users から選び、users が空の組は member の file の試験から選んだ。採用した組はすべて helper の置き換えか表駆動への束ねで、守る試験の行（入力）は消さない。

## daemon を起こす helper の crate をまたぐ写し
- daemon_port（`tests/daemon_port/mod.rs`）・support（`tests/support/mod.rs`）・actor_trace/support・celerisctl の `tests/common/mod.rs` の関数は、本体の hash が同じ写しをほかの crate に持たない（走査の組に入っていない）。
- crate をまたぐ組で cli の scope の試験を含むものは 5 件あり、どれも先頭の member が celeris src なので map-celeris-src が判断する: `hook`（classify_tick/e2e と credential_wait_notify）、`at`（routing_guardrail_tests と cos_chat_triage の 2 file）、`now`（local_settings_wire・script_compare_report・task-api の chat_stream）、`git`（knowledge_curation/tests と daily_curation）、`lane_input`（script_ops_measure_tests と script_generation_daemon）。
- celerisctl の `ctl` は本体の違う 2 種（`--db` と `CELERIS_FOLLOWUPS_FILE` の有無）が別の組になった。どちらも採用し、同じ 1 関数にまとめる案にした。

## 確かめた結果（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --check-fragment agent-docs/progress/2026-11-10-health-v2-r5/mapping/cli.json` → exit 0（`ok`）
- 雛形との突き合わせ（python で HEAD の雛形と比べた）: `scan`・`scope`・各 file の `tests`・`b0_hint`・各組の `members`・`users`・`names` は同じ。
- `git diff --name-only "$CELERIS_WU_BASE"` → `mapping/cli.json` とこの進捗の 2 file だけ（crates/ と試験の変更なし）
- `git diff --name-only "$CELERIS_WU_BASE" | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0
- `bash scripts/dev/test-affected.sh --list` → 対象の crate なし（crates/ を変えていない）。clippy・fmt は crates/ を変えていないので走らせていない。

## 変更
- `wu/map-cli: mapping/cli.json に 472 件の契約と 30 組の判断を書く` — 上のとおり。

## 未解決
- 契約の 1 行は試験名・doc comment・本体の assert から書いた。本体の assert まで読んで直したのは browser の challenge の形・worker_run の browser の能力・browser 台帳の status と exit code・CoS 一次対応の切替の 4 箇所で、ほかは試験名と doc comment に拠る。
- 採用した組のうち 3 件は先頭の member の file の外を含む（`wait_until` の unpromoted_release、`hook` の root_done_summary、`is_timeout` の script_generation_e2e）。実際の整理（dedupe 段）ではこれらの file も変える。

## 提案
- `is_timeout` は共通の `support/script_compare.rs` に既にある関数の写しなので、dedupe 段で最初に消すと mutation の確認が少なくて済む。
- celerisctl の `ctl` の 2 組は、外す env の一覧を `tests/common/mod.rs` の 1 か所に置くと、worker の run の env（`CELERIS_RUN_DB`・`CELERIS_FOLLOWUPS_FILE`）を足したときの漏れが起きなくなる。
