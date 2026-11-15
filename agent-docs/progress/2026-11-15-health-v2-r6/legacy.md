---
title: "健全性v2 R6 legacy: celerisctl plan の旧い入口を消し、backup の計測の消去を記録する"
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R6 legacy

完了日: 2026-11-15

R5 の `agent-docs/progress/2026-11-10-health-v2-r5/candidates/old-impl.json` の 2 件を扱った。

## 結論

- `old-celerisctl-plan-legacy-path`: 消した。決定 r6-plan-e2e の答え a に従い、`celerisctl plan` と
  `task_ops::plan::create_plan`（と `truncate_title`・`TITLE_MAX_CHARS`・その試験）を消した。
  `tests/e2e/tests/plan_scenarios.rs` の 2 つの e2e は、Plan task の作成だけを `env.store.create_task` に替えた。
- `old-incremental-backup-measurement`: R5 で消えている。この unit はコードを変えていない。
  残る試験は `vacuum_backup_finishes_with_a_concurrent_writer_and_integrity_ok` ほか 2 件（下の節）。

## 決定 r6-plan-e2e の答え

- 答え: a（推奨どおり）。R6 の write set に `^tests/e2e/tests/plan_scenarios\.rs$` を足し、e2e の 2 つの Plan task の作成を
  `env.store.create_task` に替え、`celerisctl plan` と `task_ops::plan::create_plan` を消す。
- 答えたのは運用者で、人ではない。人が 2026-11-15 に運用者の Claude Code の対話 session で「R6が止まっているのにも
  対処してください」と指示し、運用者がそれを受けて a を選んだ。
- write set の変更は e2e-scope unit が行った（`e2e-scope.md`）。この unit の提出時の HEAD で
  `printf 'tests/e2e/tests/plan_scenarios.rs\n' | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R6 …/slices.json`
  → `write-set R6: ok`（exit 0）。

## 前の試行からの引き継ぎ

前の試行（3 run）の進捗は退避 branch `celeris-wu/01M6TYVJT7EJ6WWHEF0EBH3JPS/legacy-superseded-*`（末尾は旧い HEAD の 12 桁）の
`agent-docs/progress/2026-11-15-health-v2-r6/legacy.md` にある（`git show` で読んだ）。そこで分かったこと:

- 1 回目の run: `celerisctl plan` を実 binary で呼ぶ e2e が 2 件あると分かり、決定を返した（unit に決定は登録されなかった）。
- 2 回目の run: check 3（撤去）と範囲・write set（check 4・5）が同時に満たせないことを plan_issue にした。
  result.json に `summary` と `yield` を併記したため、celeris は done と読み、plan_issue は捨てられた
  （`crates/task-worker/src/result_report.rs` の `interpret` は `question` → `summary` → `yield` の順に見る）。
- 3 回目の run: `yield` だけで plan_issue を申告し直した。replan で e2e-scope unit と決定 r6-plan-e2e が足された。
- 1 回目の run の「`celerisctl add --kind plan` で同じ task を作れる」は誤り。`task_ops::add::build_task_with_roles` は
  `require_acceptance = true` で `build_acceptance` を呼び、受け入れ条件 0 件を拒む。e2e は `acceptance` が空であることを
  assert するので、試験の中の `store.create_task` を使った。

## 実装前 brief

1. 根本原因: ADR-0079 で `POST /plans` は 410 になったが、同じ仕事（DB に直接 `kind = plan` の根の task を作る）をする
   `celerisctl plan` が残っていた。旧い入口の 2 つの経路のうち片方だけが消えていた。
2. 責務 owner: architecture-map の celerisctl の行（`crates/celerisctl/src/commands/plan.rs`、`main.rs` の `Command::Plan`）と
   task-ops の `crates/task-ops/src/plan.rs`（`create_plan`）。
3. 既存の再利用先: 新しい仕事の入口は root task と `celerisctl execution plan set`（plan/3）。e2e の `Env` は
   `SqliteStore`（`env.store`）を持ち、`TaskStore::create_task` が insert と `Created` を 1 トランザクションで書く
   （`create_plan` と同じ書き込み）。`Task` の literal は `tests/e2e/tests/scenarios.rs` の `add_ready_task` と同じ形。
