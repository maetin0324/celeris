---
task: 01M5B84ZY6H4001ZYPSGN9VR34
tasks: [01M5B84ZY6H4001ZYPSGN9VR34]
title: 実dispatcher・worker・processによる専門分担適用検証（application）
work_unit: recheck
status: running
updated: 2026-10-19
---
# 実 dispatcher・worker・process による専門分担の適用検証（application）

前段の記録は同じ dir の `contract.md`（契約）、`propagation.md`（伝播。§8 は固定証拠の境界、§9 は統合検証）、`normal-e2e.md`（通常経路の無 LLM 回帰試験）。この文書は `crates/campaign/tests/ablation_application.rs` を作る葉群の記録。

## 調査（WU recheck、2026-10-19。コード変更なし）

### 0. 基点と最新 main
- 本 WU の HEAD = `3b58ae86`（integrate wu/normal-propagation）。`CELERIS_WU_BASE` = `3e22b62c`。
- `git fetch origin` 後の `origin/main` = `cb0cd7b8`（merge main caef190d into task branch: 実験セットと B1/C1 side manifest・系列台帳）。merge-base は `c9654bff`（fix-pilot-wiring）。**main は本 task branch より先に進んでいて、本 branch の祖先ではない**（`git merge-base --is-ancestor origin/main HEAD` が偽）。差分は 235 file（+28529/−370）。
- `git merge-tree --write-tree HEAD origin/main` の衝突は 16 file: progress 4 件、`crates/campaign/src/main.rs`、task-core の `decision_log.rs`・`delivery.rs`・`store/{decision_log,migrations,route_records,routing_evidence}.rs`、試験 6 件。migration 番号は main が `0087_decision_log_lane_stages`、本 branch が `0088_task_campaign_variant` なので番号は衝突しない。衝突するのは `store/migrations.rs` の登録行だけ。
- main 側で `crates/campaign/Cargo.toml` の `time` が dev-dependencies から dependencies に移っている。後続の infra 葉が dev-dependency を足すと、この行の近くで衝突しうる。

### 1. 既存 3 task の状態と重複回避
状態は `celerisctl --db /local/celeris/data/db/celeris.sqlite3 show <id> --json` で読んだ（読み取りのみ。path は `~/.config/celeris/config.toml` の `[db] path`）。`contract.md` §1 の時点（running / ready / ready）から状態が変わっている。

| task | 現状態（updated_at） | 実装・試験（再利用し、作り直さない） | 本 task が足すもの（重複しない範囲） |
|---|---|---|---|
| 01M58VFRBA6HG0SSF3CG5BKM91 設定確定 | **done**（2026-10-19T17:06:28Z） | manifest / lock / env-lock / preflight（`manifest.rs` `lock.rs` `env_lock.rs` `preflight.rs`）。試験 `campaign_lock.rs` `env_lock.rs` `preflight_contract.rs` `lock_cli.rs` | 非対象の同一性は、既存の `ExecutionPin`・env-lock catalog・`variant_proof::prove_runs` の照合を再利用する。新しい hash や lock は作らない |
| 01M58VG5C4XGK4E70M6WJ6MEX0 無LLM統合 | **cancelled**（2026-10-19T11:19:47Z） | runner・`CelerisExecutor`（`celeris_exec.rs`）・`mock_e2e.rs`・`lock_execution.rs`。投入・再開・冪等照会の経路は既存 | 投入・再開・冪等照会は既存の `CelerisExecutor` と通常 HTTP POST/GET を**実 daemon に対して**使う。経路を新しく作らない。cancelled なので、この task の実行証拠は流用しない |
| 01M58VGDNQ8GWGJPB8SJ73XSPF 主比較 pilot | **cancelled**（同上） | `pilot_wiring.rs` `pilot_fixtures.rs` | 実 pilot・実 LLM には触れない。A0/B0 との依存も足さない |

wire-variant の前段で入った実装・試験（変更せず、重複もさせない）:

