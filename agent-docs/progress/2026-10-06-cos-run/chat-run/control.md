# CoS chat run の停止・割り込み・再起動回収

- WorkUnit: control（01M47J2YNMZ14NQ4AXTNA355AA）
- 根拠: [CoS chat home D2](../../../adr/2026-10-05-cos-chat-home.md)

`chat_run_stop` または割り込み送信が DB に記録した `stopping` を dispatcher の tick が読む。手元の worker があれば run ID で process group に停止を送る。worker task は adapter の終了後に stop の理由を再確認し、本文と tool の途中記録を保ったまま `stopped` または `interrupted` に確定する。終端 transaction は thread の live-run 枠を解放し、run credential を失効させる。後続 run の ID は別なので旧 run 宛ての遅延 stop は影響しない。

再起動後に手元の handle が無い live run は、それだけでは回収しない。既存の daemon instance による ownerless 判定が確定してから閉じる。人の stop は queue pause を保ち、割り込みは受信済みの優先メッセージへ進む。通常の孤児は受信済み入力・添付を継続入力へ戻し、適用済み operation の receipt を冪等な system message に残す。`pending` operation があるときは副作用の成否が不明なので queue を pause し、確認を求める system card を出す。保留 operation がある間は割り込みも claim せず、成否が確定しても明示的な queue resume まで待つ。

試験は `cos_chat_run_control_` の 5 件。偽 harness の shell と FIFO barrier、SIGSTOP、固定 test clock を使い、待機時間を成否条件にしない。

検証: `cargo test -p task-dispatch cos_chat_run_` 34 件、`cargo test -p task-core chat_store_` 16 件、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`sh scripts/dev/check-doc-links.sh` は通過。