4. 選択肢: (a) e2e の作成を `store.create_task` に替えて command と `create_plan` を消す（採用。決定 a）。
   (b) command を残す。(c) e2e 2 件ごと消す（planner の経路の e2e が無くなるので採らない）。
5. 新しい状態・分岐・fallback・port: 無し。e2e に試験の helper `Env::create_plan_task` を 1 つ足した（試験の中だけ）。
6. 撤去対象: `crates/celerisctl/src/commands/plan.rs` 全体（`PlanArgs`・`run`・`ADR_0079_NOTE`・2 試験）、`main.rs` の
   `Command::Plan` と import、`commands/mod.rs` の `pub mod plan`、`task_ops::plan::create_plan`・`truncate_title`・
   `TITLE_MAX_CHARS`、`crates/task-ops/src/plan/tests.rs`（5 試験）、docs の `celerisctl plan` の説明。
7. 不変条件: `kind = plan` の task の planner の経路（人の承認 → planner run → `plan.json` の検査 → 子の draft 挿入、
   不正な plan の 1 回の retry と `prior_review`）は e2e の 2 件が守る。`cargo nextest run -p e2e --test plan_scenarios` で確かめた。

## 根拠の検索（提出時の HEAD で実行）

- 利用の検索（前の試行の検索を提出時の HEAD でやり直した）:
  `rg -n 'celerisctl plan\b|celerisctl plan"|"celerisctl", "plan"|celerisctl\(&\["plan"' crates tests scripts web/api`
  → 残りは `web/api/generated/schema.json:21195` と `crates/task-ops/src/plan.rs` の `NewPlanSpec` の説明文と注釈だけ。
  scripts・`web/src`・`web/e2e` に呼び手は無い（前の試行と同じ。`scripts/dev/general-cos-compare/test_compare.py` の
  `cli("plan", …)` は別の script の subcommand）。
- 呼び手の検索: `rg -n 'task_ops::plan|plan::create_plan|plan::NewPlanSpec' crates tests`
  → 変更前の呼び手は `crates/celerisctl/src/commands/plan.rs`（`create_plan`・`NewPlanSpec`）と
  `crates/task-api/src/schema.rs:19`（`NewPlanSpec`）だけ。`task-api` の `handlers/system.rs` の `create_plan` は 410 を返す
  別の関数で、`task_ops` を呼ばない。
- 変更後の検索: `rg -n 'PlanArgs|commands::plan\b|plan::run|Command::Plan\(' crates/celerisctl/src` → `ExecutionCommand::Plan`
  （`execution plan set|replan`）だけ。
- docs: `rg -n 'celerisctl plan\b' docs --glob '*.md'` → 2 件。どちらも「R6 で撤去」と書いた履歴の記述（下の表）。

## 項目ごとの判定

| 項目 | 判定 | 根拠・残す試験 |
|---|---|---|
| `celerisctl plan`（`commands/plan.rs`・`Command::Plan`・`pub mod plan`） | 消した | 決定 a。呼び手は e2e 2 件だけで、作成を `store.create_task` に替えた |
| `ADR_0079_NOTE` | 消した | command の本体の注記。command と一緒に消えた |
| `task_ops::plan::create_plan`・`truncate_title`・`TITLE_MAX_CHARS` | 消した | 呼び手は `celerisctl plan` だけだった |
| `crates/task-ops/src/plan/tests.rs`（5 試験） | 消した | すべて `create_plan` の試験。下の「消した試験」 |
| `NewPlanSpec`（と既定値の 4 関数） | 残した | `task-api` の `schema.rs` と生成 API schema が使う。提案 1 |
| `NewPlanSpec` の `///` の説明文 | 残した（文言を変えていない） | 生成 schema の description に写り、schema の JSON はこの unit の範囲の外。提案 1 |
| e2e `plan_scenarios.rs` の 2 試験 | 寄せた（作成の 1 か所だけ替えた） | 下の「替えた試験」 |
| `docs/protocol/worker-protocol.md:733` | 直した | 「`celerisctl plan` が作る Plan」→「受け入れ条件の無い Plan〈R6 で撤去した `celerisctl plan` が作った行〉」 |
| `docs/api/v1/api.md` §5.6（4202・4204 行目） | 直した | 見出しと本文から `create_plan`・`plan` を除き、`add` だけにした。参照先 §3.14（410）を外した |
| `docs/api/v1/api.md:384`（`POST /plans` の表） | 残した（注記を足した） | 撤去済みの経路の履歴。「`celerisctl plan` も R6 で撤去」を足した |
| `old-incremental-backup-measurement` | 消した（R5 で消えている。この unit は変えていない） | 下の節 |

