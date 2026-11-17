---
title: LoC 削減 v3 F — task-worker・task-api の整理と Console 互換・旧い報告 API・aider の撤去（試行 2: production 純減 unit 4,151 行、size check exit 0）
tasks: [01M7J3NKVC2KQ4NGJ4GESHJV0B]
status: done
updated: 2026-11-17
completed: 2026-11-17
---
# LoC 削減 v3 F — task-worker・task-api の整理

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 F。依存は E（01M7J3NKVAH9NG53C5FEFNQJYK）。

## 結論（試行 2）
- 人が 2026-11-17 に 3 つの廃止（C4 Console 互換・C5 deprecated API・aider adapter）と、承認なしで消せる残りの候補の削除を認めた（operator が中継）。設計は [ADR 2026-11-17-console-deprecated-api-aider-removal](../adr/2026-11-17-console-deprecated-api-aider-removal.md)。
- 提出時の HEAD で、production 全言語の純減は unit（`git merge-base HEAD main` → HEAD）で **4,151 行**、固定（開始時 main 415a19a7004cb6a73b872e861fa8d1cf634dddbb → HEAD）で **11,163 行**。production Rust は unit 2,727・固定 9,739 行、own_total_with_tests は unit 7,687・固定 17,588 行（下の「測定」）。
- 受け入れの size check（閾値 `2500 1 - - -`）は exit 0。
- C・D・E の unit の合計 7,012 行に F の 4,151 行を足すと 11,163 行で、固定の計測と一致する（差 0 行。無関係の main の production の変更は無い）。
- 親の最終目標（累積 10,000 行以上・production Rust も純減・own_total も純減）は、固定の計測で production 11,163・Rust 9,739・own 17,588 行の純減。

## 試行 2 で消した対象（production）
### 承認された 3 つの廃止
- C4 Console 互換（ADR D1・D2）:
  - task-api: `console.rs`（`GET /api/v1/console`・`GET /api/v1/console/stream`・`POST /api/v1/console/instruct`、1,141 行）。`GET /tasks/{id}/runs/{run_id}/events` は run のログ画面が使うので `run_events.rs` に移した。`types.rs` の `ConsolePage`・`ConsoleBlock`、schema の 5 型、CoS 操作の除外 `console_instruction_chain`。
  - task-ops: `console.rs`（Console の写像、377 行）。`GET /tasks/{id}/timeline` が使う時刻の比較（`at_nanos`、5 行）だけを `task-api/src/timeline.rs` に移した。
  - task-core: `store/legacy.rs` の `chat_legacy_console_messages`（Console の読み取り専用、約 85 行）。
  - celeris-mcp: `tools/api.rs` の拒否表の `/api/v1/console/stream`。
  - web: `features/console/`（9 file）、`routes/console.tsx`・`routes/org.$id.tsx`（その人の Console）・`routes/org.secretary.tsx`、`features/home/console-region.tsx`・`home-entries.tsx`、gateway の `server/console.js`（`/console/stream` の中継）と `app.js`・`auth.js`・`spa-routes.js` の配線、ナビの「Console」、組織画面の「話す」、shell の空の Console の置き場、`consoleKeys`、未使用の `inboxQuery`。
  - SPEC の Console の記述、`docs/api/v1/api.md` の 5 経路の節と表、CoS の skill（`config/skills/cos-operator/`）の記述、`docs/guides/knowledge.md`。ADR-0048・CoS チャット ADR・ADR 2026-10-09-cos-operations-all-mutations に付記（操作表から撤去した 3 経路の行を消した）。
- delivery の通知（ADR D2）: `crates/celeris/src/delivery.rs` の人に向けた通知（「取り込み完了」「デプロイ準備完了」「取り込み・リリース準備の確認が必要」）は、`messages` に書いた直後に `SqliteStore::cos_messages_project`（起動時の写し `backfill_legacy_cos` と同じ関数）で CoS の固定スレッドへ写す。これまでは Console に出て、CoS チャットには次の daemon の起動の後に写っていた。
- C5（ADR D3）: `POST /api/v1/reports/read`・`POST /api/v1/reports/notified`、CoS 操作の `report.read`・`report.notified`、それだけが進めていた API のメモリの `last_notified_at` と `DaemonSnapshot.reports` の `last_notified_at`・`notify_now`、task-core の `report::notify_now`・`NOTIFY_INTERVAL_SECS`。
- aider（ADR D4）: task-worker `aider.rs`（335 行）と試験、`routing_context_transport::configure_aider`、celeris `[adapters.aider]`（`AiderAdapterConfig`）と `daemon/adapters.rs` の配線、例の設定 `config/celeris.aider.example.toml`。ADR-0061 に付記。

