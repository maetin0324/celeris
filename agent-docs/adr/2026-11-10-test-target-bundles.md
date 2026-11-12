---
status: accepted
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
---
# ADR 2026-11-10: test target の束ね方・削除の証拠・test-affected の作り直しの直し方（健全性 v2 R5）

- 日付: 2026-11-10
- 状態: 採用（設計）。この ADR ではコードを変えない。実装は健全性 v2 R5（task 01M6TYVJT53598CPKMGSQGA7XM）の段 bundle（`bundle-api-a`・`bundle-api-b`・`bundle-celeris`・`bundle-ctl`・`affected-features`）、段 dedupe（`dedupe-api`・`dedupe-celeris`）、段 final（`sync-main`・`adjust-api`・`adjust-worker`）、段 measure（`measure-build`・`measure-run`）。決定と実装の食い違いは末尾の付記「実装との突き合わせ」に書く
- 関係: [ADR 2026-11-07 健全性 v2 の基準](2026-11-07-health-v2-baseline.md)（R5 の scope・write set・停止条件）、[ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md)（`size.responsibility-growth` と `dup.test-fixture`）、[ADR-0041 §8](0041-self-improvement-loop-hardening.md)（nextest と test-group）、[ADR 2026-10-20 全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)、[ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)、[ADR 2026-10-23 test-affected の web 型検査](2026-10-23-test-affected-web-typecheck.md)
- 根拠の記録（段 survey の成果。すべて `agent-docs/progress/2026-11-10-health-v2-r5/`）: [inventory.md](../progress/2026-11-10-health-v2-r5/inventory.md)・`inventory.json`（test target 352 件の分類）、[baseline.md](../progress/2026-11-10-health-v2-r5/baseline.md)・`baseline.json`・`features.json`（base の計測と作り直しの原因）、[assemble.md](../progress/2026-11-10-health-v2-r5/assemble.md)・`contracts.json`（契約→試験の対応と候補）、[guard.md](../progress/2026-11-10-health-v2-r5/guard.md)・`cases.py`・`mutate.sh`（検査と mutation の台本）

## 背景

### B0 の数値
- workspace の Rust の test target（`cargo metadata` の kind=test）は 352 個で、全部が `crates/<crate>/tests/` 直下の file。入れ子の target（`tests/<dir>/main.rs`）は 0 個（B0 `test-targets.md`）。
- test binary（`cargo nextest list --list-type binaries-only`）は task-api 114・celeris 57・task-worker 25・task-core 3・task-dispatch 2。task-api は test target 113 に lib の unittest binary 1 を足した数。
- B0 の edit→verify（`src/lib.rs` に 1 行足して `test-affected.sh --list` と対象 crate の `nextest run --no-run`）は task-api 105〜113 秒、celeris 33〜85 秒、task-core 139〜144 秒。B0 は「前回と違う crate の組で test-affected.sh が走ると workspace の crate が作り直される」交絡を指摘した。

### baseline.json（R5 の base。`git merge-base HEAD main`、同じ run で 3 crate を順に計測）
| crate | build --no-run | test binary | edit→verify | 再 build で link した binary | run1 / run2 | 試験 | 失敗 |
|---|---|---|---|---|---|---|---|
| task-api | 133.25 秒 | 114 | 155.78 秒 | 328 / 333 | 59.56 / 60.64 秒 | 876（skip 2） | 0 / 0 |
| celeris | 124.64 秒 | 57 | 37.11 秒 | 76 / 76 | 184.30 / 69.98 秒 | 802（skip 2） | 0 / 0 |
| celerisctl | 55.60 秒 | 19 | 5.53 秒 | 1 / 19 | 4.78 / 4.45 秒 | 205 | 0 / 0 |

- task-api の 1 行の編集は対象 7 crate の test binary 333 個のうち 328 個を link し直した。link の回数を減らすには binary の数を減らすしかない。
- 2 回の実行で flaky は 0 件。celeris の run1 と run2 の差（184 秒と 70 秒）は負荷か page cache で、原因は未確認（段 measure で測り直す）。
- 作り直しの原因（baseline.md）: Cargo（resolver 2）は `-p` で選んだ crate の組の中だけで features を統合する。workspace の crate は共有 crate に違う features を足す（task-api の dev-dependencies が task-worker に `attack-test-hooks`・`h3-e2e-insecure-cert`・`live-cross-test-support`・`same-uid-harness`、celeris-credentiald に `same-uid-harness`、llm-proxy の通常の依存が time に `macros` など）。test-affected.sh は変更ごとに違う組を `-p` で渡すので、組ごとに別の版（別の metadata hash）が build され、source が変わるたびに組ごとに作り直しが要る。`-p celerisctl` だけの組は tokio の features が変わり、その上の 26 crate が別の版になる。

