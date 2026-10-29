---
task: 01M62V0FQMMYQ1S83GBXQP7A6J
unit: measure-probe
status: done
completed: 2026-10-29
---
# measure-probe: daemon を変えずに stub harness で CoS 経路を通せるか

結論: daemon（release 94f86928dc13 と同じ HEAD の crates/celeris・crates/task-*）を変えずに、config だけで CoS 経路を stub で通せる。コードは書いていない（読解のみ）。基準は `$CELERIS_WU_BASE`。

## (1) CoS run の起動経路と stub への差し替え点
- `[cos]` の欄は `enabled`・`route_candidates`・`worker_reserve_five_hour`・`harness`・`max_turns`・`max_wall_secs`・`[cos.triage]`・`stream_retention_days`・`[cos.attachments]`（`deny_unknown_fields`）。crates/celeris/src/config/cos.rs:14-37
- harness は claude_code 固定。`[cos] harness` が claude-code 以外なら読み込みエラー。crates/celeris/src/config/cos.rs:356-364、起動設定の harness も "claude-code" 固定 crates/celeris/src/daemon/cos_launch.rs:21
- route の選択: CoS は harness id `"claude_code"` を要求し `cos: true` で選ぶ。crates/task-dispatch/src/dispatcher/cos_chat/routes.rs:175-184。候補は `[cos] route_candidates` だけ（他の route_defaults に落ちない）。crates/task-core/src/llm_route.rs:487-496、registry への写し crates/celeris/src/config/llm_route.rs:321-328。候補の source kind が harness の `source_kinds` に合わないと選ばれない（通常の route_select の判定）。
- 「Claude 供給元」の制約はコードの検査ではなく、実測定の config で `[llm_sources.*] kind = "claude_oauth"` を CoS の候補に置くことで満たす。daemon は CoS の source kind を検査しない（上の cos.rs:356-364 は harness だけを見る）。
- adapter: `route_adapter` は `harness_adapters[pick.harness]` を使う。crates/task-dispatch/src/dispatcher/route_launch.rs:95-120。したがって `[harnesses.claude_code] kind = "fake"` + `[adapters.fake] command = [...]`（または `[harnesses.claude_code] command/args`、旧 daemon.toml の `cap-probe-claude-code` と同じ形）で、harness id を `claude_code` のまま stub に差し替えられる。前例 crates/celeris/tests/root_done_summary.rs:116-139（`[cos] route_candidates = [{source="local",model="model-1"}]`、`[llm_sources.local] kind="fake"`、`[harnesses.claude_code] kind="fake"`）。
- 実測定（有料）の config では `[harnesses.claude_code] kind = "claude_code"` と `[adapters.claude_code] command = <claude の path>` に戻す。stub と実測定の差は harness・source・adapter の 3 節だけにする。
- 注意: 旧 final-freeze-2 の harness id は `claude-code`（ハイフン）。CoS は `claude_code`（下線）を要求するので、id を `claude_code` に改める。

## (2) stub の CoS run が起票し、報告が thread に残る仕組み
- run credential の env: `CELERIS_COS_RUN_CREDENTIAL`（bearer）、`CELERIS_COS_POLICY_VERSION`、`CELERIS_API_URL`（`[api] listen` から `http://127.0.0.1:<port>/api/v1`）、PATH の先頭に daemon と同じ dir（celerisctl）、TMPDIR。crates/task-dispatch/src/dispatcher/cos_chat/launch.rs:37-67、発行 launch.rs:654-673。fake adapter は `with_env` で env を受ける crates/task-worker/src/fake.rs:91-95。
- 起票: credential は domain 変更を `/api/v1/cos/operations` 経由でしか許さない crates/task-api/src/cos/mod.rs:126-140。route crates/task-api/src/cos/operations.rs:51。body は `{idempotency_key, expected_revision?, reason, policy_version, request:{method:"POST", path:"/api/v1/tasks", body:<POST /tasks の body>}, instructed_by?, basis?}` operations.rs:361-390。`task.create` は通常の `create_task_op` を監査付きで呼ぶ crates/task-api/src/cos/ops/tasks.rs:13,108-118。`policy_version` は run に渡した版と一致しないと 422 `cos_policy_version_mismatch` operations.rs:452-474。stub は curl でも celerisctl（CoS 経路は自動で `/cos/operations`、crates/celerisctl/src/main.rs:516-517）でもよい。案件 thread への報告を得るには POST /tasks body に試行案件の `project_id` を入れる。
- CoS 自身の返事: run の `Done{summary}` が output message（assistant）の本文になる。crates/task-dispatch/src/dispatcher/cos_chat/sink.rs:94-100、crates/task-core/src/chat/run_store.rs:687-707 と finish_conn（`UPDATE chat_messages SET text=...`、run_store.rs:894-898 付近）。
- 完了の報告: root task が done になると、daemon の root_done tick が task の root 案件の CoS thread（`cos_thread_for_task`）に system 入力を積み、CoS run の返事が thread に残る。crates/task-dispatch/src/dispatcher/cos_chat/root_done.rs:97-110,240-303、crates/task-core/src/chat/store.rs:686-700。fake での前例は root_done_summary.rs（CoS run 1 回・thread への書き込み 1 件を assert、root_done_summary.rs:355-395）。
- stub の分岐: 同じ stub が「人の message → 起票」と「完了の要約」と worker/reviewer run の 3 役を受ける。stdin の RunRequest（input の本文）で分岐するか、worker 用は別の fake harness id にして `[harness_selection] rules` で振り分ける（旧 daemon.toml の cap-probe と同じ形）。

