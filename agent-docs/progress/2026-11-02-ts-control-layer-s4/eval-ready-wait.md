---
title: S4 eval-ready-wait — routing_eval_run の起動待ちを出来事待ちにし、落ちた原因を文面で読めるようにする
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S4 eval-ready-wait — routing_eval_run の起動待ちを出来事待ちにする

完了日: 2026-11-03（unit eval-ready-wait）。変えたのは試験だけで、製品コードは変えていない。

## 失敗の事実
- final review の `bash scripts/dev/test-affected.sh` で `celeris::routing_eval_run daemon_runs_two_stub_trials_with_hidden_checks_and_eval_records` だけが落ちた（35.267 秒、`routing_eval_run.rs:146` の『daemon API became ready』）。
- 同じ tree の統合の全件実行は 2 回とも 7139 passed。単独では約 1 秒で通る。
- 真因は未確認。負荷時の時間依存の失敗と推定する。旧い待ちは失敗の理由を何も出さなかったので、今回の log からは daemon が起動に失敗したのか、遅かったのかを区別できない。

## 旧い待ちの弱点
1. spawn した daemon（`tokio::spawn(celeris::run(..))`）の終了を見ない。起動エラーでも 30 秒待ち、理由も出ない。
2. reqwest の要求に timeout が無い。受け付けても答えない接続 1 本で残り時間を使い切る。
3. 起動 30 秒・trial 完了 8 秒・後始末 8 秒の固定の上限が短い。

## 直した点
- `crates/celeris/tests/daemon_port/mod.rs` に `wait_api_ready` を足した。各要求と daemon の `JoinHandle` を `tokio::select!` で並べ、daemon が先に終われば上限を待たずに run の結果（`Ok`/`Err` の Debug 表示か panic の文面）を入れたエラーを返す。要求ごとに timeout を付け、全体の上限に達したら経過秒・試行数・最後の接続エラー（source の連鎖）を返す。
- `routing_eval_run.rs` はこれを使う（要求 5 秒、全体 180 秒）。trial 完了の待ちは 120 秒、後始末の待ちは 60 秒にした（ADR-0125 の出来事待ち + 長い保険。条件が揃えばすぐ抜ける）。trial 完了の待ちが切れたときは daemon が終わっているかも文面に出す。
- helper の試験は新しい test binary `crates/celeris/tests/daemon_ready_wait.rs`（3 件）。`routing_eval_run` は PATH を書き換えるので 1 試験のまま。
  - `finished_daemon_fails_without_waiting_for_the_limit`: `Err` を返して終わった handle と panic した handle で、上限 300 秒を待たず（60 秒未満で）エラーになり、文面に `Err` の文面・panic の文面が入る。
  - `silent_connection_does_not_use_up_the_limit`: 最初の接続を受け付けて答えない listener でも、要求の timeout 200ms の後の接続が答えて成功する。
  - `no_listener_fails_at_the_limit_with_elapsed_time`: listen しない予約 port で上限 1 秒に達してエラーになり、文面に経過秒・上限・最後の接続エラーが入る。
- 他の試験 binary（`cap_probe_real_daemon.rs`）の `daemon_port` の使い方は変えていない。CPU 負荷はかけていない。

## 検査（提出時の HEAD、基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p celeris --test daemon_ready_wait --test routing_eval_run` → 4 tests run: 4 passed
- test-affected・clippy・fmt・文書の検査の結果は親の進捗 `../2026-11-02-ts-control-layer-s4.md` の「検査（eval-ready-wait、提出時の HEAD）」に書く。

## 未解決
- 真因は未確認。次に落ちたときは文面（daemon の終了と run の結果、または経過秒と最後の接続エラー）で原因を分けられる。
- `cargo clippy -p celeris --tests` は既存の `campaign_lock_e2e`（同じ module の多重読み込み）で、`--all-targets` は既存の task-core の lib test で落ちる。どちらもこの unit の変更ではなく、検査コマンド（`cargo clippy --workspace -- -D warnings`）の対象外。
