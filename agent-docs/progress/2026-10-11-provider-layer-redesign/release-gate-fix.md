---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
status: done
title: provider 層の再設計 — release gate で落ちた e2e worker_db_read_only の config を新しい宣言に移す
completed: 2026-10-13
updated: 2026-10-13
---

# release-gate-fix: e2e worker_db_read_only

親: `agent-docs/progress/2026-10-11-provider-layer-redesign.md`。main（`101aced5`）の release.sh の cargo-test（`CELERIS_USERNS_TESTS=1`）で
`tests/e2e/tests/worker_db_read_only.rs` の `a_codex_worker_run_cannot_write_the_daemon_db_but_can_read_it_with_celerisctl` が落ちた。

## 原因

1. 試験の config が `[adapters.codex]` だけで、`[llm_sources.*]`・`[harnesses.*]`・`[harness_selection]`・`[route_defaults.*]` が無かった。
   harness 選択が失敗し、run が起動しない。
2. 宣言を足しても `no declared route candidate for harness codex is eligible` で defer された。`codex_oauth` は managed account pool の
   source（`route_select::account_adapter_for_source_kind` が Codex に対応付ける）。`Dispatcher::route_state` は
   `[accounts] codex_dir` の下にあり、ログイン済み（`auth.json` がある）account だけを admission 候補にする。試験には `[accounts]` が無いので
   候補の account が 0 件だった。そのため `task_core::llm_route::admit_source` が `AccountUnavailable` を返した。ADR 2026-10-11 の
   fail-closed の設計どおりで、本番コードの不備ではない。
3. `[accounts] codex_dir` を置くと、daemon が account の確認として namespace の**外**で `codex app-server` を起動する。古いスタブは
   引数を見ずに探り（`celerisctl add` と直接書き込み）を走らせた。このため確認の起動から DB に直接 task が作られ（`direct`・`injected`
   が余分に 4 件）、`only the declared follow-up` の assert が落ちた。worker run（`codex exec`）の探りの結果は期待どおりだった
   （`direct_exit=1`、生の書き込みも失敗）。

## 直したこと（`tests/e2e/tests/worker_db_read_only.rs` のみ）

- config に `[accounts] codex_dir = "codex-accounts"` を足し、ログイン済みの account `codex-accounts/a/auth.json` を作る。
  運用セッションが足した `[llm_sources.codex]`（`codex_oauth`、`accounts = []`、`models = ["codex"]`）・`[harnesses.codex]`・
  `[harness_selection]`・`[route_defaults.codex]` はそのまま残す。
- スタブの codex は、`exec`（worker run）のときだけ探りを走らせる。`app-server`（account の確認）には
  `codex_account_pool_scenarios.rs` と同じ形の成功の handshake を返し、それ以外の起動は exit 2 にする。
- 件数 assert の失敗時の文言に task の題名の一覧を出す。冒頭 doc の `adapter = "codex"` を harness・source の言い方に直す。

試験の意図（ADR-0095/0098/0126: codex の worker run から daemon の DB は読めるが書けず、`celerisctl add` は後続の宣言になる）は変えていない。

## 証拠

| 検査 | 実行したコマンド | 結果 |
|---|---|---|
| 対象の試験 | `CELERIS_USERNS_TESTS=1 cargo test -p e2e --test worker_db_read_only` | exit 0、1 passed / 0 failed |
| 他の userns 試験 | `CELERIS_USERNS_TESTS=1 cargo test` を `-p e2e --test api_scenarios`、`-p celeris-credentiald --test injection_peer_uid`、`-p celeris --test worker_db_guard_refuse`、`-p task-api --test browser_restore_live_session` / `browser_restore_deliver`、`-p task-worker --test browser_launcher_ptrace` / `browser_restore_deliver` / `browser_egress_relay` / `browser_runtime_isolated`、`-p task-worker --lib db_guard` / `claude_code` で個別に実行 | すべて exit 0（12 / 4 / 2 / 1 / 1 / 6 / 4 / 1+2 / 1+6（3 ignored）/ 29 / 124 passed、failed 0）。旧 config のまま落ちる試験は無かった |
| 全体試験（release gate と同じ条件） | `CELERIS_USERNS_TESTS=1 bash scripts/dev/test-parallel.sh` | exit 0、`test-parallel: ok`。nextest `Summary: 4995 tests run: 4995 passed (1 slow), 13 skipped`、`CELERIS_TEST_SUMMARY`: failed 0、nextest_exit 0、doctest_exit 0、binaries 177、userns true、tmp_leftovers 4（script が後片付けする） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| fmt | `cargo fmt --all -- --check` | exit 0 |

cargo には `CARGO_TARGET_DIR=/local/celeris/data/scratch/ap-e2e-target TMPDIR=/tmp` を付けた。

## 未解決事項

- `injection_peer_uid` は `cargo test -- --nocapture` で走らせると内側の probe の出力が `test inner_peer_uid_probe ... PEER-UID-PROBE ...`
  と同じ行に出る。行頭一致の parse が外れて `SKIP: (not passed)` になる（試験自体は ok で終わる）。nextest（`test-parallel.sh`）経由の
  挙動は確かめていない。今回の config 移行とは関係しないので、ここでは直していない。
- `CELERIS_TEST_SUMMARY` の `passed` が 3 になった。nextest の Summary 行は 4995 passed。集計の parse の値が nextest の件数と合っていない
  可能性がある（fix5 の記録では 4998）。

## 提案

- **userns 試験が task の統合検査で走らず見逃される。** `CELERIS_USERNS_TESTS=1` でしか走らない試験は、task の統合検査（userns なし）では
  SKIPPED で通る。そのため config・fixture の移行の取りこぼしが release gate まで残った。次の対策を提案する。
  (a) config の形を変える task の統合検査は、userns が使える host では `CELERIS_USERNS_TESTS=1 bash scripts/dev/test-parallel.sh` を必須にする。
  (b) userns に依存しない部分（config の読み込みと、route 選択で run が起動すること）は、userns なしでも走る試験に分ける。例えば、試験
  fixture の config を `Config::load` と `route_select` に通すだけの単体試験にする。
  (c) 統合検査の報告に SKIPPED の試験の数と名前を出し、「userns 試験 N 件が未実行」が見えるようにする。
- account pool の source（`codex_oauth` など）に、ログイン済み account が 1 件も無いまま route 候補として宣言されている場合は、daemon の
  起動時に警告するか、config 検査で弾く。現状は run が `NoEligible` で defer され続けるだけで、原因が `[accounts]` の欠落だと分かりにくい。