## (3) 試行専用の案件・thread、skill、policy、route、並列・試行回数、health
- 案件: `POST /api/v1/projects`（人の token）。案件 CoS thread: `POST /api/v1/chat/threads {project_id}` で初回に作られる。crates/task-api/src/chat/mod.rs:57、crates/task-core/src/chat/store.rs:671-683（`cos_thread_open`）。人の message: `POST /api/v1/chat/threads/{t}/messages` chat/mod.rs:62-65。seed（org.toml 等）は使わず、試験・runbook とも API で作る。
- CoS skill: CoS run は `[knowledge] root` の `skills/<name>/SKILL.md` を読み、policy key に含める。launch.rs:640-650（`policy_key(policy_version, skills)`）。前例 root_done_summary.rs:85-94 は `knowledge/skills/{cos-operator,cos-inbox-triage}` を置く。設定生成 script は repo の `config/skills/cos-operator`・`config/skills/cos-inbox-triage` を測定 state の `kb/skills/` に写す。`[cos.triage] policy_skill` の既定は `cos-inbox-triage` cos.rs:143-145。
- policy_version: `[cos.triage] policy_version = "6"`（既定も "6"、cos.rs:146-148）。生成 script は既定に頼らず明示する。
- route: CoS = `[cos] route_candidates = [{source=<claude 供給元>, model="claude-opus-5-5"}]`。planner/worker/reviewer = `[route_defaults.claude_code] candidates = [{..., model="claude-sonnet-5-5"}]`（execution class 別にするなら `[route_defaults.claude_code.classes.<class>]`、llm_route.rs:300-318）。`--models` の option key → model の表は script に置く（親の決定の option）。
- 並列 1: top-level `max_concurrency = 1`（crates/celeris/src/config/mod.rs:124-125）、`[execution] max_parallel_work_units = 1`・`max_parallel_child_tasks = 1`（crates/celeris/src/config/execution.rs:47-48,85-86）、`[accounts] max_runs_per_account = 1`（旧 daemon.toml）。
- 試行 1 回: daemon config に task 全体の `max_attempts` 欄は無い。task の `budget.max_retries = 0`（crates/task-core/src/model.rs:191-198）を起票 body に入れるか、org profile の `budget.max_attempts = 1`（crates/task-core/src/profile.rs:118-125、根→葉の最小が勝つ）を `PATCH /api/v1/org` で試行専用 DB に入れる。stub CoS の起票では前者で足りる。実 LLM の CoS は body を自由に書くので、本測定では後者（org profile）で上限を固定する。いずれも daemon の変更は要らない。
- health: `GET /api/v1/health` の `cos_policy_version` は `config.cos.enabled` のとき `config.cos.triage.policy_version`。crates/celeris/src/daemon/api.rs:364-367、crates/task-api/src/lib.rs:256-257、route crates/task-api/src/handlers.rs:73。

## (4) 本番の state・DB・port・token を使わない担保
- db・`[[api.tokens]] token_file`・`[knowledge] root`・`[scratch] dir`・`workspace_root` をすべて `--state` の下に絶対 path で書く。旧 `[api] token_file` は新 binary が拒否するので `[[api.tokens]]` にする（measure_env.rs:246-255 も同じ形を要求）。
- port: script が 127.0.0.1 の空き port を選び `[api] listen` に書く（本番 7721 等と重ねない）。CoS の `CELERIS_API_URL` はこの listen から作られる（cos_launch.rs:14-17）ので、CoS run も測定 daemon にしか届かない。
- 拒否: script は `--state`・`--out` が `~/.config/celeris`・`~/.local/celeris`・`/local/celeris/state` の下なら非 0 で止める。起動前は既存の `celeris-campaign config-check`（crates/campaign/src/measure_env.rs:108-260、通常運用 config と db・workspace・KB・記憶の重なりを拒否）を流用する。
- worker 側: 子 run は daemon が渡す env で celerisctl が測定 daemon を向く（launch.rs:40-44 の注記）。

## (5) 旧 final-freeze-2 daemon.toml と measure-launcher.sh の再利用
- daemon.toml（`.../main4-ready/bundle/final-freeze-2/pinned/main/daemon.toml`）で再利用できる部分: state 配下の db/kb/scratch/workspace、`[knowledge] default_mounts=["kb:trial"]`・langmem/gc 無効、`max_concurrency=1`・`max_runs_per_account=1`、`[selfdeploy] releases_dir`（読むだけ）、`[[clusters]]`、fake harness を `command`/`args`/`env_of` で置く形、`[harness_selection] rules`。
- 変える部分: `[cos] enabled=false` → true と triage・route_candidates、harness id `claude-code` → `claude_code`、`[api] token_file` → `[[api.tokens]]`、固定 port 7741 → 空き port。
- measure-launcher.sh: `prepare`（binary の sha256 pin）・`start`（`--config` だけで nohup 起動、scripts/campaign/measure-launcher.sh:301-330）・`verify_daemon`（220-239、pin と /proc/<pid>/exe の照合）・`config_check`（242-247）・`stop` はそのまま使える。CoS の preflight（health の cos_policy_version=6 の確認）は launcher に無いので、e2e 試験と runbook で確かめる。

## 未解決事項
- 未確認: stub CoS の起票 body に `budget.max_retries=0` を入れたとき、create_task_op がそれを受けるか（受けない場合は org profile 側で固定する。どちらも daemon の変更は要らない）。measure-config / measure-e2e で確かめる。
- 未確認: `config-check` が `[cos] enabled=true` を拒否しないこと（measure_env.rs に cos の検査は見当たらない）。measure-config で確かめる。

## 検査
- コードは変えていない（`git diff --stat $CELERIS_WU_BASE` は本ファイルだけ）。test-affected・clippy・fmt は変更 crate が無いので対象なし。

verdict: feasible