### 承認なしで消せた残りの候補
- 試験だけが呼ぶ task-worker の本体の関数（本体の経路が別にあるものだけ）。試験は本体の入口に替えた。
  - `route::request_adapter` と、それだけが返していた `RouteError::Missing`（本体は全部 `route_adapter` を route を持って呼ぶ。`celerisctl worker`・dispatcher の `route_adapter`）。試験 `worker_route_without_a_route_fails_closed` は守る契約（route の無い request）が本体に無くなったので消した。
  - `workspace_targets::finished_task_targets`（`scan_finished_task_targets(..).found` の 1 行の包み）。
  - `target_sweep::group_deps_by_key`（本体の束ね方は task-dispatch `target_sweep` の `deps_key`。`deps_key` の試験は残した）。
  - `scratch::gc::account_shared_extents`（本体は `SharedExtentIndex`。同じ試験の後半が `SharedExtentIndex` の 1 回だけ数える規則と `measure_tree_shared_with` の FIEMAP 失敗の倒し方を確かめている）。
  - `LocalWorktree::remove_if_clean`（本体の後片付けは `remove_with_branch` と `task_repos`。worktree を作り直す試験は `git worktree remove` を直に呼ぶ）。
- 写しの集約（同じ入出力・同じ失敗の扱いのものだけ）:
  - task-api の admin channel の「送信 → `timeout` で待つ」の写し 13 か所（`handlers/accounts.rs` 7・`handlers/clusters.rs` 3・`handlers/config_fields.rs`・`handlers/reload.rs`・`notify.rs`）を `admin::call` に。送れないとき（何も起きていない）は内側の `Err`、落とされた・時間切れ（結果が分からない）は外側の `Err` で、`Effect` を返す site の 2 つの区別をそのまま保つ。500 の文言は site ごとの文字列を引数で渡して変えていない。
  - task-api `stats.rs` の `catch_up` の写し 2 個を 1 つの関数に。
  - task-api `cos::cos_problem`（`chat::chat_problem` と同じ写像）を消して `chat_problem` を使う。
  - task-api の SSE の応答の組み立て（`GET /stream` と `GET /chat/threads/{t}/stream`）を `sse::response` に。
  - task-worker `scratch/reflink.rs` の FIEMAP の loop 2 個（`fiemap_all_shared`・`fiemap_extents`）を `fiemap_walk` に。最初の ioctl だけの `FIEMAP_FLAG_SYNC`、最初の非共有 extent で止まる規則は引数と `visit` の戻り値で保つ。
  - 研究系 adapter 3 個のランナーの約束（`SUMMARY_MAX_CHARS`・`PROGRESS_LINE_MAX_CHARS`・`PROGRESS_PREFIX`・`RESULT_PREFIX`）と、langmem・local-deep-research の行の扱い（`progress:` を進捗に、`CELERIS_RESULT` を結果に）を `research_runner` に 1 つにした。paperqa の取得の段（`CELERIS_ACQUIRE`）は意味が違うので adapter に残した。