### 消した試験と、廃止した契約・残る試験

- `crates/celerisctl/src/commands/plan.rs`:
  - `run_inserts_plan_task`（CLI が Plan task を 1 件 draft で作る）→ 契約ごと廃止（CLI から Plan を作る入口が無い）。
    `kind = plan`・draft・acceptance 空・Frontier・`max_retries = 1` の Plan task からの planner の経路は e2e 2 件が守る。
  - `run_with_blank_goal_returns_error`（空の goal を拒む）→ 契約ごと廃止。task の空の title・objective を拒む検証は
    `task_ops::add::create_task` が持ち、`crates/task-ops/src/add/tests.rs` の
    `create_task_rejects_blank_title_or_objective_and_missing_parent` が守る。
- `crates/task-ops/src/plan/tests.rs`（`create_plan` ごと廃止）:
  - `create_plan_inserts_plan_task_and_created_event` → `create_task` が insert と `Created` を 1 トランザクションで書くことは
    store の試験と、e2e の `replay_is_consistent`（`celerisctl replay` が 0 mismatches）が守る。
  - `create_plan_truncates_multiline_goal_title_but_keeps_full_objective`・`create_plan_with_blank_goal_returns_error`・
    `create_plan_with_custom_tier_priority_and_retries`・`create_plan_without_workspace_defaults_to_relative_task_id_path`
    → goal から title を作る規則・既定値・workspace の既定は `create_plan` 固有の契約で、関数ごと廃止。
- `POST /plans` が 410 を返すことは `crates/task-api/tests/cos_delivery_suite_c/operations.rs` の `create_plan_is_gone` が守る（変えていない）。
- 新しい仕事の入口（plan/3）は `crates/celerisctl/src/commands/execution_tests.rs` の
  `set_adopts_a_v3_plan_with_the_configured_tree_limits`・`set_rejects_an_invalid_plan` が守る（変えていない）。

### 替えた試験（`tests/e2e/tests/plan_scenarios.rs`）

- `celerisctl_plan_generates_children_that_complete_after_human_approval` → 名前を
  `plan_task_generates_children_that_complete_after_human_approval` に変えた（作成が `celerisctl plan` でなくなったため）。
- `invalid_plan_is_retried_once_with_prior_review`（名前は同じ）。
- 替えたのは Plan task の作成だけ。新しい helper `Env::create_plan_task(goal, dir)` が `kind = Plan`・`Status::Draft`・
  `acceptance = []`・`Tier::Frontier`・`max_turns = 30`・`max_wall_secs = 900`・`max_retries = 1`・workspace = `dir` の
  根の task を `env.store.create_task(&task, vec![])` で作る（消した `create_plan` の既定値と同じ）。
- 1 つ目の試験の kind・draft・objective・acceptance 空・Frontier・`max_retries = 1` の assert、2 つの試験の
  `celerisctl approve`（実 binary）、planner の run、遷移の列、`ReviewVerdict`、子の draft 挿入と承認後の done、retry と
  `prior_review`、`replay` の assert は変えていない。
- 廃止した契約: 「`celerisctl plan` の stdout が task id だけ」（`.trim().parse()`）。command ごと消えた。

### old-incremental-backup-measurement

- `rg -n incremental_backup_progress_restarts_after_other_connection_writes crates tests scripts docs` → 0 件（exit 1）。
- `git log -S incremental_backup_progress_restarts_after_other_connection_writes -- crates/celeris` → 消した commit は件名
  「phase 1: celeris の試験の helper の写しを tests/support に寄せ、旧い backup 方式の計測試験を消す」（R5）、足した commit は
  件名「定期DBバックアップの再開始ループと中断遅延を解消」。どちらも main の祖先（`git merge-base --is-ancestor` が 0）。
