# Browser injection の時間依存試験

---
tasks: [01M3Y4AV5ZJNMV5W81EMTEV2CZ]
---

`real_broker_browser_injection_receipt_and_origin_guards` では、`IsolatedRuntime::launch` が返しても browser の CDP pipe が応答可能とは限らない。直後の `Target.createTarget` は 5 秒の poll 上限で `SinkFailed` になり得た。

試験は `Browser.getVersion` の `product` と `protocolVersion` を受け取るまで CDP ready を待つ。`attack-test-hooks` feature の下でだけ CDP 応答待ちを 30 秒に設定し、ready 待ち全体には 60 秒の異常時上限を置いた。runtime の終了も確認し、失敗時には最後の応答、bwrap と sandbox init の `/proc/<pid>/status`、stderr を報告する。sandboxd は browser の終了時に自身も終了する。ready 後の `Target.createTarget` エラーは引き続き失敗となる。receipt に秘密と selector が無いこと、origin 不一致・cross-origin iframe・auth section 外を拒否する assert、および前提条件が欠けたときの失敗の扱いは変更していない。

## 遅延フックによる再現

試験専用の scripted CDP responder が `Target.createTarget` を受けてから 6 秒後に有効な応答を返す。CPU 負荷は使わない。

1. `response_timeout_for_test(Duration::from_secs(30))` の呼び出しを入れる前に、`cargo test -p task-worker --test browser_injection_wire delayed_cdp_page_target_response -- --exact --nocapture` は 5.00 秒で `delayed page target: SinkFailed` により exit 101。
2. 同じコマンドを呼び出し追加後に実行すると 6.00 秒で `1 passed`、exit 0。

## 実 browser の検証制約

この run の sandbox では `unshare -U -r true` が `unshare: write failed /proc/self/uid_map: Operation not permitted`、exit 1 だった。`cargo test -p task-worker --test browser_injection_wire` は scripted responder と inner guard の 2 件が通り、実 browser の 1 件は `unshare: unshare failed: Operation not permitted` で失敗した。実 browser での修正前後の再現と全件通過は未確認であり、user namespace が使える環境で同じ全件コマンドを再実行する必要がある。

## 実環境での確認（最終コード、2026-10-02、ADR-0079 D7 real-env-2 人の回答）

人が `docs/PROGRESS.md`「時間依存試験の決定化」の「人が実行する手順」を、merge-main 完了後の最終 SHA `ab1914e629d9`（`e64043be` の SIGCHLD 継承修正と main merge `a46b7423` を含む）で実行した。host `home-dev`、`unshare -U -r true` は exit 0、load average 14〜18（CPU を焼く負荷なし）。`ab1914e629d9` を `/var/tmp` の worktree に取り出し、`CARGO_TARGET_DIR` はローカル、`CELERIS_USERNS_TESTS=1` で実行した。`git diff --stat a46b7423 ab1914e629d9 -- crates` は空（crates の tree は同一）。

- 手順1（単独）×3: 3/3 `test result: ok`（各 0.43〜0.52s）。`SinkFailed`・`SKIPPED` の出力なし。
- 手順4 SIGSTOP stutter（`STUTTER_SCOPE=group`）×3: 3/3 `test result: ok`（1.65s・1.73s・7.03s、stutter による遅延が効いている）。`SinkFailed`・`panicked`・`SKIPPED` の出力なし。

結果: 全件合格。未解決の失敗なし。コードの変更はこの記録には含まれない。