## 消さなかったもの（理由）
- `GET /api/v1/inbox`（C5 の 1 つ）: 廃止は認められたが残した。release の検証 `scripts/selfdeploy/verify.sh` の `probe_api` がこの path の 200 と JSON を求める。`scripts/selfdeploy/` は高 risk の path class で、人が認めた write set の外にある。撤去の条件（`verify.sh` を `GET /inbox/items` に替える変更と試験 14 か所・`daemon_soak.py` の書き換え）と責任者（親 01M7J3N68JRNTP5Q4G934QWF36 の closeout が後続 task を人に提案）は ADR D3 に書いた。
- celeris-mcp の `console_instruct`・`console_reply`: 名前は Console だが CoS チャットの入口（`chat_mcp_instruct`）で、運用 session の中継に使う現行の経路。
- `POST /org/cos/messages`（`chat_console_enqueue`）と起動時の写し `backfill_legacy_cos`: 承認の範囲の外。写しは delivery の通知の写し先として使う。
- 未配線の仕組み（ADR が使うと書いているのに本体から呼ばれない。消すか配線するかは人の判断）: `run_tmpdir::sweep_stale`（ADR 2026-10-07-build-tmp-hygiene D2.2。task-dispatch `worker_task.rs` の注釈が「拾い直す」と書く）、`browser_supervisor::reap_on_start`（ADR-0116。launcher の起動時の回収）、task-api `llm_sources::source_state_view`（ADR 2026-10-04-multi-objective-model-routing Phase 2）。
- browser の試験の入口（`run_with_executable`・`verify_launcher_observation`・`launcher_runtime_facts` ほか）と `browser_identity::restore_for_session`: 隔離・credential の試験が使う。
- 振る舞いの違う写し: `task_actions` の `approve`・`reject`（共通の関数にすると型の引数で行数が増えた。+55/−23 で取り消した）、`execution.rs` の phase-gate と plan-gate の拒否（文言が違う）、`decisions.rs` の `answer_op`・`withdraw_op`・`revise_op`（既に `resolve_op` の薄い包みで、本文の型が違う）、harness の監視 loop の残り（行の意味が harness ごとに違う）。
- C10・C11: 試行 1 の「残りの候補」に書いた理由のまま。

## 試行 2 の設計 brief
1. 根本原因: ホームが CoS チャットに移った後も、Console 互換の 3 経路と画面、deprecated の報告 API、本番で使われない aider adapter が残っていた。task-api・task-worker には、同じ入出力の手順（admin channel への送信と待ち、SSE の応答、FIEMAP の loop、研究ランナーの行の扱い）が site ごとに写されていた。
2. 責務 owner: 通知の行き先は `delivery.rs`（`messages` を書く所）。admin channel の約束は `task-api/src/admin.rs`（`AdminRequest` を定義する所）。SSE は `sse.rs`。FIEMAP は `scratch/reflink.rs`。研究ランナーの約束は `research_runner.rs`。
3. 既存の再利用先: 通知の写しは起動時の写しと同じ `SqliteStore::cos_messages_project`。試験だけが呼んでいた関数は本体の `route_adapter`・`scan_finished_task_targets`・`SharedExtentIndex`・`deps_key`・`remove_with_branch` に替えた。
4. 選択肢: `GET /api/v1/inbox` も消す案は、release の検証 `verify.sh`（selfdeploy の高 risk path、write set の外）を変える必要があるので採らなかった。`approve`・`reject` の共通化は行数が増えたので取り消した。
5. 新しい状態・分岐・fallback・port: 無し。`admin::call` は既存の送信と待ちの手順を 1 か所にしただけで、時間切れの秒数と 500 の文言は site ごとの引数のまま。
6. 撤去対象: 上の「試行 2 で消した対象」。
7. 不変条件: 認可（`require_admin`・`require_active`・CoS の操作表の分類）は変えていない（task-api の `cos_ops_registry_matches_adr_domain_assignment_and_exclusions` が ADR の表と操作表の一致を確かめる）。admin の失敗の 2 つの区別（何も起きていない／結果が分からない）は `Effect` を返す site で保つ。credential・sandbox・store・lease・budget・selfdeploy の高 risk path は変えていない（task-core `store/legacy.rs` は Console の読み取り専用の関数の削除だけ）。

### 試行 2 で変えた試験
- 消した契約の試験を消した: task-api `unmapped_suite_a/console.rs`・`console_instruct.rs`、`reports.rs` の既読・通知の試験、`inbox_notifications.rs` の旧 API の部分、task-ops `console/tests.rs`、task-worker `aider/tests.rs`、web の Console の画面と e2e（`console.spec.ts`・`home-layout.spec.ts` ほか）、gateway の `console.test.mjs`。
- task-worker: `worker_route_without_a_route_fails_closed`（route の無い request を返す入口が無くなった）と `cleanup_removes_a_clean_worktree_and_keeps_a_dirty_one`（`remove_if_clean` を消した）を消した。`group_deps_by_key`・`account_shared_extents` の assert を消し、同じ試験の残り（`deps_key`、`SharedExtentIndex`・`measure_tree_shared_with`）は残した。route の試験は `route_adapter` を、worktree の作り直しの試験は `git worktree remove` を直に呼ぶ。