| 層 | 実装 | 既存試験 | 既存試験で足りない点（本 task が埋める） |
|---|---|---|---|
| campaign 契約 | `crates/campaign/src/variant.rs`、`variant_proof.rs`（L2 照合） | `crates/campaign/tests/ablation_contract.rs`（hash 安定性、改変・再 seal・欠落・再開/冪等の不一致） | 契約単体の試験だけで、実 daemon はない |
| task-core / API | `variant_wire.rs`、migration 0088、`campaign_org_check_tx` | `store/campaign_variant_tests.rs`、task-api の試験 | — |
| dispatcher | `dispatcher/variant_pin.rs`（`check_assignee`・`variant_run_pin`・`set_variant_observation`） | `crates/task-dispatch/src/dispatcher/tests/variant_pin.rs`（13 件。retry / continuation / repair は in-process の dispatch_one を通す） | worker が in-process 偽 adapter で、実 process ではない |
| worker | `task-worker/src/campaign_pin.rs`（RunContext.campaign・証拠保存・再開時の照合） | `campaign_pin_tests.rs` | — |
| 通常 daemon 境界 | — | `crates/celeris/tests/variant_propagation.rs`（5 件。`task_api::serve_with_listener` を in-process の tokio task として起動し、`build_dispatcher` の tick を回す。観測値は試験が `set_variant_observation("inst-1", …)` で手で与える） | 実 `celeris` process ではない。run は起動しているが、計画出力の採用・孫・retry は通していない。観測値は手で与えたもの |

本 task の範囲（他と重複しない部分）: **実 `celeris` binary の process**（一時 DB・token・org・workspace を所有）に通常 HTTP POST を送り、実 dispatcher から実 fake adapter が起動する**実スタブ process** の `execution-plan.json` を通常どおり採用させる。その結果、子・孫が実際に生成され、決定的失敗による retry も実際に起き、spec-off/on の events と runs を読んで assert する。これは前段のどの試験にもない。

### 2. 実 worker adapter が非 LLM スタブ process を実行する経路
- **起動**: config の `[harnesses.<id>] kind = "fake"` + `command`/`args`、または `[adapters.fake] command = [...]`。`crates/celeris/src/daemon/adapters.rs:153-180`（`FakeAdapter::ID` 分岐）が `FakeAdapter::new(argv)` を作る。`FakeAdapter`（`crates/task-worker/src/fake.rs`）は `run_subprocess`（`task-worker/src/subprocess.rs`）で実 process を起こす。stdin には `RunRequest` の JSON が入る（`protocol.rs:1025`: `task`・`workspace`・`work_dir`・`artifacts_dir`・`context`・`llm_route`）。stdout は WorkerMessage の JSON 行（`progress`/`done`/`error{retryable}`/`question`/`yielded`…）。`with_model` は自分を返すので、route のモデルが何であっても実行されるのはスタブだけである。
- **実 daemon の起動例**: `crates/celeris/tests/cap_probe_real_daemon.rs` は実 `celeris` binary（`CARGO_BIN_EXE_celeris`）を `env_clear` した環境で起動する（一時 DB・loopback・偽 harness だけ）。worker は `[adapters.fake] command` で起こす。投入は `CurlTransport` を使った実 HTTP。**これが `ablation_application.rs` の雛形になる**。
- **PID・終了**: daemon は worker の PID を events/runs に**記録しない**（`Event::WorkerStarted{run_id, adapter, model, provider, account, role, task_role}`、`WorkerFinished{outcome, metrics}`。pid の欄はない）。subprocess は `child.id()` を `ProcessGroup::register(run_id, pid)` に渡すだけ（`subprocess.rs:155`）で、event には書かない。したがって「PID と run id の対応」は、スタブ自身に `$$`・`$PPID` と stdin の `RunRequest`（`task.id`、`artifacts_dir` は run ごとに異なる）を、fixture が所有するログ（1 run 1 行、追記）へ書かせて取る。それを `WorkerStarted.run_id`/`WorkerFinished` と突き合わせる。`$PPID` が実 daemon の PID（fixture が `Child::id()` で持つ値）に一致すること、run の終了後に `/proc/<pid>` が消えていることで、process の実在と終了を確かめる。daemon 本体は変えない。
- **計画出力の採用**: `crates/task-dispatch/src/dispatcher/planner_flow.rs::on_planner_finished` は、planner run（`RunRequest.context.execution_planner` が Some）が `done` で終わったとき、`<artifacts_dir>/execution-plan.json` を検証して `task_ops::execution::adopt_plan_with_children` で採用する。不正なら 1 回だけ再試行し、だめなら atomic に倒す。kind `task` の unit は子 task になる（`[execution.tree] enabled = true`、`max_depth` は既定 3。root=1・子=2・孫=3、`task-core/src/tree.rs`）。スタブは stdin の `context.execution_planner` を見て plan を書けばよい（in-process での同じ形は `task-dispatch/src/dispatcher/tests/tree.rs:39-43`。こちらは偽 adapter なので、証拠としては使わず形の参照だけにする）。atomic な coding task は planner なしの直行経路（`dispatch_run.rs::direct_route_context`）に乗るので、子・孫を作らせる task は直行経路の条件に当てはまらない形にする（あるいは計画が要る genre/role を使う）。planner の harness は `[harness_selection.execution_class_defaults] planner = ["<stub>"]` で選ぶ（`[execution.planner] adapter` は読み込みエラーになる。`config/mod.rs:443-448`）。
- **retry**: スタブが `{"type":"error","retryable":true}` を返すと `Terminal::Error{retryable}` になり、`max_attempts` の範囲で再起動される。どの run で失敗させるかはスタブが決定的に選ぶ（例: fixture 所有の印 file がなければ失敗して印を作る）。retry では assignee が保持される（`decide` が assignee 付きを `NotApplicable` にする）。`variant_run_pin` は run ごとに再検査する。
- **委譲**: `delegate.json` も子を作れる（`task-worker/src/delegate_file.rs`）が、objective が求めているのは「計画出力の通常採用」なので、主経路は `execution-plan.json` とする。
- **実 LLM の排除**: config の harness/source をすべて `kind = "fake"` にし、`claude`/`codex`/`opencode`/`anthropic` の語が config にないことを assert する（`variant_propagation.rs` に前例がある）。`[harnesses.*]` の `env_of` は使わない。

