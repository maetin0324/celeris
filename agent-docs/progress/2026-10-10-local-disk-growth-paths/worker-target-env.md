---
title: "/local disk growth paths: worker-target-env（D1）"
tasks: [01M4HX54MZ1P6ZYZEB6ABHD6BV]
status: done
updated: 2026-10-10
---

# worker-target-env: repo checkout の全 run・check に scratch の `CARGO_TARGET_DIR`（ADR D1）

## 完了（2026-10-10）

- `crates/task-dispatch/src/dispatcher/workspaces.rs` に `CargoTargetFallback`（`SharedPath` / `Ancestor` / `Missing` / `NotRust`）と
  `Dispatcher::cargo_target_repo`・`Dispatcher::cargo_target_fallback` を追加した。自分の作業場所の先頭が git の repo でないとき、
  (1) task の Local path・先頭の `kind = dir` repo が Rust の checkout（`Cargo.toml`）ならそれ、(2) 親から祖先へ（上限 8）たどって
  作業ツリーがある Rust の git checkout（`repos=[]` の子 task が親の checkout を使う形）に結び付ける。owner は子 task 自身
  （`task-<id>`）で、lease の repo は祖先の repo。Remote は対象外。
- worker run: `dispatch_run.rs` が先頭が git でない run に `RunExtras.cargo_target_fallback` を入れ、`worker_task.rs::run_worker` が
  その checkout で scratch の target を割り当てる（reflink seed・lease は既存の `allocate_scratch_target`）。
- daemon の検査（WU check・段の統合の検査・reviewer の checks・reviewer run）: `housekeeping.rs::check_cargo_target_env` と
  `with_review_cargo_env` が `cargo_target_repo` を使う。
- 渡せないとき: 祖先の Rust checkout がまだ無い（`Missing`）、または adapter が `with_env` 非対応で Rust の checkout に env を
  重ねられないとき、`tracing::warn!` に加えて run の `WorkerProgress`（`kind = status`、接頭辞 `cargo-target:`、定数
  `CARGO_TARGET_NOTE_PREFIX`）を残す。
- 試験 `crates/task-dispatch/src/dispatcher/tests/target_env.rs`:
  - `target_env_repos_empty_child_run_uses_the_scratch_target`（repos=[] の子 task の run と受け入れ command check。lease の repo_key は親の repo、親 checkout・子の作業場所に `target/` 無し）
  - `target_env_repos_empty_child_work_unit_check_uses_the_scratch_target`（repos=[] の子 task の WU check）
  - `target_env_integration_check_uses_the_task_scratch_target`（段の統合の検査。`IntegrationCheckStarted` の log で判定）
  - `target_env_missing_ancestor_checkout_is_recorded`（渡せない理由の event）

## 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| target_env_ 試験 | `cargo nextest run -p task-dispatch target_env_` | 5 passed（本葉の 4 本を含む）、exit 0 |
| task-dispatch 全体 | `cargo nextest run -p task-dispatch` | 932 passed、exit 0 |
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 5028 passed / 13 skipped、exit 0 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| fmt | `cargo fmt --all` | 差分なし |
| 範囲 | `sh "$CELERIS_WU_SCOPE_PATHS"` | `crates/task-dispatch/src/dispatcher{.rs,/…}` のみ（＋本進捗・ADR 付記） |

## 未解決

- 祖先の checkout を使う子 task の target は子の owner なので、子ごとに seed から作る（親の warm target は共有しない）。
  終端掃除は子の lease の終端で効く。
- ADR の表の試験接頭辞は `cargo_target_env_` だが、objective に合わせて `target_env_` にした。
- Rust でない作業場所・祖先にも Rust の checkout が無い task（`NotRust`）は従来どおり env を与えない（event も出さない）。

## 提案

- 子 task の objective で指す「repo の場所」を task の構造（祖先の checkout）から決める現在の推定を、将来は子 task 作成時に
  checkout の参照（owner checkout id）として DB に持たせると、D2 の「最後の利用者」追跡と共有できる。
