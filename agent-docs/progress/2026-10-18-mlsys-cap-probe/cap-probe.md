---
title: K2/K4 必要能力の固定と A sandbox 同一経路の無 LLM probe
tasks: [01M56HCNTPN0H2VGP4JE0JYRM9]
status: done
updated: 2026-10-18
---
# cap-probe（本測定移行 P1-3 の WU）

## 実装
- `cases.rs`: `Case.required_capabilities`（id・kind・argv・expect_stdout）。K2（cc・c++・cmake・git・ucx pkg-config・ucx/cmake 最小ビルド・sirius 接続 `ssh -O check` のみ）と K4（python3・stdlib）を `case.json` に固定。
- `harness_exec.rs`: `Sandbox.toolchain`（ro_mounts・管理 socket・env）。`/`・`/home`・HOME を覆う path・HOME 上書きは `validate` が拒む。`wrap_resolved` を A の起動と probe が共有。
- `cap_probe.rs` + CLI `celeris-campaign cap-probe --case <id> --arm a|b|c --cap-env <json> [--dry-argv]`。cap env の `worker`（B/C）は `a` と同じ集合でなければ読み込み拒否。
- `preflight`: 選んだ case が能力を固定していれば `--cap-env` 必須、各 arm を probe、不足は check `capability`・`failure_class: env_failure`（モデル失敗と別）で `gate` が停止。preflight は最初の LLM 呼出しの前。
- A の実起動（`a_executor`）も同じ `--cap-env` の toolchain を使う。

## 証拠
- `cargo test -p celeris-campaign --test cap_probe`: 6 pass（argv 同一性・B/C 同一・不足は env_failure・preflight 停止）。
- `bash scripts/dev/test-parallel.sh` exit 0（5724 pass）、clippy exit 0、fmt --check exit 0。

## 未解決・人手順（cap-runbook は close WU）
- userns が要る実 sandbox 実走は worker sandbox で出来ない。人が host で `celeris-campaign cap-probe --case k2-... --arm a --cap-env <json>` を A/B/C で実行する。
- cap env の実値（UCX prefix・sirius 管理 socket path）は host 側の承認が要る。case の接続 argv は `/run/celeris-cap/sirius-ssh.sock` を想定。
- case.json を変えたので campaign lock は再計算される。旧 pilot の試行とは混ぜない。
- submodule 3 件込みの実 tree ビルドは grader-real WU。
