# ADR: 偽 CDP screencast 試験の失敗原因と LiveTap の NUL 走査

- 状態: 採用
- 日付: 2026-10-19

## 背景
`browser_launcher::live_tests::browser_launcher_live_frame_cdp_screencast_start_ack_stop_on_fake_cdp` が全体試験で 1 件だけ落ちた。単独実行では通る。

## 観測
`taskset -c 0` で 4 process を同時に走らせる（CPU を焼く負荷ではなく 1 core の取り合い）と 240 回中 16 回、`live_tests.rs` の `agent_events` の barrier が `barrier: SinkFailed`（5.48 s）で落ちた。controller の応答待ち上限（5 s）を超えた。

## 原因
`LiveTap::demux` は Chrome から 16 KiB ずつ読み、毎回 `buffered` を先頭から NUL まで走査していた。偽 Chrome は ACK 後に上限超過の巨大 frame（約 2.8 MB）を書き、その後ろに barrier の応答が続く。巨大 frame の受信中の走査が二乗になり、debug build で 1 core を共有すると barrier 応答の転送が 5 s を超えた。順序（frame→応答）自体は正しく、frame の内容が漏れた形跡はない。

## 修復
`demux` は既に走査した範囲（`scanned`）を覚え、新しく読んだ bytes だけを走査する。FRAME-1/FRAME-2、巨大 frame の棄却、agent/LIVE 分離、ACK/stop/detach、許可コマンド検証は変えない。timeout は延ばさない。回帰試験 `browser_launcher_live_frame_tap_forwards_reply_behind_huge_frame_in_pieces` は、巨大 frame を 1 KiB ずつ書き終えた後にだけ偽 Chrome が応答する順序を channel で強制し、応答が順に controller へ届き frame が漏れないことを検査する。