## 変更（commit の件名）
- `loc-reduction-v3 F: 研究系 adapter の Python ランナーの起動・読み取りと harness の監視 loop の締め切り付きの 1 行読みを共有の関数にまとめ、truncate の写しを 1 つにする`
- `loc-reduction-v3 F: GET /api/v1/notify の効かない 4 値と [notify] の 4 欄（inbox_batch_secs・inbox_reminder_secs・digest_interval_secs・digest_max_lines）を消し、欄を読み込みエラーにする`
- `loc-reduction-v3 F: harness・研究ランナーの stdout・stderr の用意を open_child_streams に、console の呼び手の無い project_task_ids を消す`
- `loc-reduction-v3 F: 起動の入口の試験を research_runner の runner_command に合わせる`
- `loc-reduction-v3 F: 監視 loop の共有の関数を adapter から child_stream に移し、adapter と subprocess の循環を広げない`
- `loc-reduction-v3 F: 移した summary の helper の doc comment を移し先に付け直す（clippy empty_line_after_doc_comments）`
- `loc-reduction-v3 F: 試行 1 の進捗ファイルを足す（size check は未達で人に質問）`
- `loc-reduction-v3 F: 進捗の数値を提出時の HEAD の測定に直す`
- 試行 2:
  - `loc-reduction-v3 F: Console 互換（GET /console・/console/stream・/console/instruct と web の画面）・POST /reports/read・/reports/notified・aider adapter を撤去し、delivery の通知を書いた時点で CoS チャットへ写す`
  - `loc-reduction-v3 F: delivery::advance を伸ばさず通知を CoS チャットへ写し、code の注釈の ADR 名を短くする`
  - `loc-reduction-v3 F: CoS 操作の ADR の表から撤去した 3 経路の行を消し、ADR と進捗を試行 2 に合わせる`
  - `loc-reduction-v3 F: task-worker の試験だけが呼ぶ本体の関数（request_adapter・finished_task_targets・group_deps_by_key・account_shared_extents・remove_if_clean）を消し、試験を本体の入口に替える`
  - `loc-reduction-v3 F: task-api の admin channel の送信と待ちの写し 13 か所を admin::call に、統計の catch_up の写しと scratch の FIEMAP の 2 つの loop を 1 つにする`
  - `loc-reduction-v3 F: 研究系 adapter のランナーの約束（summary・progress 行の上限と目印・CELERIS_RESULT の読み取り）を research_runner の 1 か所にする`
  - `loc-reduction-v3 F: cos_problem を chat_problem に 1 つにし、SSE の応答の組み立てを sse::response に共有する`
  - この進捗ファイルと ADR D3 の commit。

## 試行 1 の内容
### 試行 1 で消した対象（production）
- task-worker 研究系 adapter（`langmem.rs`・`local_deep_research.rs`・`paperqa.rs`）: Python ランナーの起動（`Command` の組み立て・`db_guard::launch`）、ランナーと入力 JSON の書き出し、標準出力の監視 loop の写し 3 組を、新しい `research_runner.rs` の `runner_command`・`write_runner_files`・`stream_child` に 1 つにした（`stream_child` と `StreamedRun` は paperqa にあったものを移した）。`single_line_summary`・`truncate_chars`（末尾の印なし）の写し 3 組も同じ module に 1 つにした。行の意味（`progress:`・`CELERIS_RESULT`・`CELERIS_ACQUIRE`）と langmem の `idle_timeout_secs` の上限の規則は各 adapter に残した。paperqa の取得の段の「起動できなければ 0 件で先へ進む」と、process group の登録の仕方も変えていない。
- task-worker harness（`claude_code.rs`・`codex.rs`・`pi.rs`・`aider.rs`・`acp.rs`）と研究ランナー: 監視 loop の「`deadline_terminal` → `timeout(read_line_limited)`」の写し 5 組を `child_stream::next_line` に、stdout の reader・`stdout.log`・stderr の写しの task の用意の写し 6 組を `child_stream::open_child_streams` にした。byte 数で切る `truncate` の写し 3 組（acp・codex・claude_code）を `progress::truncate_bytes` にした。acp の `wait_for_response` は行の読み方（`pump_one_line`）が違うので `next_line` に替えていない。
- task-api `notify.rs`・`state.rs`・`lib.rs`: `GET /api/v1/notify` の効かない 4 値（`inbox_batch_secs`・`inbox_reminder_secs`・`digest_interval_secs`・`digest_max_lines`）と `ApiSettings` の `notify_*` の 4 欄。E の進捗が F を撤去の責任者にした項目で、同じ変更で celeris の `NotifyConfig` の 4 欄・既定値の関数 4 個・`daemon/api.rs` の受け渡しを消し、4 欄を読み込みエラーにした（`deny_unknown_fields`）。生成 schema（`docs/api/v1/api-v1.schema.json`、`web/api/generated/`）は生成し直した。運用手順 `docs/ops/inbox-notifications.md` と ADR-0133 の付記を直した。
- task-api `console.rs`: `#[allow(dead_code)]` の付いた呼び手の無い `project_task_ids`。

