---
title: LoC 削減 v3 F — task-worker・task-api の整理（試行 1: production 純減 unit 225 行、閾値 2,500 は未達で人に質問）
tasks: [01M7J3NKVC2KQ4NGJ4GESHJV0B]
status: blocked
updated: 2026-11-17
---
# LoC 削減 v3 F — task-worker・task-api の整理

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 F。依存は E（01M7J3NKVAH9NG53C5FEFNQJYK）。

## 結論（試行 1）
- 提出時の HEAD で、production 全言語の純減は unit（`git merge-base HEAD main` → HEAD）で **225 行**、固定（開始時 main 415a19a7004cb6a73b872e861fa8d1cf634dddbb → HEAD）で **7,237 行**。production Rust も同じ 225 / 7,237 行、own_total_with_tests は 235 / 10,136 行。
- 受け入れの size check（閾値 `2500 1 - - -`）は exit 1（`FAIL ['prod.unit=225<2500']`）。目標に届いていないので done にしない。
- C・D・E の unit の合計 7,012 行（E の進捗）に F の 225 行を足すと 7,237 行で、固定の計測と一致する（差 0 行。無関係の main の production の変更は無い）。
- write set（task-worker・task-api とその試験）の中で、承認なしに消せるものは残り約 500 行と見積もった（下の「残りの候補」）。2,500 行には、承認が要る機能の廃止（C4 Console・C5 deprecated API・aider adapter）か閾値の変更が要る。人への質問は「提案」に書いた。

## 消した対象（production）
- task-worker 研究系 adapter（`langmem.rs`・`local_deep_research.rs`・`paperqa.rs`）: Python ランナーの起動（`Command` の組み立て・`db_guard::launch`）、ランナーと入力 JSON の書き出し、標準出力の監視 loop の写し 3 組を、新しい `research_runner.rs` の `runner_command`・`write_runner_files`・`stream_child` に 1 つにした（`stream_child` と `StreamedRun` は paperqa にあったものを移した）。`single_line_summary`・`truncate_chars`（末尾の印なし）の写し 3 組も同じ module に 1 つにした。行の意味（`progress:`・`CELERIS_RESULT`・`CELERIS_ACQUIRE`）と langmem の `idle_timeout_secs` の上限の規則は各 adapter に残した。paperqa の取得の段の「起動できなければ 0 件で先へ進む」と、process group の登録の仕方も変えていない。
- task-worker harness（`claude_code.rs`・`codex.rs`・`pi.rs`・`aider.rs`・`acp.rs`）と研究ランナー: 監視 loop の「`deadline_terminal` → `timeout(read_line_limited)`」の写し 5 組を `child_stream::next_line` に、stdout の reader・`stdout.log`・stderr の写しの task の用意の写し 6 組を `child_stream::open_child_streams` にした。byte 数で切る `truncate` の写し 3 組（acp・codex・claude_code）を `progress::truncate_bytes` にした。acp の `wait_for_response` は行の読み方（`pump_one_line`）が違うので `next_line` に替えていない。
- task-api `notify.rs`・`state.rs`・`lib.rs`: `GET /api/v1/notify` の効かない 4 値（`inbox_batch_secs`・`inbox_reminder_secs`・`digest_interval_secs`・`digest_max_lines`）と `ApiSettings` の `notify_*` の 4 欄。E の進捗が F を撤去の責任者にした項目で、同じ変更で celeris の `NotifyConfig` の 4 欄・既定値の関数 4 個・`daemon/api.rs` の受け渡しを消し、4 欄を読み込みエラーにした（`deny_unknown_fields`）。生成 schema（`docs/api/v1/api-v1.schema.json`、`web/api/generated/`）は生成し直した。運用手順 `docs/ops/inbox-notifications.md` と ADR-0133 の付記を直した。
- task-api `console.rs`: `#[allow(dead_code)]` の付いた呼び手の無い `project_task_ids`。

