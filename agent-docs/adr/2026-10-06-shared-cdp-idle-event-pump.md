# shared CDP relay の idle event pump

- 日付: 2026-10-06
- 状態: 実装済み（task 01M48DGY90GJAW2T8C95267HV3、WorkUnit cdp-pump）

## 背景と決定

`Page.navigate` の応答後に Chrome が出す `Page.loadEventFired` を agent-browser は command を追加せずに待つ。従来の relay は次の command が来るまで Chrome pipe を読まないため、open が 25 秒で timeout していた。relay は agent socket を最大 20 ms 待ち、入力がなければ Chrome pipe の利用可能な frame を非ブロッキングで読む。event は CDP session ID ごとに保持し、その session を attach した agent 接続だけに渡す。接続に属さない event は他の接続用に残す。

未取得 event は全 controller で最大 4096 件とし、超過時は最も古い event を捨てる。無制限に増えるメモリを防ぎつつ接続は維持する。auth 区間と identity 復元後は agent への観測を止める。auth 区間中の relay は idle pump も転送も行わない。event の redisplay 検査と request body の除去は command 応答経路と idle 経路で共通に行う。

## 検証

Chrome を使わない scripted browser の試験で、修正前の relay は event 待ちで失敗し、修正後は command を送らずに自 session の load event だけを受け取る。queue 上限と auth 停止も単体試験で固定する。