### 契約→試験の対応（assemble.md）
- task-api 855・celeris 804・celerisctl 205 の試験関数 1864 件に capability と契約が付いた。不変条件の試験（`invariant=true`）は 874 件。
- 候補は `duplicate_setup` 49 件（helper の写し。試験の数は減らない）、`equivalent_scenario` 4 件（束ねると試験が 4 件減る）、`old_impl` 3 件（delete 2 件で 3 件減る）。不変条件の試験は候補に 1 件も入っていない。

## 決定

### D1. 束ねの規則
1. 束ねる単位は「同じ crate・同じ leaf・同じ capability group（`capability_key`）・同じ fixture（`fixture_key`）」の test target。capability group は B0 capability-map の `cap-*`、fixture は target と支援 module が読む `tests/common`・`tests/support`・外の fixture file の組（`inventory.json` の欄）。capability の無い target（`group:none`）は fixture だけで束ね、束の名前を `unmapped_*` にする。意味の違う fixture（別の daemon を起動する・別の userns gate を読む）は束ねない。
2. 1 束は `crates/<crate>/tests/<bundle>/main.rs` を root にし、元の target を `tests/<bundle>/<target>.rs` に置いて `mod <target>;` で宣言する。Cargo はこの dir を 1 つの test target（binary id `<crate>::<bundle>`）として認識する。試験の名前は `<target>::<fn>` になり、関数名は変わらない。
3. 束の名前は英小文字と `_` だけ（数字を含めない。B0 R5 の write set `tests/[a-z_/]+\.rs` の中に収めるため）で、crate の中で既存の target 名・`tests/` の dir 名と重ならない。同じ鍵の束が上限で割れるときは `_a`・`_b`・`_c` を付ける。
4. nextest（release gate の `test-parallel.sh` と `test-affected.sh`）は試験ごとに別 process で走らせるので、束ねても試験の隔離は変わらない。`cargo test --workspace`（開発者が使ってよい。`test-parallel.sh` の前提）は 1 binary の試験を 1 process の thread で並列に走らせるので、D3 の汚染を持つ target は束ねない。

### D2. 1 束の上限
- 1 束は test target 8 件まで（`inventory.json` の `max_per_bundle`。同じ鍵の組は件数を均して割る）。
- 束の本体の行の合計は `size.responsibility-growth` の `module_lines`（8000）、1 file は `file_lines`（2000）を超えない。この size 規則の module の単位は `<pkg>/src/<module>` で、`tests/` の file は `external_test` に分類されて数えない（`scripts/dev/repo-health/rules/size_growth.py`、`source-size-report.toml`）ので、束ねで `module_lines` の finding は出ない。それでも上限を置くのは、規則が `tests/` に広がっても新しい違反を出さないため。計画の束で最大は task-api `unmapped_suite_a` の 8 file 4627 行（注釈・空行込み。付記 reclass で組み直した後）、最大の file は `cos_delivery_suite_c` の `operations.rs` の 1133 行で、どちらも上限の内。
- `dup.test-fixture`（内容がほぼ同じ試験 file の候補）は束ねでは変わらない（file の中身を変えないため）。