## 変えた試験
- `db_guard_tests.rs` の `every_worker_spawn_site_goes_through_launch`: 研究系 adapter の起動は `research_runner.rs` の 1 か所の `db_guard::launch` を通るので、一覧を `research_runner.rs`（1）に替えた。langmem・local_deep_research・paperqa には `runner_command(` の数（1・1・2）と、`Command::new(`・`db_guard::launch(`・`container::wrap(` が無いことを求める検査を足した。adapter が自分で `Command` を組んで namespace を通らずに起動する変更は、前と同じくこの試験で落ちる。
- `cos_delivery_suite_c/notify.rs`: `get_notify_status_exposes_route_settings_and_last_successful_sends` を `get_notify_status_exposes_the_last_successful_sends` に改名し、4 値の assert を「4 値を返さない」assert に替えた（契約を廃止した）。経路ごとの最終送信時刻の assert は残した。
- celeris `config/tests.rs`: 4 欄を読む assert を「4 欄を書くと読み込みエラー」の assert に替えた。`notify/tests.rs` の既定値の assert 4 行を消した（欄が無い）。
- task-api `handlers/tests.rs`・`tests/common/mod.rs`: 設定の 4 欄の受け渡しを消した。

## 測定（提出時の HEAD で実行）
- 受け入れの size check（開始時 main の `scripts/dev/source-size-report.py`・`source-size-report.toml` を `git show` で取り出し、`--exclusive-classes` で 3 点を測る。閾値 `2500 1 - - -`）→ exit 1。
  - production 全言語: unit 純減 225、固定純減 7,237
  - production Rust: unit 225、固定 7,237
  - own_total_with_tests: unit 235、固定 10,136
  - unit の class ごとの純減: production 225、external_test 10、inline_test 0、generated 29、schema 25。試験・生成物への付け替えは無い（external_test・generated・schema も減った）。
- C・D・E・F の unit の合計: 2,701 + 2,558 + 1,753 + 225 = 7,237 行 = 固定の 7,237 行（差 0 行）。

## 検査（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `bash scripts/dev/test-affected.sh` → exit 0。repo-health `verdict=pass`（new=0・worsened=0）、nextest `6132 tests run: 6132 passed, 22 skipped`（celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）、web の型検査と unit `716 passed`。
- 途中で repo-health の deps.cycle が 1 件落ちた（`next_line`・`open_child_streams` を `adapter.rs` に置いたので、`adapter` → `subprocess` の辺ができ、既存の循環が 3 単位から 6 単位に広がった）。2 関数を新しい `child_stream.rs`（`adapter` と `subprocess` を使うだけで、どちらからも参照されない）に移して直した。
- `cargo nextest run -p task-worker` → `1315 tests run: 1315 passed, 9 skipped`。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 変更（commit の件名）
- `loc-reduction-v3 F: 研究系 adapter の Python ランナーの起動・読み取りと harness の監視 loop の締め切り付きの 1 行読みを共有の関数にまとめ、truncate の写しを 1 つにする`
- `loc-reduction-v3 F: GET /api/v1/notify の効かない 4 値と [notify] の 4 欄（inbox_batch_secs・inbox_reminder_secs・digest_interval_secs・digest_max_lines）を消し、欄を読み込みエラーにする`
- `loc-reduction-v3 F: harness・研究ランナーの stdout・stderr の用意を open_child_streams に、console の呼び手の無い project_task_ids を消す`
- `loc-reduction-v3 F: 起動の入口の試験を research_runner の runner_command に合わせる`
- `loc-reduction-v3 F: 監視 loop の共有の関数を adapter から child_stream に移し、adapter と subprocess の循環を広げない`
- `loc-reduction-v3 F: 移した summary の helper の doc comment を移し先に付け直す（clippy empty_line_after_doc_comments）`
- この進捗ファイルの commit。

## 設計 brief（高 risk の path は変えていない）
1. 根本原因: 研究系 adapter 3 個と harness 5 個が、子プロセスの起動・stdout の監視 loop・stderr の写しを adapter ごとに写して持っていた。
2. 責務 owner: 締め切りの規則は `adapter::deadline_terminal`、行の読み取りは `subprocess::read_line_limited`。共有の手順は新しい `child_stream`（harness と研究ランナー）と `research_runner`（研究系 adapter）に置いた。
3. 既存の再利用先: paperqa の `stream_child`（両方の段で共用していたもの）をそのまま移した。`progress::truncate_chars` の隣に byte 版を置いた。
4. 選択肢: (a) `adapter.rs` に置く（deps.cycle が広がるので不採用）、(b) `subprocess.rs` に置く（sandbox の高 risk path を変えるので不採用）、(c) 使う側にだけ依存する新しい module（採用）。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 上の写しと `[notify]` の 4 欄・`GET /notify` の 4 値。
7. 不変条件: 起動は全部 `db_guard::launch` を通る（`every_worker_spawn_site_goes_through_launch`）。締め切りの規則（wall clock は `BudgetExhausted`、idle は retryable な `Error`）は変わらない（task-worker の adapter の試験 1,315 件）。`sandbox` の高 risk path（`db_guard`・`subprocess`・`tool_policy`）は変えていない。

