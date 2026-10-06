# CoS chat run: dispatcher から worker を起動・継続・停止する

---
tasks: [01M47J2YNMZ14NQ4AXTNA355AA]
status: done
completed: 2026-10-06
---

## 実装

- dispatcher の tick が thread ごとの durable queue を `chat_run_claim_next` で claim し、CoS の実効設定から一時 Task と `CosChatContext` を作って worker を起動する。通常 task の events や lease は作らない。
- thread 単位の session key で resume / retire / fresh を選び、resume 拒否時は同一 run 内で fresh を 1 回だけ再試行する。要約は worker が checkpoint に保存したものを読む。
- worker の text、tool、thinking/status と終端を `ChatRunSink` が `chat_events` に書く。API の SSE は同じ event page と cursor を読む。旧 `result.actions` は実行せず、エラーカードを残す。
- 停止は process group の終了を待って run と credential を閉じ、queue を pause する。割り込みは旧 worker の終了後に優先入力を起動する。再起動時は孤児確定後に interrupted とし、新 run で継続する。副作用が不明なら人の確認を待つ。
- 添付を hash 照合して read-only で workspace に stage し、image/file の delivery を manifest に載せる。run credential は worker の env だけに渡し、終端で失効させる。

## 証拠

- `cos_chat_run_e2e_send_stream_interrupt_stop_resume_and_restart` は FakeAdapter の shell、FIFO barrier、一時 SQLite、test clock で送信から進行イベントの cursor、割り込み、停止、queue 再開、孤児回収を通す。待ち時間を結果条件に使わない。
- `CELERIS_TEST_JOBS=2 bash scripts/dev/test-parallel.sh`：exit 0。nextest 124 binaries / 4,127 passed / 0 failed / 12 skipped。doc-test を含む合計は 4,127 passed / 0 failed / 13 ignored（134 binaries）。
- `cargo test -p task-dispatch --lib cos_chat_run_e2e_`：1 passed。進行 status が store に保存された時点を通知する test adapter を使い、FIFO だけに依存せずイベント順序を確認した。
- `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`、`sh scripts/dev/progress-index.sh --check`：すべて exit 0。
- `cargo clippy --workspace -- -D warnings`：exit 0。

## 未解決事項

- 現時点で無し。

## 提案

- 無し。