### D3. process global・env・CWD・時刻の汚染
- 次の操作を target かその支援 module（`mod`・`#[path]` で取り込む全 file）の本文に持つ target は keep（自分の binary のまま）: `env::set_var`・`remove_var`（env）、`set_current_dir`（CWD）、`process::exit`、`static mut`・`OnceLock`・`Lazy` などの可変 static（mutable_static。注入した時計を static に持つ形を含む）、signal の送受（`libc::kill`・`signal_hook`・`tokio::signal` など）、`umask`、`current_exe`（自分の binary を子 process として起動する）、固定 port の bind/connect、固定 path（`/tmp`・`/run` などの決め打ち）。検出の正規表現は `classify.py` の `PROCESS_GLOBAL`。
- 時刻を読むだけ（`Instant::now`・`SystemTime::now`・`OffsetDateTime::now_utc`）は汚染でない。`tokio::time::pause` は runtime ごとなので汚染でない。時刻を全 process で差し替える操作（static の時計・`TZ` の `set_var`・faketime）は mutable_static か env_mutation として keep になる。
- 静的な走査の限界（支援 crate の関数の中の操作、実行時に組み立てる port・path）は、束ねた後に `cargo nextest run -p <crate> -E 'binary_id(<crate>::<bundle>)'` と `cargo test -p <crate> --test <bundle>`（1 process の実行）の両方を通して確かめる。`cargo test` だけで落ちる試験があれば、その target を束から外して keep に戻し、`inventory.json` の `leaf_reasons` に理由を足す（束の分解は失敗でなく、D3 の検出）。

### D4. keep・r6 の条件（`inventory.json` の `rules.leaf_order`。先に当たったものが leaf）
1. keep: D3 の汚染、`scripts/` の `--test`・`-E` か `.config/nextest.toml` の `binary_id` で名前を呼ばれる suite（named_suite。`task-worker::ssh_localhost`・`e2e::cluster_scenarios` の test-group を含む）、file 名の数字（write set の外）、別の target の file を `#[path]` で取り込む・取り込まれる（shares_source_with。`local_machine_settings` と `local_settings_admin`、`llm_routes_admin` と `llm_accounts_admin`・`llm_sources`）、R5 に束ねる leaf の無い crate（celeris-campaign 111 件を含む）、adjust leaf の無い crate での R2〜R4 との競合、束ねる相手の無い 1 件だけの組（singleton_bundle_group）。
2. r6: `docs/architecture-map.md` の表の行が path を挙げる target（keep の理由の無い 55 件。task-api 36・celeris 12・celerisctl 7）。行の path は `scripts/dev/check-architecture-map.py` の `collect_backtick_paths` が集める形で数える（brace の `a/{b,c}.rs` と、path の直後の `（…）` の中の相対 path を含む。付記 reclass）。地図は R5 の write set の外なので、R5 では動かさず R6 で地図と一緒に動かす。
3. adjust-api・adjust-worker: R2〜R4 の branch が変える file・ADR が名指す target・R2 が変える interface を名指す target。task-worker は R2 の write set と interface が全 target を覆うので、keep でない 9 件を段 final の adjust-worker に置く。
4. 残りが段 bundle の 4 leaf。keep と r6 の target は動かさないので、R2〜R4 の変更と衝突しない。

### D5. 削除の規則（段 dedupe と段 final）
1. 消してよい試験は、`contracts.json` の候補（`duplicate_setup`・`equivalent_scenario`・`old_impl`）の members だけ。候補の無い試験は消さない。
2. 消す前に 3 つを示す: (a) 契約の case が残る（候補の `keep_tests` の試験が作業ツリーにあり、同じ契約の 1 行を持つ。`equivalent_scenario` は表駆動の 1 本がどの入力も行に持つ）、(b) `mutate.sh <patch> <crate> <filter>` が `KILLED` を返す（候補の `mutation_target` の module に当てた patch を、残す試験が落とす。build の失敗・試験 0 件・filter の誤りは KILLED でない）、(c) 不変条件の試験でない（`invariant=true`、B0 capability-map の `invariants[].evidence_path`、architecture-map の関連試験の列にある試験は消さない。`cases.py` は removals に載せても通さない）。
3. 記録は `agent-docs/progress/2026-11-10-health-v2-r5/removals/<crate dir>.json`（`{"crate", "removals": [{"name", "count"?, "cases"?, "reason", "replacement": [...], "mutation": {"patch", "filter", "result": "killed"}}]}`）。patch は同じ dir の `mutations/<crate>-<name>.patch` に置き、`mutate.sh` の出力を進捗に写す。`python3 cases.py --crate <crate> --base "$CELERIS_WU_BASE"` が exit 0 でなければ消したことにならない（統合では base を `git merge-base HEAD main`）。
4. 試験の数が減る変更（`equivalent_scenario` 4 件、`old_impl` の delete）は B0 slices.json の停止条件に当たるので、段 dedupe の人の決定 `test-removal` の答えが出るまで着手しない。`duplicate_setup` は helper を共通 module へ寄せるだけで試験の数は減らず、決定を待たない。
5. 旧実装の固定でも、DB に残る旧い shape の event を読む replay の契約の試験（`task-api/tests/legacy_routing_events.rs`・`celeris/tests/legacy_event_startup.rs`）は消さない（契約は現行）。
6. write set の外（数字を含む file 名、試験でない src の本体を変える `old_impl` 2 件、`duplicate_setup` 12 件）は R5 では消さず、R6 か別の task に回す。カバレッジ値・LoC の目標だけを理由に消さない（親 task の原則）。