### 試行 1 で変えた試験
- `db_guard_tests.rs` の `every_worker_spawn_site_goes_through_launch`: 研究系 adapter の起動は `research_runner.rs` の 1 か所の `db_guard::launch` を通るので、一覧を `research_runner.rs`（1）に替えた。langmem・local_deep_research・paperqa には `runner_command(` の数（1・1・2）と、`Command::new(`・`db_guard::launch(`・`container::wrap(` が無いことを求める検査を足した。adapter が自分で `Command` を組んで namespace を通らずに起動する変更は、前と同じくこの試験で落ちる。
- `cos_delivery_suite_c/notify.rs`: `get_notify_status_exposes_route_settings_and_last_successful_sends` を `get_notify_status_exposes_the_last_successful_sends` に改名し、4 値の assert を「4 値を返さない」assert に替えた（契約を廃止した）。経路ごとの最終送信時刻の assert は残した。
- celeris `config/tests.rs`: 4 欄を読む assert を「4 欄を書くと読み込みエラー」の assert に替えた。`notify/tests.rs` の既定値の assert 4 行を消した（欄が無い）。
- task-api `handlers/tests.rs`・`tests/common/mod.rs`: 設定の 4 欄の受け渡しを消した。

### 試行 1 の測定
- 受け入れの size check（開始時 main の `scripts/dev/source-size-report.py`・`source-size-report.toml` を `git show` で取り出し、`--exclusive-classes` で 3 点を測る。閾値 `2500 1 - - -`）→ exit 1。
  - production 全言語: unit 純減 225、固定純減 7,237
  - production Rust: unit 225、固定 7,237
  - own_total_with_tests: unit 235、固定 10,136
  - unit の class ごとの純減: production 225、external_test 10、inline_test 0、generated 29、schema 25。試験・生成物への付け替えは無い（external_test・generated・schema も減った）。
- C・D・E・F の unit の合計: 2,701 + 2,558 + 1,753 + 225 = 7,237 行 = 固定の 7,237 行（差 0 行）。

### 試行 1 の検査
- `bash scripts/dev/test-affected.sh` → exit 0。repo-health `verdict=pass`（new=0・worsened=0）、nextest `6132 tests run: 6132 passed, 22 skipped`（celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）、web の型検査と unit `716 passed`。
- 途中で repo-health の deps.cycle が 1 件落ちた（`next_line`・`open_child_streams` を `adapter.rs` に置いたので、`adapter` → `subprocess` の辺ができ、既存の循環が 3 単位から 6 単位に広がった）。2 関数を新しい `child_stream.rs`（`adapter` と `subprocess` を使うだけで、どちらからも参照されない）に移して直した。
- `cargo nextest run -p task-worker` → `1315 tests run: 1315 passed, 9 skipped`。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

