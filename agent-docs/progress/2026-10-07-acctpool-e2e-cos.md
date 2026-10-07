---
title: account_pool_scenarios の a/b 取り違え（CoS triage run がプールの "a" を先に取る）を台本の設定で切り分ける
tasks: [main-direct]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# account_pool_scenarios の e2e 失敗の修正

## 症状

`tests/e2e/tests/account_pool_scenarios.rs` の 2 本が、t1 の `WorkerStarted.account` に "a" を期待して "b" を得て落ちる
（main f78b67e9 と efd38f0c の両方）。ホストの `/` が 5 GiB を切ったときは `t1 never completed` / `t1 was never dispatched` にもなる。

## 原因（証拠）

1. **CoS triage run がプールを使う。** `[cos]` は既定で有効（`crates/celeris/src/config/cos.rs` `default_enabled`）。
   `celerisctl add` の draft が受信箱の `draft_accept` 項目になり（`cos_inbox_items`）、CoS の triage run が唯一の
   provider `pool` から least-loaded（同点は id 昇順）で "a" を取る。試験 DB の `node_sessions` に
   `cos_chat … claude-code|a` の行、CoS run の `stdout.jsonl` に a のスタブの util 0.9、その直後（約 50 ms 後）に
   `dispatching … account="b"`。
2. **終わった CoS run の分が 1 tick 残る。** `Dispatcher::account_in_use` は `cos_chat_launch.accounts_in_flight` を数えるが、
   終わった handle を刈るのは `tick_cos_chat_launch`（`tick` 内で `dispatch_ready` の**後**）だけ。スタブが即座に
   終わっても、次の tick の dispatch では "a" が `in_use = 1`（スコア 0.95）で "b"（1.0）に負ける。だからほぼ毎回 "b"。
3. **空き容量の検査は `/` を必ず見る**（`housekeeping.rs` `check_disk_space`）。`/` が 4.3 GiB（既定の下限 5120 MiB 未満）の
   とき `dispatch paused for disk space` で t1 が始まらない。

## 修正

台本の設定（`write_config`）に `[cos] enabled = false` と `[dispatch] min_free_disk_mb = 0` を足した。この台本は
「ワーカー run がどのアカウントに行くか」だけを見るので、プールの消費者をワーカー run に限り、ホストの空き容量に
左右されないようにした。期待値は変えていない。

## 証拠

- `cargo nextest run -p e2e --test account_pool_scenarios` を 5 回: 5 回とも 3 passed（各 2 秒弱）
- 修正前（`min_free_disk_mb = 0` だけ足した状態）: 2 failed、`left: Some("b") right: Some("a")`
- `bash scripts/dev/test-parallel.sh`: exit 0、4661 passed / 13 skipped、doctest exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 未解決事項

- 他の e2e 台本も `min_free_disk_mb` を既定のまま使う。ホストの `/` が 5 GiB を切ると release gate 全体が環境要因で落ちうる。

## 提案

- `accounts_in_flight` の刈り取りを tick の先頭（`dispatch_ready` の前）でも行う。終わった CoS run がワーカー run の
  アカウント選択を 1 tick 歪めない（本件は台本で切り分けたので未実施）。
- e2e 共通の設定雛形に `[dispatch] min_free_disk_mb = 0` を入れるか、検査対象の `/` を設定で外せるようにする。