### 3. campaign crate の dev-dependency と循環（`cargo tree`）
- `cargo tree -p celeris-campaign -e normal,dev --depth 1 --offline`: normal は `routing-eval` `task-core` `task-worker` ほか。dev は `task-api` `task-ops` `axum` `futures-util` `rusqlite` `time` `tokio` `tower`。
- `cargo tree -i celeris-campaign -e normal,dev --offline`: 逆依存は **`celeris` の dev-dependency だけ**（その上に `celerisctl`）。normal 依存として campaign を使う crate はない。
- `cargo tree -p task-dispatch -e normal --offline`: campaign・celeris・task-api・routing-eval のどれも含まない。→ **`task-dispatch` を campaign の dev-dependency に足しても循環しない**。
- `celeris` を campaign の dev-dependency に足すと、`celeris`↔`campaign` が互いに dev-dependency になる。normal 依存の循環ではないので cargo は解決できるが、試験 binary の中に campaign が 2 系統入り、型が別物になる。そのうえ campaign の試験からは `CARGO_BIN_EXE_celeris` が見えない（同じ package の bin しか設定されない）。**推奨: celeris は dependency に足さない**。代わりに実 `celeris` binary を process として起動する。場所は `CARGO_BIN_EXE_celeris-campaign` と同じ dir の `celeris` とし、無ければ `cargo build -p celeris --bin celeris`（`cap_probe_real_daemon.rs:38-60` の逆向き）。実 process を使うので、in-process の偽 Transport も DB 直挿入も要らない。HTTP は既存の `celeris_campaign::curl_transport::CurlTransport` を使えば、追加の dev-dependency はゼロで済む。task-dispatch の公開 API が要るのは、`set_variant_observation` を直接呼ぶ場合（in-process）だけで、それは実 daemon を使う本 task の証拠にはならない。

### 4. daemon 本体の観測値配線（`set_variant_observation`）
- `grep -rn set_variant_observation crates`: 定義は `crates/task-dispatch/src/dispatcher.rs:1761`。呼び出しは試験 2 か所だけ（`crates/celeris/tests/variant_propagation.rs:315`、`crates/task-dispatch/src/dispatcher/tests/variant_pin.rs:336`）。**`crates/celeris/src` に呼び出しはない**。`normal-e2e.md` の未解決事項はまだ残っている。
- 影響: 実 daemon では、variant 付き task の run は常に「daemon instance id / config sha256 are not available」で起動されない（fail closed）。このままでは実 process による適用検証ができないので、葉 `daemon-obs` が前提になる。
- 配線点（daemon-obs 向けの事実）: `crates/celeris/src/daemon/run.rs:58` で `let identity = InstanceIdentity::new(..)`（`identity.instance_id`。`GET /health` と同じ値）を作り、`run.rs:107` で `build_dispatcher(&config, ..)` を呼ぶ。config の sha256 は `Config.source_sha256`（`config/mod.rs:285-287,495`。`Config::load` が読んだ bytes の sha256。`GET /config` の `identity.config_sha256` と同じ値、`daemon/api.rs:150`）。107 行目の直後に `dispatcher.set_variant_observation(identity.instance_id.clone(), sha)` を置けばよい。`source_sha256` が None なら観測値を設定せず、fail closed のままにする。config の reload（`reload_providers`）で sha が変わる場合の扱いは daemon-obs で決める。
- 適用試験はこの値を GET `/health`・`/config` から読み、run 証拠（`CampaignRunPin.daemon_instance_id`・`config_sha256`）と突き合わせる。