## 残りの候補（承認なしで消せる見込み。試行 1 では消していない）
- 試験だけが呼ぶ production の関数（`dead2.py` の走査、約 300 行）: `run_tmpdir::sweep_stale`（33）、`scratch/gc.rs` の `account_shared_extents`（31）、`browser_supervisor::reap_on_start`（28）、`local_worktree::remove_if_clean`（19）、`target_sweep::group_deps_by_key`（13）ほか。どれも本体の経路が別にあるかを 1 件ずつ確かめ、試験を本体の入口に替える必要がある。browser の試験の入口（`run_with_executable` ほか）は隔離の試験が使うので候補にしない。
- task-api の handler の写し（約 150 行）: `stats.rs` の `catch_up` 2 組、`handlers/accounts.rs` の login の開始・取り消し、`decisions.rs` の 4 組、`cos/ops/*` の dispatch の 4 組、`handlers/clusters.rs`・`execution.rs`・`task_io.rs` の組。1 組 10〜15 行。
- 研究系 adapter の残りの写し（約 50 行）: config の型の欄（`local_deep_research.rs`・`paperqa.rs` の先頭）、結果の読み戻し（`terminal_from_summary`）。
- 候補にしないもの: C10（`WorkUnitView` は `quota`・`running_run_id` を足し、`task_id`・`lease_*`・`needs_decisions` を出さない。task-core の行と JSON が違う）、C11（task-api と task-dispatch の循環依存を避けるための写しで、1 つにするには task-core の変更が要る）、`cos/ops/*` の `EXCLUDED`（試験が全 route の分類を確かめる材料）。

## 未解決
- 受け入れの size check は閾値 2,500 に 2,275 行足りない（unit 225 行）。人の判断を待つ（「提案」）。
- `source_state_view`（task-api `llm_sources.rs`、89 行）は本体から呼ばれていない（試験 `routing_source_state.rs` だけ）。ADR 2026-10-04-multi-objective-model-routing Phase 2 の未配線の部品で、`GET /llm/sources` の `deployments` は今は空。消すか配線するかは人の判断が要る。
- E が残した task-core・task-ops の項目（`notification_scan_at`・`is_cluster_login_needed_headline` と doc 参照 3 か所）は F の write set の外なので触っていない。

## 提案（人に決めてもらうこと）
1. 閾値を試行 1 の実測値（production unit 225 行）に下げて F を閉じる。
2. 承認が要る廃止を認め、write set を広げる。production の行数の見積もり:
   - C4 Console 互換（`/console`・`/console/stream`・`/console/instruct`）: task-api `console.rs` 1,126 行、celeris-mcp `tools/console.rs` 245 行、task-core `store/legacy.rs` 306 行、web `features/console/` と `routes/console.tsx` 約 930 行。合計 約 2,600 行。本番 DB では 2026-11-16 にも `messages` の node_id='cos' が書かれていた（A の調査）ので使われている機能で、SPEC の変更と task-core・celeris-mcp・web への write set の拡大が要る。
   - C5 deprecated API（`GET /api/v1/inbox`・`POST /reports/read`・`POST /reports/notified`）: 約 150〜350 行。CoS の操作表（`cos/ops/projects.rs`）が 2 つを載せており、`crates/celeris/tests/integration_api.rs` が `GET /api/v1/inbox` を使う。
   - aider adapter: task-worker `aider.rs` 351 行と celeris `config/adapters.rs` の `[adapters.aider]`・例の設定 39 行。本番の config に `[harnesses.aider]` は無い（2026-11-17 に読み取りで確認）。task を aider に回す経路が本番に残っていないかは未確認。
3. 残りの候補（約 500 行）は承認なしで次の試行で消す。2 を認めない場合、1 か 3 のどちらでも 2,500 行には届かない。

## 本番への移行
- `[notify]` の 4 欄は読み込みエラーになる。本番の `~/.config/celeris/config.toml` の `[notify]` にはこの 4 欄が無い（2026-11-17 に読み取りで確認）ので、本番 config の書き換えは要らない。
