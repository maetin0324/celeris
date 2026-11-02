---
title: S2 egress-stray — egress の「接続しない」試験を host 上の迷い込み接続で落ちないようにする
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 egress-stray

## 結論
- 「接続が来ない」を確かめる試験 2 件を直した。変更は `crates/task-worker/src/browser_egress/tests.rs` だけで、本番コードは変えていない。
- proxy が接続先に接続する退行は、今までどおり試験が落とす。本番に一時的に退行を入れて確かめた（下の「確認 2」）。

## 原因
integrate-wire の test-affected で `egress_get_switching_origin_on_the_same_connection_never_connects` の `timeout(50ms, other.accept())` が接続を受けて落ちた。

本物の proxy の接続ではないと判断した。根拠は `crates/task-worker/src/browser_egress.rs` の次の 3 点である。
- `serve_recorded` は `destination_recorded` と `TcpStream::connect` を 1 回ずつしか呼ばない。
- `read_header` は 1 byte ずつ読み、最初の `\r\n\r\n` で止まる。
- `forward` は応答を流した後 `discard_and_close` を呼ぶ。この関数は残りの入力を読み捨てるだけで解析しない。

## 未確認の点
- 迷い込み接続の出所は未確認である。他の試験が bind して捨てた port へ送る接続という候補は、「試験の loopback listener・stub が host 上の他の接続で落ちないようにする」の commit で扱った `echo_listener` と同じ形だが、今回の接続の相手は記録が無く特定できない。
- `egress_get_switching_origin_on_the_same_connection_never_connects` の失敗そのものは再現していない。下の確認 1 は、迷い込みを試験に入れた場合の再現である。

## 直し方
- `observe_unreached(listener, marker)`: 自分で番兵の接続を 1 本張り、番兵を accept するまで accept する。番兵より先に張られた接続を、静かな時間を待たずに数える。結果は `Quiet`・`Unmarked`・`Marked` のどれか。
- `never_connects(scenario)`: 新しい port で `scenario` を走らせて判定する。目印が届けば `Reached::Marker` で即失敗する。目印の無い接続は迷い込みの疑いとして新しい port でやり直し、3 回とも接続が来たら `Reached::EveryAttempt` で失敗する。
- 目印は 2 つ目の要求の path に入れる（`/egress-never-reached-<pid>-<attempt>-<port>`）。
- 接続 1 本を読む時間には上限 2 秒（`MARKER_READ_LIMIT`）を置いた。開いたまま何も送らない接続だけがこの時間を使う。上限を過ぎた接続は `Unmarked` になるので、判定は時間の長さで合否が変わらず、やり直しに回る。

## 直した試験
- `egress_get_switching_origin_on_the_same_connection_never_connects`: `never_connects` で判定する。
- `denied_origin_never_reaches_even_the_configured_dns_socket`: resolver の listener を `127.83.0.1` に bind し、`observe_unreached` が `Quiet` を返すことを確かめる。`destination` はこの address を受け付けた。

`tests.rs` の他の試験に「接続が来ない」確認は無い。`timeout(..., accept())` が `is_err` であることを確かめる箇所はこの 2 件だけだった。`browser_allowed_domains_egress_denies_outside_task_and_grant` の listener は accept の確認をしていないので、変えていない。

## 足した試験（名前に egress_stray を含む）
- `egress_stray_on_the_first_attempt_only_still_passes`: 本物の GET 経路の 1 回目だけに迷い込みを 3 種（すぐ閉じる、別の bytes を送る、開いたまま黙る）入れても通る。
- `egress_stray_connection_on_every_attempt_fails`: 毎回接続が来ると `EveryAttempt`。3 回とも別の port であることも確かめる。
- `egress_stray_marker_fails_on_the_first_attempt`: 目印が届くと 1 回目で `Marker { attempt: 1 }`。先に迷い込みが並んでいても見逃さない。
- `egress_stray_marker_survives_the_real_forward_path`: 本物の `serve_recorded` を通った要求にも目印が残る。
- `egress_stray_quiet_listener_stops_at_the_quiet_attempt`: 接続が来なければその回で止まる。
- `egress_stray_to_127_0_0_1_cannot_reach_the_other_loopback_listener`: `127.0.0.1` への接続は `127.83.0.1` の listener に届かない。

## 確認 1: 旧試験に迷い込みを入れると落ちる
旧試験 2 件の写しを一時的に足し、確認の直前に迷い込みを 1 本入れて走らせた。確認後に写しを消した。

```
cargo nextest run -p task-worker --no-fail-fast -E 'test(tmp_old)'
FAIL tmp_old_denied_with_stray     assertion failed: timeout(Duration::from_millis(30), listener.accept()).await.is_err()
FAIL tmp_old_switching_with_stray  assertion failed: timeout(Duration::from_millis(50), other.accept()).await.is_err()
Summary 2 tests run: 0 passed, 2 failed
```

## 確認 2: 本物の退行を新しい試験が落とす
`serve_recorded` を一時的に書き換え、応答の後で 2 つ目の要求を読んで接続するようにして走らせた。確認後に `git checkout` で戻した。

```
cargo nextest run -p task-worker --no-fail-fast -E 'test(egress_stray) | test(switching_origin) | test(serves_one_request)'
FAIL egress_get_switching_origin_on_the_same_connection_never_connects  Err(Marker { attempt: 1 })
FAIL egress_stray_on_the_first_attempt_only_still_passes
Summary 8 tests run: 6 passed, 2 failed
```

## 検査（提出時の HEAD で実行）
基準は `$CELERIS_WU_BASE`。

| コマンド | 結果 |
|---|---|
| `cargo nextest run -p task-worker -E 'test(egress_stray) \| test(switching_origin) \| test(denied_origin_never)'` を 5 回 | 5 回とも 8 件すべて合格 |
| `bash scripts/dev/test-affected.sh` | exit 0、4958 件合格、20 件 skip |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo clippy -p task-worker --all-targets -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

## commit
- phase s2: egress-stray: 接続しない確認を迷い込みと区別できる helper に直す
- phase s2: egress-stray: 原因と確認の記録

## 未解決事項
- 迷い込み接続の出所（上の「未確認の点」）。

## 提案
なし。