### 5. 後続の葉への引継ぎ（要点）
- infra: 実 `celeris` binary（`env_clear`・一時 DB/token/org/workspace/HOME・loopback・fake だけ）。スタブは sh か `celeris-campaign` の subcommand で、PID・run の対応をログに書く。同期は events を GET で読む待ち＋期限で、固定 sleep は使わない。終了時に daemon と子 process を kill・wait して回収する。userns は要らない（ADR-0126）。
- scenario: off/on を別の daemon・DB で、同じ B0 資料・非対象設定で回す。off は全子孫の `Assigned` が単一 node、他 node への割当は 0、retry の assignee/hash は維持。on は複数課への割当と、その差を見る。
- negative: 投入・再開・冪等照会、改変・欠落の拒否は、既存の `CelerisExecutor` を実 daemon に向けて使う。K2 は not_judged のまま。

## daemon観測配線（WU daemon-obs、2026-10-19）

- 変更: `crates/celeris/src/daemon/bootstrap.rs` に `build_daemon_dispatcher(config, masters, identity)` を足し、`lib.rs` から公開した。中身は `build_dispatcher` の後に `Dispatcher::set_variant_observation(identity.instance_id, config.source_sha256)` を呼ぶだけで、新しい機構は作っていない。`crates/celeris/src/daemon/run.rs` の起動経路（旧 107 行目の `build_dispatcher`）はこれに置き換えた。instance id は `GET /health`、sha256 は `GET /config` の `identity.config_sha256` と同じ値（同じ `InstanceIdentity`・`Config.source_sha256` を使う）。
- fail closed: `source_sha256` が `None`（設定がファイル由来でない）なら観測値は渡さず warn を出す。variant 付き run は従来どおり「daemon instance id / config sha256 are not available」で blocked になる。
- reload: `reload_providers` は `source_sha256` を書き換えないので、観測値も `GET /config` も起動時の値のまま一致する。変えた設定の sha を run に記録したいなら再起動する（reload で観測値を差し替える仕組みは作らない）。
- 試験（`crates/celeris/tests/variant_propagation.rs`、6 件）:
  - 新 `daemon_build_path_pins_observed_identity`: 起動経路の `build_daemon_dispatcher` で tick を回す。`[harnesses.fake] command = "sh"` のスタブ（実 process）が stdin の `RunRequest` を一時 dir に書く。その `context.campaign.daemon_instance_id` が `GET /health` の instance id に、`config_sha256` が `GET /config` の値と設定ファイル bytes の sha256（試験側で別に計算）に一致することを確かめる。同期は `WorkerFinished` event 待ち＋期限 60 秒。
  - `normal_path`（spec-off/on）・org drift・foreign assignee の各試験は、手で与えていた `set_variant_observation("inst-1", …)` と素の `build_dispatcher` をやめ、`build_daemon_dispatcher` を使う。API の instance id も固定値ではなく、同じ `InstanceIdentity` の値にした。
  - `missing_daemon_observation_blocks_the_run`: sha256 の無い config で `build_daemon_dispatcher` を組み、`WorkerStarted` が無く「not available」の理由 event が出ることを確かめる（fail closed は残っている）。
- 証拠: `cargo test -p celeris --test variant_propagation` → 6 passed / 0 failed。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。`cargo test -p celeris --no-run` → exit 0。
- 実 `celeris` binary による確認は、葉 infra・scenario が `GET /health`・`/config` と run 証拠を突き合わせて行う。

## 未解決事項
- （解消: WU daemon-obs）daemon は起動経路の `build_daemon_dispatcher` で `set_variant_observation` を呼ぶ。
- task branch が main（`cb0cd7b8`）に追従していない。merge では 16 file が衝突する（§0）。final review の前に main を取り込む必要がある（本 WU では merge しない）。
- worker の PID は events/runs にない。証拠はスタブ側のログと daemon の `Child::id()` の突き合わせで作る（本体に pid 欄を足すかどうかは提案）。

## 提案
- `WorkerStarted` に任意欄 `pid` を足すと、process と run の対応を daemon 側の記録だけで示せる。ただし本 task の範囲（新しい仕組みは作らない）を超えるので、別 task とする。
