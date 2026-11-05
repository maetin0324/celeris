---
tasks: [01M6N7XY9KX62X9TX696TEHXNF]
---
# ADR 2026-11-05: drain 中の daemon は自分が起動した release 準備の終了を待つ

- 状態: 採用・実装済み。
- 関係: [ADR-0040](0040-self-improvement-deploy.md) D4（drain）、[ADR-0070](0070-task-failure-visibility-and-handoff-safe-runs.md) D4（run が生きている限り待つ）、ADR-0051（配送の release 準備）。

## 背景
2026-11-05 の配送（task 01M6N7XY9KX62X9TX696TEHXNF、head 672b77ac）の release 準備が `cargo build --release` の途中で `Terminated` になった。準備（`prepare.sh`）は 08:59:03 に旧 daemon `38127f8a64fe` の子として起動した。09:17 の promote で旧 daemon は draining になり、手元の run が 0 になった 09:33:09 に「drained; exiting 0」で終了した。systemd が unit の cgroup ごと止め、同じ秒に準備の子も SIGTERM で死んだ。`prepare.sh` の trap が `{"ok":false}` を書き、新 daemon は配送を「リリース準備に失敗しました」で blocked にした。コードの不備ではなく、drain の判定が準備の子を数えていなかったことが原因である。

## 決定
1. `delivery::start_prepare` が起動した準備の子を process 内の数（`delivery::prepares_in_flight`）に入れ、子を待つ thread が終了時に外す。
2. tick loop は `in_flight` を「dispatcher の run とレビューの数 + 生きている準備の数」にする。drain 中の daemon は準備が終わるまで終了しない。準備の上限は `prepare.sh` の timeout（release 7200 秒 + verify 900 秒）で決まる。
3. drain の timeout と `drain_force_abort` の扱いは ADR-0070 D4 のまま（既定は待ち続ける。force abort のときは準備も unit の停止で止まる）。
4. 準備の結果の読み取り（`result.json`）は従来どおり active の daemon が行う。drain 中の daemon は待つだけで、配送の状態は書かない。

## 試験
- `crates/celeris/src/delivery/tests.rs` の `prepare_child_counts_as_in_flight_until_it_exits`: 子が生きている間は数が 1 増え、終了すると元に戻る。drain の判定（`in_flight > 0` なら終了しない）は既存の instance の試験が固定している。