### D6. test-affected の crate の組による作り直しの直し方（unit `affected-features`）
1. 原因は baseline.md のとおり features の統合の範囲。直すのは `scripts/dev/test-affected.sh`（write set の中）の cargo の呼び方だけで、対象 crate を決める規則（変更 file → crate → 逆依存閉包、workspace 共通 file で全 crate）は変えない。
2. 案 F1 を採る: `cargo nextest run "${args[@]}"` を `cargo nextest run --workspace -E '<filter>'` にし、filter は今の対象 crate の `package(<crate>)` を `|` で結ぶ。build は `--workspace` の features（release gate の `test-parallel.sh` と同じ版）で行い、走らせる試験は今と同じ crate に絞る。`--list` の出力は変えない。
3. doc test も `cargo test --doc --workspace` にする。`cargo test --doc -p <crate>` は組ごとの版を作るので、F1 の目的（版を 1 つにする）に反する。費用は全 crate の doc test の実行で、`crates/*/src` の doc comment の fence の数から概数 31 件（未確認）。`affected-features` は件数と秒を measure.sh の `edit_verify` の段に足して記録する。
4. 受け入れる費用: scratch の target に `--workspace` の版が無い初回は、対象外の crate の test binary も build・link する（B0 の 14 crate 全部の再 build は 139〜144 秒）。source が新しい版では作り直しは変更した crate と逆依存だけで、今の対象と同じ。試験の実行の範囲は変えない（対象規則を弱めない）。
5. 採らない案: F2（各 crate の Cargo.toml で features を揃える。直接依存しない crate の features まで揃える必要があり Cargo.lock が変わる、依存が増えるたびに崩れる）、F3（Cargo の `resolver.feature-unification = "workspace"`。cargo 1.98.1 は `-Zfeature-unification` が無いと無視する。`.cargo/config.toml` は `worktree-target-dir.sh` が作り write set の外）。
6. 撤去の条件: stable の cargo が `feature-unification = "workspace"` を読むようになったら、F1 の filter を `-p` に戻して環境変数 `CARGO_RESOLVER_FEATURE_UNIFICATION=workspace` に替える（責任者は test-affected.sh の変更を入れる task。この ADR に付記で書く）。
7. 効果は measure.sh の `edit_verify` と、組を変えた後の build（A → B → A の Compiling 行）で確かめる。write set の中で直せない部分（scratch の seed の版、`.cargo/config.toml`）が要れば、直さず進捗の提案に回す。

### D7. 測り方（段 measure）
- `measure.sh run base <out> task-api,celeris,celerisctl` と `measure.sh run head <out> …` を同じ run・同じ host・同じ順で続けて走らせ、`baseline.json` と同じ欄（build --no-run・Compiling・test binary・link した binary・edit→verify・run1/run2・失敗）を比べる。base は `git merge-base HEAD main` の worktree で測る。負荷（`/proc/loadavg`）を各段の前後に記録し、1 分値が 2 倍以上違う段は比較に使わない。
- `measure-run` は HEAD の試験の実行時間と flaky（2 回の実行で結果が違う試験）を測り、束ねた target を `cargo test -p <crate> --test <bundle>` で 1 process でも走らせる（D3 の確認）。
- 前後の比較で示す数: test target の数（`python3 scripts/dev/test-target-inventory.py --json`）、test binary の数、test build/link の秒、edit→verify の秒、run の秒、flaky の件数、`cargo nextest list` の試験の数（束ねで変わらない）。

## 束ねの表（`inventory.json` の bundle と leaf。base は `inventory.json` の `base_sha`）

