---
title: B/C 能力 probe を測定専用 daemon の worker 環境へ結び付ける（CLI 環境の代用をやめる）
tasks: [01M57MB8PWVW50KD127M30HEFT]
status: done
updated: 2026-10-18
---
# bc-probe-daemon-env

ADR: `agent-docs/adr/2026-10-18-bc-probe-daemon-env.md`。

## 再現（main 0f9c058642c4）
`WorkerDaemonEnv::from_process` と `Host::worker_daemon_env` の既定が CLI の `std::env` を使っていた。子 process も CLI の環境を継いでいた。
解消済みではない（本変更が初めての修正）。

## 変更
- `cap_probe.rs`: `from_process` を削除し `from_daemon_pid`（`/proc/<pid>/environ` から PATH・名指し変数・許可リストだけ）に。
  各検査は `env -i` で daemon の環境だけにする。`ProbeReport.daemon`（検査先 identity）。
- `env_check.rs`: `verify`（照合 1 回 GET → pid・instance_id・release を返す）。`check_identity` はその薄い版。
- `preflight.rs`: B/C は `verify` に通った pid だけを検査先にする。通らない / 指定なしは `capability` miss。`Report.probe_targets`。
  照合結果は env-check と共有（GET は 1 回）。`Host::worker_daemon_env(expected, pid)`。
- `main.rs`: `cap-probe --arm b|c` は `--manifest --measure-config --daemon-pid --api-url --token-env` が要る。
- runbook（cap-runbook §1）・worker-probe.md・host-final.md の旧説明（B/C も A と同じ sandbox/argv）を現経路に更新。

## 試験
- `tests/cap_probe_daemon.rs`（新規。daemon の代役は `env_clear` した実 process で、`from_daemon_pid` → `probe_worker` の実経路。手動注入なし）:
  daemon にだけある PATH・変数で通る / 秘密と無関係な変数は持ち込まない / CLI は正常でも daemon の toolchain・変数が欠ければ B・C の env_failure /
  socket・mount 欠落 / 子が CLI の環境を継がない（CARGO_MANIFEST_DIR が見えない）/ preflight 全体で identity 照合済み pid の環境を使い
  `probe_targets` に pid・instance_id を記録 / identity が別 pid・測定 daemon 指定なしなら検査しない。
- `tests/trial_state_cli_e2e.rs`: 実 CLI（子 process）＋実 router＋実 process の daemon 代役。CLI の PATH と daemon の PATH を分け、socket / toolchain 欠落で B・C が投入前に env_failure。

## 未解決・提案
- 実 `celeris` binary を launcher で起動した host での再実走は人の手順（cap-runbook §1）。本 run では実 daemon を起動していない（代役 process）。
- dispatcher が run ごとに足す env と adapter 設定 env は取らない（ADR の限界）。
