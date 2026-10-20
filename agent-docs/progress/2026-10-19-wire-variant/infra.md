---
task: 01M5B84ZY6H4001ZYPSGN9VR34
tasks: [01M5B84ZY6H4001ZYPSGN9VR34]
title: ablation_application の無 LLM 所有 fixture（infra）
work_unit: infra
status: done
updated: 2026-10-20
---
# infra: ablation_application の所有 fixture と smoke

記録の本体は同じ dir の `application.md`（record 葉で統合する）。この WU は試験 infra だけ。

## 変更
- `crates/campaign/tests/ablation_application_support/mod.rs`（新規）: `Fixture` / `FixtureSpec` / `StubSpec` / `Http`。
  - 所有物: 一時 dir（`celeris.sqlite3`・`api.token`（fixture ごとに別値）・`org.toml`（`org_include` の種＝通常の起動経路で空 DB に蒔く）・`ws`・`home`（HOME/TMPDIR）・スタブ状態）、loopback の port、実 `celeris` binary の daemon process（`env_clear`、PATH は罠 dir + `/usr/bin:/bin`）。
  - `Drop`: daemon に TERM → 期限で KILL・wait、記録したスタブの process group を（生存かつ cmdline がスタブなら）KILL、TempDir を消す。
  - 実 `celeris` は `CARGO_BIN_EXE_celeris-campaign` と同じ dir の `celeris`（無ければ `cargo build -p celeris --bin celeris`、失敗は panic）。celeris crate に依存しない。dev-dependency の追加はゼロ（Cargo.toml 不変）。
  - HTTP は試験内の最小 HTTP/1.1 client（Bearer。試験 process の環境変数を書き換えない。`celeris_exec::Transport` を実装するので `CelerisExecutor` も向けられる）。
  - スタブ: この試験 binary 自身。`.init_array` の入口が `--ablation-stub <dir>` のときだけ libtest の `main` より前に動いて終わる（libtest の出力を worker protocol の stdout に混ぜない）。stdin の `RunRequest` を読み、PID・親 PID・`CELERIS_RUN_ID`・`CELERIS_TASK_ID`・title・planner 有無を `launches.jsonl` に追記。`spec.json` の `plans`（title → `execution-plan.json`、planner run のときだけ）と `fail`（title ごとに最初の n 回 `retryable` error、回数は状態 file）で決定的に振る舞う。
  - LLM 実行先: config の source/harness は全部 `kind = "fake"`（`assert_config_is_stub_only`）。daemon の PATH 先頭に `claude`/`codex`/`opencode`/`gemini`/`aider` の罠（起動で `unexpected.log` に残し exit 97）。`assert_only_stub_launches` は罠の記録ゼロ・全 `worker_started.adapter == "fake"`・`worker_started` の run id 集合とスタブ起動の run id 集合の一致を見る。
  - 同期: `GET /api/v1/events?after_id=` を読み足し、述語が成り立つまで期限付きで待つ（`wait_events`）。固定時間の sleep で順序を作らない（50ms は再読みの間隔）。
- `crates/campaign/tests/ablation_application.rs`（新規）: smoke 2 件。
  - `smoke_post_spawns_real_stub_worker_and_events_match_its_pid_and_run`: 通常 POST → 実 dispatcher → 実 fake adapter → スタブ process。スタブの親 PID = daemon PID、`RunRequest.task.id` = 投入 task、スタブ終了後 `/proc/<pid>` なし、`CELERIS_RUN_ID` = `worker_started`/`worker_finished` の run_id、`GET /tasks/{id}/runs` に同じ run。drop 後に daemon・スタブ PID 消滅、port 拒否、一時 dir 消滅。
  - `trapped_llm_cli_launch_fails_the_fixture_check`: 罠の CLI を起動すると `assert_only_stub_launches` が panic する。

## 証拠
- `cargo test -p celeris-campaign --test ablation_application` → 2 passed（0.7s、3 回繰り返して同じ）
- `cargo nextest run -p celeris-campaign --test ablation_application` → 2 passed
- `bash scripts/dev/test-parallel.sh` → exit 0、5915 passed / 14 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo clippy -p celeris-campaign --test ablation_application -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決・注意（後続の葉へ）
- variant 付き task の run は daemon-obs（観測値配線）の統合後でないと fail closed のまま。smoke は variant なしの task。
- 子・孫を作る task は直行経路（atomic coding）に乗らない形にする必要がある（recheck §2）。スタブの `plans` は title で引く。
- org の matching には課の `profile.harnesses.allowed` に harness id（`coding`）が要る（`FixtureSpec::minimal` 参照）。
- `cargo clippy -p celeris-campaign --tests` は既存の `env_lock.rs`・`launcher.rs` の lint で落ちる（本 WU の範囲外。workspace gate は `--tests` なしで通る）。
- daemon は container runtime（docker）を自動検出するが、fake の run は container を使わない。