### 段 bundle（77 target → 15 束。task-api 56 → 9、celeris 15 → 4、celerisctl 6 → 2）
| crate | leaf | 束（`tests/<bundle>/`） | 件 | capability group | fixture | target |
|---|---|---|---|---|---|---|
| task-api | bundle-api-a | `cos_delivery_suite_a` | 8 | cap-cos-delivery | common | chat_attachments・cos_auth・cos_chat_attach_handoff_kb・cos_chat_triage_scenarios・cos_job_wait_extend・cos_operations_review_report・cos_ops_mutations・cos_ops_projects_cron_changes |
| task-api | bundle-api-a | `cos_delivery_suite_b` | 7 | cap-cos-delivery | common | cos_ops_projects_cron_docs・cos_ops_registry・cos_plan_conforms_guards・cos_review_rework・cos_routine_answers・cos_triage_override・general_cos_consult |
| task-api | bundle-api-a | `routing_budget_suite` | 3 | cap-routing-budget | common | project_brief・project_wake_human・project_wake_human_chat |
| task-api | bundle-api-b | `browser_eval_surface_suite` | 6 | cap-browser-eval-surface | common | api_principals・browser_restore_deliver・browser_restore_live_session・campaign_ident・campaign_variant_atomic・mcp_admin |
| task-api | bundle-api-b | `cos_delivery_suite_c` | 8 | cap-cos-delivery | common | daemon_clusters_config・integration_api・integration_principals・notify・operations・single_human_window・task_list_with・task_wait_api |
| task-api | bundle-api-b | `routing_budget_suite_a` | 7 | cap-routing-budget | common | cron_jobs・legacy_routing_events・routing・routing_source_state・routing_stage・saved_credentials・script_exhaustive |
| task-api | bundle-api-b | `run_worker_suite` | 2 | cap-run-worker | common | repository_required・secrets_admin |
| task-api | bundle-api-b | `unmapped_suite_a` | 8 | none | common | approvals・cluster_settings_admin・concurrency・console・console_instruct・conversation・docs・files |
| task-api | bundle-api-b | `unmapped_suite_b` | 7 | none | common | genres・memory・roles・skills・standby・terminals・unblocks |
| celeris | bundle-celeris | `browser_eval_surface_suite` | 3 | cap-browser-eval-surface | none | browser_startup_reap・fake_http・workspace_roots |
| celeris | bundle-celeris | `cos_delivery_suite` | 7 | cap-cos-delivery | none | cos_chat_stagnation_notify・cos_chat_triage_notify・cos_chat_triage_scenarios・deployment_audit・releases_api・stranded_default_branch・stranded_integration |
| celeris | bundle-celeris | `routing_budget_suite` | 3 | cap-routing-budget | support | script_compare_execution_gate・script_compare_report・tick_scale_baseline |
| celeris | bundle-celeris | `unmapped_suite` | 2 | none | none | legacy_event_startup・variant_propagation |
| celerisctl | bundle-ctl | `browser_eval_surface_suite` | 4 | cap-browser-eval-surface | common | browser_doctor・browser_ledger_release・campaign_connected・mcp_client_scopes |
| celerisctl | bundle-ctl | `unmapped_suite` | 2 | none | common | pipe・skills_import |

- `routing_budget_suite_a` の `legacy_routing_events` は旧い shape の event の replay の契約の試験で、束ねるが消さない（D5-5）。
- `group:none` の束（`unmapped_*`）は fixture だけで束ねた。`contracts.json` の extra capability（`approvals-standing-rules`・`console-feed`・`org-projects-reports` など）が B0 capability-map に入ったら、段 final の sync-main が `classify.py --write` をやり直して鍵を付け直す。それまで名前は変えない。

### 段 final（決定 `r-landed` の後。R2〜R4 が main に入ってから `classify.py --write` をやり直し、表を直す）
| crate | leaf | 束（案） | 件 | target |
|---|---|---|---|---|
| task-api | adjust-api | `cos_delivery_suite`（R3 の競合が解ければ `cos_delivery_suite_*` へ） | 4 | cos_operations_domains・cos_ops_admin_config・cos_ops_projects_cron・project_auto_integrate |
| task-worker | adjust-worker | `browser_eval_surface_suite` | 5 | browser_control_gate_wire・browser_live_cross・browser_loopback_prod_build・browser_prod_admission・browser_shared_cdp_events |
| task-worker | adjust-worker | `unmapped_suite` | 2 | reap_finished_children・ssh_cluster_manual |
| task-worker | adjust-worker | 1 件だけの組（束ねない） | 2 | browser_action_recovery（fixture `browser_action_recovery.py`）・browser_restore_deliver（fixture `userns_gate`） |