- 廃止した契約: 旧方式（rusqlite の online backup を 1 page ずつ step する）が別接続の書き込みで最初からやり直すこと。
  この試験は rusqlite の旧 API を計測するだけで、celeris の関数を呼ばなかった。`backup_once` は `VACUUM INTO` に置き換え済み。
- 残る試験（`crates/celeris/src/db_maintenance/tests.rs`。この unit は変えていない）:
  - `vacuum_backup_finishes_with_a_concurrent_writer_and_integrity_ok`（17 行目）: VACUUM の progress callback の中で別接続が
    commit しても、backup が終わり integrity_check が ok になる。
  - `vacuum_progress_handler_interrupts_an_active_copy`（104 行目）: progress handler が実行中の copy を中断し、`.partial` が残らない。
  - `backup_once_writes_a_restorable_copy_and_keeps_recent_generations`（133 行目）: 復元できる写しを作り、世代を保つ。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p e2e --test plan_scenarios` → 2 tests run: 2 passed（2.4 秒。`cargo build -p celerisctl -p celeris` の後）。
- `bash scripts/dev/test-affected.sh` → exit 0（853 秒。1 つ目の commit の後に実行。2 つ目の commit は `plan.rs` の `//` 注釈 1 行だけで、再実行していない）。対象は celeris・celeris-campaign・celeris-mcp・celerisctl・e2e・
  llm-proxy・task-api・task-dispatch・task-ops・task-worker と web の typecheck+unit。nextest は 6946 tests run: 6946 passed、
  23 skipped。web の unit は 103 files・714 tests passed。repo_health_gate は all checks passed。
  この run で health は new=1 を出した（`migration.remnant`: `crates/task-ops/src/plan.rs` の注釈の語 legacy。進捗 file 名を
  書いていた）。注釈を直し、`bash scripts/dev/check-health.sh` → verdict=pass、new 0・worsened 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- unit の check 3（撤去）→ exit 0。check 4（凍結対象の差分 0 と `slices-check.py --write-set R6`）→ exit 0、`write-set R6: ok`。
  check 5（範囲）→ exit 0、範囲外 0。
- `sh scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/progress-index.sh --check` → ok（exit 0）。

## commit（件名）

- 「health-v2 R6 legacy: celerisctl plan と task_ops::plan::create_plan を消し、e2e の Plan task は試験の中で作る」
- 「health-v2 R6 legacy: NewPlanSpec の注釈から移行の残骸の語を除く」
- 「health-v2 R6 legacy: 進捗に決定 r6-plan-e2e の答えと撤去の判定・証拠を書く」

## 未解決

- 無し。

## 提案

1. `NewPlanSpec` は撤去済みの `POST /plans`（410）の本文の型で、`task-api` の `schema.rs` の `new_plan` と生成 API schema
   （`docs/api/v1/api-v1.schema.json`・`web/api/generated/schema.json`・`types.ts`）だけが使う。説明文は今も「`celerisctl plan` から
   組み立てる」と書く。破壊的変更の方針（ADR 2026-10-11）に従い、同じ節の型（`ProjectPlanBody` など）と合わせて API schema から
   消す task を立てることを提案する（close が判断する）。この unit の範囲は生成 schema の JSON を含まないので扱わなかった。
2. R5 の old_impl の走査は members を unit 試験から引き、`tests/e2e` の実 binary の利用を拾わなかった。CLI の command を消す候補では
   `tests/e2e` の `celerisctl(&["<cmd>", …])` も検索に入れ、計画の段で write set と unit の範囲に e2e の file を入れる。
3. worker の result.json で `summary` と `yield.plan_issue` を併記すると、`interpret` は done と読み、plan_issue を黙って捨てる
   （前の試行の 2 回目の run で起きた）。WorkUnit の prompt の plan_issue の指示に「`summary` を書かない」と明記するか、
   `interpret` が `yield.plan_issue` を `summary` より先に見るようにする task を R6 の外に立てることを提案する。
