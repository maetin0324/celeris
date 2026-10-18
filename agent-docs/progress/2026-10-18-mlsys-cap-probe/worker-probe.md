---
title: B/C 能力 probe を task-worker の実起動関数経由にする
tasks: [01M57DNCFYT06NB1H4G7DA8KSS]
status: done
updated: 2026-10-18
---
# worker-probe

## 実装
- `cap_probe::probe_worker`: B/C の probe を `task_worker::FakeAdapter`（`run_subprocess`）に通す。LLM は呼ばない。検査 script を daemon 設定の PATH・env・cwd（workspace）の子 process で走らせ、`artifacts/cap-probe/<i>.{code,out}` で結果を受ける。campaign が task-worker に依存する。
- `CapEnv.worker` は配送ではなく daemon 設定（`WorkerDaemonEnv` = `[worker]` の PATH・env）と照合する期待値。env 値の違い・欠落、期待 PATH 要素の欠落は `worker-env` として arm 別 `env_failure`。mount（`test -d`）・socket（`test -S`）・必要能力の欠落も同様。新しい配送 API は無い。
- A の sandbox probe（`probe`）は変更なし。arm の付け替えはしない。`Host::worker_daemon_env`（既定 = この process の環境。daemon と同じ EnvironmentFile で起動する前提）を追加し、`ProbeRunner` は A 専用になった。
- 作業 2 の `CapEnv::pin`（toolchain 取得）は触らず、二重実装していない。

## 試験
- `tests/cap_probe.rs`: run_subprocess 経由（request.json・env・cwd）、socket 欠落、toolchain/道具欠落、daemon 設定の食い違いが B/C それぞれ env_failure。
- `tests/trial_state_cli_e2e.rs`: 実 CLI で worker の socket / toolchain 欠落 → B/C の env_failure で投入前に停止。launcher / trial_state の既存試験は worker PATH を cap env に合わせ、`sh` stub は複合 probe command のみ成功させる。

## 証拠
- `cargo test -p celeris-campaign`: 全 pass。`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`: exit 0。

## 未解決・提案
- 実 daemon の `[worker]` 設定を直接読む口は無い（celeris config に該当節が無い）。今は process 環境を daemon 環境の代理にしている。本番運用で daemon config を機械的に読めるなら `worker_daemon_env` をそこへ差し替える。
- `cargo clippy --workspace --all-targets` は task-dispatch の既存 type_complexity で落ちる（本変更と無関係）。