### 動かさない target（task-api・celeris・celerisctl）
| crate | keep | 内訳 | r6 |
|---|---|---|---|
| task-api | 17 | 名前で呼ばれる suite 5・`#[path]` の共有 5・数字 4・process global 3・1 件だけの組 2（重複あり） | 36 |
| celeris | 28 | 数字 16・process global 10・名前で呼ばれる suite 4・1 件だけの組 2・adjust leaf の無い競合 1（重複あり） | 12 |
| celerisctl | 5 | 1 件だけの組 3・数字 1・process global 1 | 7 |

## 撤去
- 束ねた元の target: `crates/<crate>/tests/<target>.rs` は `git mv` で `tests/<bundle>/<target>.rs` へ移すので、元の path に file は残らない。Cargo.toml に `[[test]]` の明示は無いので Cargo.toml は変えない。
- 写した setup: 各 target の先頭の `mod common;`（celeris は `mod support;`、`#[path = "…/tests/support/http_async.rs"] mod fake_http;` など）は束の `main.rs` の 1 回の宣言に置き換え、target の file からは消す。同じ支援 file を 2 つの target が別の名前で取り込んでいたら、束では 1 つの名前で宣言し、もう一方は `use crate::<name> as <旧名>;` で合わせる。
- 消した試験（D5）の本体と、それだけが使っていた helper は同じ変更で消す。消した helper が `dup.test-fixture` の候補に載っていたら、その finding は消える。
- `scripts/dev/test-affected.sh` の `-p` の組での build は F1 で消え、組ごとの版は scratch の target に残らなくなる（既にある版は次の prune で消える。手で消さない）。
- 支援 module の `#[allow(dead_code)]` は、束ねて使われるようになった helper では要らなくなるが、R5 では触らない（他の keep の target が同じ module を読むため）。

## 試験
| 検査 | 守ること | 置き場所 |
|---|---|---|
| `python3 classify.py --check inventory.json` | base の test target が過不足なく 1 回ずつ載り、leaf と束の規則（D1・D2・D4）を満たす | 段 design・bundle・final の check |
| `python3 classify.py --verify-leaf <leaf> inventory.json` | その leaf の target が `tests/<bundle>/<target>.rs` にあり元の path に無く、`main.rs` が `mod <target>;` を宣言し、keep と r6 の target が元の path のまま | 各 bundle・adjust leaf の check |
| `python3 cases.py --crate <crate> --base "$CELERIS_WU_BASE"` | 試験関数の名前の多重集合と表の行が減っていない（減った分は removals に根拠がある）。不変条件の試験は減らない | 各 bundle・dedupe・adjust leaf の check。統合は `git merge-base HEAD main` |
| `sh mutate.sh <patch> <crate> <filter>` → `KILLED` | 消す試験の代わりの試験が、対象 module の変異を落とす（D5-2b） | dedupe leaf が removals の `mutation` に書く |
| `cargo nextest list -p <crate>` の試験の数 | 束ねの前後で同じ。binary の数は束の数だけ減る | 各 bundle leaf が進捗に前後を書く |
| `cargo nextest run -p <crate> -E 'binary_id(<crate>::<bundle>)'` と `cargo test -p <crate> --test <bundle>` | 束の試験が別 process でも 1 process でも通る（D3） | 各 bundle leaf と measure-run |
| `bash scripts/dev/test-affected.sh` | 対象 crate の試験が通る。F1 の後は `--workspace` の features で build する | 全 leaf |
| `python3 scripts/dev/test-target-inventory.py --json` | test target の数（B0 と同じ方法） | 段 measure |
| `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` | `size.responsibility-growth`・`dup.test-fixture`・`policy.changed` に new・worsened が無い | 全 leaf |
| `python3 test_guard.py` | cases.py・mutate.sh 自体の 16 件の unittest | 段 survey（済み）。台本を変える leaf だけ再実行 |

- 守る不変条件と、既存の試験で検出できない理由: 「試験の数と不変条件の試験を減らさない」は `cases.py` だけが base と比べて検出する（nextest は数を比べない）。「束の中で process を汚染しない」は nextest の実行では検出できない（別 process）ので `cargo test --test <bundle>` を足す。

