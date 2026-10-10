---
task: 01M4JAK3MY5G1K8T66Q4RTWSS5
unit: frame-core
status: done
completed: 2026-10-10
---

# frame-core: 揮発 LiveFrame と容量 1 の LatestFrameSlot

ADR `agent-docs/adr/2026-10-10-browser-launcher-live-view-frames.md` 付記 2026-10-10b の task-core 部分。

## 変更

- `crates/task-core/src/browser_live_frame.rs`（新規）
  - `LiveFrame`: seq・寸法・encoding（jpeg/png）・body。`Serialize`/`Deserialize`/`Debug`/`Clone` を実装しない。
    空 body・2 MiB 超・寸法 0 は `LiveFrame::new` で拒否。診断は `LiveFrameDiag`（byte 長・seq・寸法・encoding）だけ。
  - `LatestFrameSlot`: 容量 1。`publish` は未受領 frame を上書き（`FramePublish::Replaced`、上書き数を数える）、
    queue・再送・disk なし。`next()`（std の Waker だけの future、tokio 不要）/ `try_take()` で受け取る。
    `close()` は保持中 frame を捨てて待ち手を起こし、以後の `publish` は `Closed`。Debug は診断事実だけ。
  - compile_fail doctest 3 本（serde_json 化・`{:?}`・`ScrubbedLiveEvent` への `into` が通らない）。
- `crates/task-core/src/browser_isolation.rs`: `LiveSessionEntry::live_frames()`（既定 `None`）。
- `crates/task-core/src/lib.rs`: module 宣言。

task-core は tokio に依存しないため、async 待ちは自前の `Future` にし、試験は数える Waker で出来事を確かめる（時計を使わない）。

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo nextest run -p task-core browser_live_frame_` | 4 passed（backpressure_keeps_latest_only・not_serializable・rejects_oversize_and_empty・entry_has_no_frames_by_default） |
| `cargo test -p task-core --doc browser_live_frame` | compile_fail 3 passed |
| `cargo nextest run -p task-core` | 897 passed / 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

全体試験（`bash scripts/dev/test-parallel.sh`）はこの unit では流していない（追加だけの変更。close-out で流す）。

## 未解決事項

- slot の待ち手は複数登録できるが frame は 1 枚を最初に取った者だけが得る（単一購読前提）。viewer が複数の場合は
  task-api 側で購読ごとに slot を分けるか、1 本の stream に限る（api-stream unit で決める）。

## 提案

- なし
