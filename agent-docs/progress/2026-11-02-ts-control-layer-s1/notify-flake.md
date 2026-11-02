---
title: S1 notify-flake — notify 試験の偽 webhook が受けた接続を応答なしに捨てていた
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S1 notify-flake — `a_failing_webhook_is_retried_three_times_and_then_given_up`

## 結論
- 原因は試験の偽サーバ。`fake_webhook_raw`（`crates/celeris/tests/notify.rs`）は 1 要求ごとに `fake_http::accept`（`tests/support/http_async.rs`）を呼ぶ。`accept` は受けた接続を `JoinSet` で並べて読み、最初に読み終えた 1 本を返し、残りの接続を `JoinSet` ごと捨てる。本物の送信の接続を受けた後で別の接続（迷い込み）が先に読み終わると、本物の接続は応答なしに閉じられる。
- 応答なしに閉じられた送信は、reqwest の `is_request` の誤りになり、`safe_error` が "invalid request" に写す（下の再現 2）。integrate-load の check 6 で見えた症状と一致する。
- 迷い込みの出所は未確認。候補は「`127.0.0.1:0` を bind して捨てた port へ送る」形の試験（同じ file の `an_unreachable_webhook_never_reveals_the_host` は捨てた port へ `POST /hook` を送り、文面の種別を確かめない）。test-affected は試験を別 process で並べて走らせるので、その port を偽サーバが取ると、迷い込みは `POST /hook` として通る。
- 疑いの「`connection: close` の接続を client が次の tick で再利用する」は再現しなかった。毎 tick の送信は偽サーバ側で新しい accept として届き、通常の実行で 3 回とも 500 を受けている。
- 本番の `notify.rs` は変えていない。

## 直し方（試験側だけ）
- `fake_webhook_raw` を書き直した。待ち受けの loop が接続を 1 本ずつ別の task（`answer_hook`）に渡し、各 task は要求を読み切って `POST /hook` なら `response` を、違えば 404 を返してから閉じる。受けた接続を応答なしに捨てない。`max` 件の `POST /hook` を受けたら本文の一覧を返す。
- 読みに時間切れを置かない（旧 `fake_http::read` の 5 秒）。負荷で要求の到着が遅れても 404 で切らない。
- 要求の区切りの判定は共通の `tests/support/http_request.rs`（`complete`・`NOT_FOUND`）を `http_framing` として読む。`tests/support/` の共有 file は変えていない。
- 回帰試験 `the_fake_webhook_answers_a_connection_even_if_another_request_finishes_first` を足した。

## 再現（決定的）
時間でなく接続の順序と出来事で決める。CPU 負荷はかけていない。
1. 偽サーバ（`fake_webhook(4, 500)`）へ接続 B を張り、要求の最後の 1 byte を残して送る。
2. `/proc/net/tcp` の LISTEN 行の rx_queue（accept 待ちの列）が 0 になるまで待つ（B を accept し終えたという出来事）。
3. 接続 A で要求を全部送り、応答を読み終える（A は 500 を受ける）。
4. B の残りの 1 byte を送り、応答を読む。

- 直す前（旧 `fake_webhook_raw`）: `cargo nextest run -p celeris --test notify -E 'test(the_fake_webhook)'` を 5 回、5 回とも FAIL（`B got no response: Ok(0) []`。B は応答なしに閉じられる）。
- 再現 2（一時的な probe 試験。直した後に消した）: `fake_http::accept` で受けた接続を応答せずに drop すると、`notify::post_webhook` の結果は `Failed("invalid request")` だった。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `cargo nextest run -p celeris --test notify` を 5 回続けて実行: 5 回とも exit 0、34 tests run: 34 passed。
- `bash scripts/dev/test-affected.sh`: exit 0（対象 crate celeris・celerisctl、844 tests run: 844 passed）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p celeris --test notify -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 変更
`phase 1: notify-flake: 偽 webhook が受けた接続を必ず応答してから閉じる`
- `crates/celeris/tests/notify.rs`: `fake_webhook_raw` の書き直し、`answer_hook`・`wait_until_accepted`、回帰試験 1 件。
- この進捗ファイル。

## 未解決事項
- 同じ `fake_http::accept` を使う他の偽サーバ（`tests/support/http_async.rs` の利用者）も、迷い込みが先に終わると受けた接続を捨てる。この WU の範囲（notify 試験）の外なので直していない。

## 提案
- `fake_http::accept` を「受けた接続をすべて応答してから閉じる」形に替え、全利用者を移す task を別に起こす。
- `an_unreachable_webhook_never_reveals_the_host` の「bind して捨てた port へ送る」形は、他の試験の偽サーバへ迷い込みうる。到達できない宛先を port の再利用に頼らない形（例: 予約済みで listen しない socket）にする。