## 束ねの手順（bundle・adjust の leaf が従う）
1. 自分の leaf の target を `python3 classify.py --verify-leaf <leaf> inventory.json` の出力で確かめる（最初は全件が「元の path にある」で落ちる）。他の leaf の target、keep・r6 の target、数字を含む file には触らない。
2. 束ごとに `crates/<crate>/tests/<bundle>/main.rs` を作り、支援 module を 1 回だけ宣言する: `#[path = "../common/mod.rs"] mod common;`（celeris は `#[path = "../support/mod.rs"] mod support;`。`mod.rs` を `#[path]` で読むと入れ子の `pub mod cos_ops;` は `tests/common/` から解決される。rustc 1.98、edition 2024 で確認）。target が `#[path]` で読む repo root の `tests/support/*.rs` や他 crate の `tests/userns_gate/mod.rs` も `main.rs` に 1 回だけ宣言し、相対 path は dir が 1 段深くなる分 `../` を 1 つ足す。
3. 元の file を `git mv crates/<crate>/tests/<target>.rs crates/<crate>/tests/<bundle>/<target>.rs` で動かし、`main.rs` に `mod <target>;` を足す（名前順）。
4. 動かした file の先頭の `mod common;`（と `#[path] mod …;`）を消し、`use crate::common;`（必要な名前だけ）に置き換える。`use common::*;` と `common::foo()` の形は edition 2018 以降の uniform path でそのまま解決する。試験関数の中身は変えない。使っていない `use` は置かない（clippy の `-D warnings` で落ちる）。
5. `cargo nextest list -p <crate>` の試験の数を base と比べ（同じであること）、binary の数の前後（例: task-api 114 → 67）を進捗に書く。`cargo nextest run -p <crate> -E 'binary_id(<crate>::<bundle>)'` と `cargo test -p <crate> --test <bundle>` を通す。`cargo test` だけで落ちる target は D3 に従って束から外し、`inventory.json` と進捗に理由を書く。
6. `classify.py --verify-leaf <leaf>`・`cases.py --crate <crate> --base "$CELERIS_WU_BASE"`・`bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`check-health.sh --base "$CELERIS_WU_BASE"`・B0 の `slices-check.py --write-set R5` を通し、進捗 `agent-docs/progress/2026-11-10-health-v2-r5/<leaf key>.md` に commit を件名で書く。

## 付記: 実装との突き合わせ
- 2026-11-10（unit adr）: 設計のみ。`inventory.json` の束は段 survey の案のまま採った（名前・上限・鍵を変えていない）。実装の leaf が束を外した・分けたときはここに書く。
- 2026-11-10（unit reclass）: 地図の brace の path を r6 に数える。段 bundle の 3 leaf（`bundle-api-a`・`bundle-celeris`・`bundle-ctl`）が test-affected の health gate の `arch-map.wrong_path` で止まった。原因は `classify.py` の `external_refs` が地図の行を path の部分一致で探していたことで、brace の path（`crates/celerisctl/tests/{no_migrate,cli_resolution,bin_smoke}.rs` など）を見落とした。`classify.py` は地図の表の行の path を `scripts/dev/check-architecture-map.py` の `collect_backtick_paths`（と相対リンク）で集めるように直し、brace の展開を自分では持たない。`--check` は地図を集め直し、地図が挙げる target の leaf が r6（D4-1 の keep の理由があれば keep）であることを確かめる。これで 20 target が r6 になった: 段 bundle・adjust から 19 件（task-api 15: auth_and_guards・browser_doctor・browser_live・browser_site_policy_db・chat_stream・cluster_connect_admin・cos_chat_attach_pin・cos_operations・lifecycle・organization・reports・request_lock_counts・task_management・task_tree・tree_adopt、celeris 1: unpromoted_release、celerisctl 3: bin_smoke・cli_resolution・no_migrate）と、1 件だけの組で keep だった task-api の `execution`。束は同じ規則（D1・D2）で組み直したので、件数が減った鍵の束は名前と中身が変わった（`task_plan_suite` は 0 件になり消えた。task-api `browser_eval_surface_suite_a`・`_b` は 6 件の `browser_eval_surface_suite` 1 つになった）。1 件だけになった束は無かった。表は上のとおり直した。記録は [reclass.md](../progress/2026-11-10-health-v2-r5/reclass.md)。
