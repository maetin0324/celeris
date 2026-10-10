# Run log without raw stdout

---
tasks: [runlog-view]
---

実装: `files.stdout === false` では stdout file polling を停止し、機密保護通知、worker_progress events、利用可能な result summary/question を表示する。実行中の events は 5 秒間隔で取得する。概要の run リンクは「進捗を開く」と表示する。stdout がある run の既存ログ経路は維持した。

ADR: `agent-docs/adr/2026-10-10-run-log-without-raw-log.md`。
