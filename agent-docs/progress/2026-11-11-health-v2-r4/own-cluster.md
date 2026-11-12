---
title: "健全性v2 R4 own-cluster: cluster・tunnel の状態と処理を ClusterRuntime へ移す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: completed
updated: 2026-11-12
---
# 健全性v2 R4 own-cluster: cluster・tunnel の状態と処理を ClusterRuntime へ移す

完了日: 2026-11-12

## 概要

- `Dispatcher` の cluster・tunnel・job の poll の 25 field（`cluster_*`・`tunnel_*`・`last_cluster_*`・`warned_cluster_tool`・
  `connect_pending_clusters`・`container_probe`。`cluster_waiting` を除く）を `crates/task-dispatch/src/cluster_runtime.rs` の
  `ClusterRuntime` 1 個（`Dispatcher::cluster`）に移した。field は private で、`Dispatcher` は method だけを呼ぶ。
- `dispatcher/cluster.rs` の処理（接続の帳簿・生存確認・鍵認証の再接続・master の終了・tunnel の forward と probe・login の要否）は
  `impl ClusterRuntime` に移し、旧い `impl Dispatcher` の本体は消した。`dispatcher/cluster.rs` に残るのは `ClusterInputs` を組んで
  owner を呼ぶ入口と、org と run の表を読むクラスタの解決（`cluster_of`・`resolve_cluster`・`task_may_use_cluster`・`cluster_in_use`）だけ。
- `dispatcher/cluster_job_wait.rs` の poll のスレッドと受け口、poll の要求の組み立ては `ClusterRuntime`（`take_job_poll`・
  `start_job_poll`・`job_poll_request`）に移した。wait の行と task・WU の遷移（durable wait の再開・上限の質問）は task・WU の待ちなので
  `dispatcher/cluster_job_wait.rs` に残した（ADR の表で `ExecutionCoordination` の範囲。`cluster_waiting` も同じく `Dispatcher` に残る）。
- `run_cluster_hooks_off_async`（ssh の hook を async の外の OS スレッドで走らせる）はそのまま `cluster_runtime` に移し、呼び出しの
  場所（`try_auto_connect_cluster`・`ensure_cluster_master_for_tunnel`・`cos_chat/digest.rs`・`script_canary.rs`）は変えていない。
- 基準は `$CELERIS_WU_BASE`（WU の base）。

## 実装前 brief

1. 根本原因: cluster の状態 25 field が `Dispatcher` の field で、`dispatcher/` のどの module からも書ける（ADR D1・D2）。
2. 責務 owner: `crate::cluster_runtime::ClusterRuntime`（ADR D1 の表）。
3. 既存の再利用先: 既存の処理を本体ごと移し、判定は変えない。試験は `dispatcher/tests/cluster_tunnel.rs`・`cluster_job_wait.rs`・
   `tests/mod.rs` の `liveness_round` をそのまま使う。時計は `Dispatcher::monotonic_now` と同じ時計を `monotonic_clock` で渡す。
4. 選択肢: (a) field を struct に包んで `self.cluster.conn` と書く（ADR で不採用の梱包だけ）。(b) 処理ごと owner に移し、owner の外の値を
   引数（`ClusterInputs`）で渡す。b を採った。port（connector など 9 件）は ADR の表では組み立て・`ProbePorts` だが、受け入れ条件
   「cluster の field が `Dispatcher` から消え `ClusterRuntime` 1 個になる」に従い owner に持たせた（ADR に付記）。
5. 新しい状態・分岐・fallback・port: 無し（`ClusterInputs` は借用の束、`JobPollSlot` は既存の `try_recv` の 3 分岐の名前）。
6. 撤去対象: `Dispatcher` の 25 field と初期化、`impl Drop for Dispatcher`（tunnel prober の停止は `impl Drop for ClusterRuntime` に移した）、
   `dispatcher/cluster.rs` の `impl Dispatcher` の本体、`cluster_job_wait.rs` の poll の型・`shell_quote`・スレッド起動、module map の旧い行。
7. 不変条件: 振る舞いは既存の cluster 試験（同じ assertion のまま owner を通す）、構造は `structure_guard_cluster_runtime` が確かめる。

## 試験の変更

- 足した試験 `structure_guard_cluster_runtime`（`cluster_runtime.rs`）: 不変条件は ADR D2 の 1〜3・7（field が private、
  `#[cfg(test)]` の field が無い、組み立て役の型を受けない、`dispatcher` module を使わない、field 25 件）。既存の試験は振る舞いだけを
  見るので、field を `pub` に戻しても通る。
- 既存の試験は assertion を変えていない。field の直接参照を `ClusterRuntime` の `#[cfg(test)]` の accessor（`connected_mut`・
  `conn_map`・`conn_map_mut`・`cooldown_map_mut`・`disconnect_info_map`・`last_liveness_mut`・`last_tunnel_refresh_mut`・
  `last_command_probe_mut`・`job_polls_in_flight`）に置き換えただけ。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(/cluster|tunnel|session_resume|git_workspace|container|cex_/)'`: 120 passed。
- `cargo nextest run -p task-dispatch -E 'test(structure_guard_cluster_runtime)'`: 1 passed。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0（repo_health_gate: all checks passed）。
- 計画の check（module map の `cluster_runtime` の行、`Dispatcher` の struct に cluster の field が無い、`cluster_runtime*` に
  `Dispatcher` の参照が無い）: すべて exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（476 秒。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  nextest 3739 passed・11 skipped・失敗 0）。
- 計画の check（`! awk … | grep -nE '^    (cluster_…|tunnel_…|last_cluster_…|warned_cluster_tool):'`、
  `! grep -rnE '(impl|&mut|&) *Dispatcher\b' crates/task-dispatch/src/cluster_runtime*`、module map の行、範囲 check）: すべて exit 0。
  `pub struct Dispatcher` の field は 92 件。

## commit（件名）

- phase 1: cluster・tunnel の状態と処理を ClusterRuntime へ移す
- phase 1: ClusterRuntime の構造の試験と地図・ADR の付記を足す
- phase 1: own-cluster の進捗を書く
- phase 1: ClusterRuntime の doc comment の旧い Dispatcher の参照を直す（`Dispatcher::tunnel_state`・`tunnel_probe_state`・
  `Dispatcher::new` を `ClusterRuntime` に、base から無かった `Dispatcher::effective_work_dir` を `cluster_of` に直した）
- phase 1: own-cluster の test-affected の結果を書く

## 未解決事項

- `cluster_waiting` は `Dispatcher` に残る（ExecutionCoordination を作る unit が移す）。
- celeris の配線（`set_cluster_connector` など）は `Dispatcher` の setter のまま。setter は `ClusterRuntime` に渡すだけの入口。

## 提案

- 無し。
