---
title: B/C 能力 probe を測定専用 daemon の worker 起動経路へ結び付ける（CLI 環境の代用をやめる）
tasks: [01M57MB8PWVW50KD127M30HEFT]
status: done
updated: 2026-10-18
---
# bc-probe-daemon-env

ADR: `agent-docs/adr/2026-10-18-bc-probe-daemon-env.md`。

## 再現（main 0f9c058642c4）
`WorkerDaemonEnv::from_process` と `Host::worker_daemon_env` の既定が CLI の `std::env` を使っていた。子 process も CLI の環境を継いでいた。
解消済みではない。

## 変更
- `cap_probe.rs`: `WorkerDaemonEnv`・`/proc` 読み・`FakeAdapter` 経由の検査を削除。`WorkerProbeSpec`（検査の定義）・`run_in_daemon`（通常の
  `POST /tasks` で検査 task を投入し `worker_finished` の outcome を読む）・`cap_worker_main`（daemon が起こす worker の中身）・
  `probe_worker`（結果 → 報告。検査先 identity と `probe_task_id`）。
- `main.rs`: `cap-worker`（測定 daemon の `[adapters.fake] command`）。`cap-probe --arm b|c` は identity 照合後に検査 task を投入。
- `preflight.rs`: `Host::probe_daemon_worker`。B/C は `check_bc_capabilities`（preflight の最後）。
  `Report.probe_targets` に `probe_task_id`。
- runbook（cap-runbook §1: 測定 config への harness 宣言を含む）・worker-probe.md・host-final.md を現経路に更新。

## 試験
- 実 `celeris` binary の一時 daemon（`crates/celeris/tests/cap_probe_real_daemon.rs`、4 passed）: daemon 側だけの toolchain/変数で通る・
  CLI にあって daemon に無ければ env_failure・socket/mount・接続できない daemon は env_failure。identity と probe_task_id を記録、秘密は出ない。
- `launcher.rs`（7 passed）・`trial_state_cli_e2e.rs`（3 passed）: daemon 役が実 `cap-worker` を起こす。

## 未解決・提案
- 実の測定専用 daemon（launcher 起動）での再実走は人の手順（cap-runbook §1）。測定 config に `cap-probe` harness の宣言が要る（本番の config 書き換えは無い）。

## 追補（差し戻し: 実 B/C harness の adapter 環境を通す）
差し戻し理由: 検査が fake harness の adapter env で走り、B/C の実 harness（claude_code/codex/pi）の adapter env を通らなかった。
- `[harnesses.<id>] env_of = "<実 harness>"`（kind=fake のみ）を追加。`daemon/adapters.rs::declared_env` が実 adapter と検査 harness の
  環境を同じ関数で組む。config 検証（宣言済み harness・kind=fake）。
- 検査 task は arm の `run_harnesses`（`Config::worker_harnesses`）ごとに分野 `cap-probe-<h>` で投入。結果 id は `<h>/<check>`、
  `probe_task_id` は task id のカンマ区切り。
- 試験 `probe_runs_under_each_real_harness_adapter_env`（実 celeris binary）: adapter env が toolchain を足す→その harness だけ通る、
  落とす→daemon の process 環境が正しくても env_failure。
- 通らないもの（ADR D6）: harness 自体の command・permission_mode・container 化、試行の task-template 文脈。
- 本番移行: 測定 config に cap-runbook §1 の harness 宣言を足す（本番 config の変更なし）。

## 追補2（差し戻し: 投入条件）
- `run_checks` は他の検査が落ちている（`a.missing` 非空）とき `check_bc_capabilities` を呼ばない（投入 0 件）。試験 `preflight_stops_before_any_llm_call_on_missing_capability`（A が落ちると `probe_daemon_worker` 呼出し 0 件、対照は通る）。
- 投入前に `cap_probe::declaration_problems`（測定 config の宣言検証）。宣言なし・分野なし規則・実 harness 指し・`env_of` 欠落は投入 0 件・`env_failure`。試験 `undeclared_or_misdeclared_probe_harness_submits_nothing_and_is_env_failure`。
- `trial_state_cli_e2e` の worker 欠落試験は A も落ちる構成なので「A の失敗のみ・B/C は投入されない」に変更。B/C 側の欠落は実 daemon 試験が担う。