### 試行 1 の設計 brief（高 risk の path は変えていない）
1. 根本原因: 研究系 adapter 3 個と harness 5 個が、子プロセスの起動・stdout の監視 loop・stderr の写しを adapter ごとに写して持っていた。
2. 責務 owner: 締め切りの規則は `adapter::deadline_terminal`、行の読み取りは `subprocess::read_line_limited`。共有の手順は新しい `child_stream`（harness と研究ランナー）と `research_runner`（研究系 adapter）に置いた。
3. 既存の再利用先: paperqa の `stream_child`（両方の段で共用していたもの）をそのまま移した。`progress::truncate_chars` の隣に byte 版を置いた。
4. 選択肢: (a) `adapter.rs` に置く（deps.cycle が広がるので不採用）、(b) `subprocess.rs` に置く（sandbox の高 risk path を変えるので不採用）、(c) 使う側にだけ依存する新しい module（採用）。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 上の写しと `[notify]` の 4 欄・`GET /notify` の 4 値。
7. 不変条件: 起動は全部 `db_guard::launch` を通る（`every_worker_spawn_site_goes_through_launch`）。締め切りの規則（wall clock は `BudgetExhausted`、idle は retryable な `Error`）は変わらない（task-worker の adapter の試験 1,315 件）。`sandbox` の高 risk path（`db_guard`・`subprocess`・`tool_policy`）は変えていない。

### 試行 1 の時点の残りの候補（試行 2 で扱った。結果は上の「承認なしで消せた残りの候補」と「消さなかったもの」）
- 試験だけが呼ぶ production の関数（`dead2.py` の走査、約 300 行）: `run_tmpdir::sweep_stale`（33）、`scratch/gc.rs` の `account_shared_extents`（31）、`browser_supervisor::reap_on_start`（28）、`local_worktree::remove_if_clean`（19）、`target_sweep::group_deps_by_key`（13）ほか。どれも本体の経路が別にあるかを 1 件ずつ確かめ、試験を本体の入口に替える必要がある。browser の試験の入口（`run_with_executable` ほか）は隔離の試験が使うので候補にしない。
- task-api の handler の写し（約 150 行）: `stats.rs` の `catch_up` 2 組、`handlers/accounts.rs` の login の開始・取り消し、`decisions.rs` の 4 組、`cos/ops/*` の dispatch の 4 組、`handlers/clusters.rs`・`execution.rs`・`task_io.rs` の組。1 組 10〜15 行。
- 研究系 adapter の残りの写し（約 50 行）: config の型の欄（`local_deep_research.rs`・`paperqa.rs` の先頭）、結果の読み戻し（`terminal_from_summary`）。
- 候補にしないもの: C10（`WorkUnitView` は `quota`・`running_run_id` を足し、`task_id`・`lease_*`・`needs_decisions` を出さない。task-core の行と JSON が違う）、C11（task-api と task-dispatch の循環依存を避けるための写しで、1 つにするには task-core の変更が要る）、`cos/ops/*` の `EXCLUDED`（試験が全 route の分類を確かめる材料）。

## 未解決
- `GET /api/v1/inbox` の撤去（ADR D3 の撤去の条件と責任者）。
- 未配線の `run_tmpdir::sweep_stale`・`browser_supervisor::reap_on_start`・`source_state_view` は、消すか配線するかを人が決める（「提案」）。
- E が残した task-core・task-ops の項目（`notification_scan_at`・`is_cluster_login_needed_headline` と doc 参照 3 か所）は F の write set の外なので触っていない。

## 提案
- 試行 1 の提案（閾値・廃止の承認・残りの候補）は人が 2026-11-17 に (2) と残りの候補の削除で答えた。
- 親の closeout で人に提案すること: (a) `verify.sh` の `probe_api` を `GET /api/v1/inbox/items` に替え、`GET /api/v1/inbox` と旧い形を読む試験 14 か所を書き換える後続 task（selfdeploy の write set）、(b) 未配線の 3 つ（`sweep_stale`・`reap_on_start`・`source_state_view`）を配線するか消すかの判断。

## 本番への移行
- `[notify]` の 4 欄は読み込みエラーになる。本番の `~/.config/celeris/config.toml` の `[notify]` にはこの 4 欄が無い（2026-11-17 に読み取りで確認）ので、本番 config の書き換えは要らない。
- `[adapters.aider]` は読み込みエラーになる（`deny_unknown_fields`）。本番の config に aider の記述は無く、本番の runs に adapter=aider は 0 件（operator が 2026-11-17 に確認）なので、本番 config の書き換えは要らない。
- Console の画面は無くなる。delivery の通知は次の release から CoS チャットに直接写る（daemon の起動を待たない）。本番 DB の移行は無い（`messages` の行は残す）。
