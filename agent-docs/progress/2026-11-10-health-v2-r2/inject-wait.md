---
title: 健全性v2 R2 unit inject-wait — credentiald の injection の最初の要求の待ちの ADR と、待ちを並行にする受付
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R2 unit inject-wait — credentiald の injection の最初の要求の待ちの ADR と、待ちを並行にする受付

task 01M6TYVJT0Z1CBGNK8VTNK8ZJH の WorkUnit inject-wait の記録。基準は `$CELERIS_WU_BASE`。

完了日: 2026-11-11

## 結論

- 決定 inject-accept の回答は a（最初の要求の待ちだけを接続ごとに並行にし、照合・lease の消費・注入は直列、待てる接続は 8 まで）。
- ADR [2026-11-11-credentiald-passed-stream-wait](../../adr/2026-11-11-credentiald-passed-stream-wait.md) を書いた。ADR-0109 D1 の「読み取り期限 5 秒」を改める決定として、D1 期限の変更（固定 5 秒 → `poll` で `IO_TIMEOUT` ごとに見直す出来事待ち）、D2 auth section への結び付け、D3 保険 `PASSED_STREAM_BACKSTOP` 120 秒とその根拠、D4 受付の並行、D5 上限 8、直列の受付の影響、採らなかった案を持つ。
- `crates/celeris-credentiald/src/ipc.rs` の injection の受付を `accept_injection` に置き換えた。旧い `serve_injection`（1 thread で 1 接続ずつ待って照合する経路）は消した。

## 変更

- `phase 1: wait for the first injection request on per-connection threads and match on one thread (decision inject-accept a)` — ADR の追加、受付の実装、試験 4 件、`injection_ipc.rs` の doc comment の参照の置き換え。
- `phase 2: record inject-wait evidence` — この記録と、`tests/injection_ipc.rs` の module doc に ADR の参照を足した。

受付の形（`ipc.rs`）:

| 処理 | 行う thread |
| --- | --- |
| 枠を取る（`WaitSlots::try_take`）。枠が無い・待ちの thread を起こせないときは要求を読まずに `invalid_request` を書いて閉じる（`admit_stream`） | accept の thread |
| `SO_PEERCRED` で uid・pid を取る → `await_first_request` → `read_request`。読めなければ固定の拒否を書く（`wait_and_forward`） | 接続ごとの待ちの thread（`thread::scope` の中、最大 8 本） |
| sink FD の検査 → `InjectionService::handle`（D3 の照合順・lease の消費・注入）→ 応答の書き込み（`answer_request`） | 照合 thread 1 本（mpsc で受け取った順） |

- 枠（`WaitSlot`）は accept から応答の書き込みまで要求と一緒に動き、drop で戻る。期限切れ・`invalid_request`・peer の取得の失敗・thread の起動の失敗・panic のどれでも戻る。
- 要求の body は `Zeroizing<Vec<u8>>` に入れて照合 thread へ move する。照合の後・照合 thread が無くて渡せなかった時・channel に残って捨てられた時のどれでも drop で zeroize される。
- `IO_TIMEOUT`・`PASSED_STREAM_BACKSTOP`（120 秒）・launcher の 5 秒（`browser_cdp_credential_ipc.rs` の `TIMEOUT`）は変えていない。D3 の照合順と lease の消費（`InjectionService::handle` の中）も変えていない。
- 試験から受付を回すため、`same-uid-harness` feature（ADR-0109 D6 の試験専用 feature）の中だけに `ipc::serve_injection_streams` を置いた。本番の `serve_inner` は同じ `accept_injection` を何もしない `on_wait` で呼ぶ。
- `write_reply` の引数を `&mut UnixStream` から `&UnixStream` にした（照合 thread と待ちの thread が `Arc<UnixStream>` を共有するため）。

## 実装前 brief（credential class の変更）

1. 根本原因: credentiald の `injection.sock` の受付は 1 thread で 1 接続ずつ処理していた。launcher-login-fix の後、auth section を開いた接続は最初の要求を最長 120 秒待つので、その間は他の session の接続が accept されず、注入が止まる（修正前の止まりは最長 5 秒）。launcher の `start_on_stream` は応答を 5 秒しか待たないので、同時の login の後の方は `sink_failed` から `Rejected` になり得る。
2. 責務 owner: credentiald の受付（`ipc.rs` の injection の accept の経路）。
3. 既存の再利用先: `InjectionService::await_first_request`（待ちの判定）、`read_request`・`write_reply`・`SeqpacketSink::new`・`InjectionService::handle`（そのまま呼ぶ）、`peer`（`SO_PEERCRED`）、試験の `Fx`・`mux`・`seqpacket`・`request`・`call_raw`。
4. 選択肢: (a) 採用。待ちだけを接続ごとの thread にし、照合は 1 本の thread で直列にする（決定 inject-accept の回答）。(b) 受付は 1 接続ずつのまま、ADR に上限と見直しの条件を書く（退行が残るので採らない）。照合も並行にする案は D3 の直列を崩すので採らない。
5. 新しい状態・分岐・fallback・port: 状態は受付の枠の数（`AtomicUsize`、memory だけ）。分岐は「枠が無い・thread を起こせない → 読まずに拒否」と「照合 thread が無い → 拒否」。定数 `MAX_WAITING_STREAMS`（8）。試験専用の入口 `serve_injection_streams`（feature の中だけ）。fallback・新旧切替は無し。
6. 撤去対象: `serve_injection`（1 接続ずつ待って照合する旧い経路）と、その中の拒否応答の closure（`denied` 関数に置き換えた）。`crates/` の「ADR-0109 D1 付記 2026-11-11」の参照（2 か所）を新しい ADR の参照に置き換えた。
7. 不変条件: 下の「試験が守る不変条件」。

## 試験が守る不変条件

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
| --- | --- | --- |
| `concurrent_first_request_another_stream_is_answered_while_a_silent_stream_waits`（`tests/injection_ipc.rs`） | auth section を開いた peer の黙った接続が待ちに入った後（出来事で確認）も、別の接続の要求は `IO_TIMEOUT` の内に照合されて注入でき、lease はちょうど 1 回消費される。応答の時点で黙った接続には応答も close も来ていない。同じ lease の 2 回目は `lease_used` | 既存の試験は 1 度に 1 接続しか開かず、接続してすぐ要求を書くので、待っている接続と別の接続が重なる形を通らない |
| `concurrent_first_request_streams_over_the_limit_are_denied_unread`（同上） | 待つ接続が 8 本あるとき、9 本目は要求を読まれずにすぐ `invalid_request` になり、sink に何も届かず、provider は呼ばれず、audit の journal も変わらず、待ちにも入らない。黙った接続を閉じると枠が戻り、同じ lease で注入できる | 上限は新しい |
| `concurrent_first_request_slot_returns_when_the_waiting_thread_panics`（`ipc.rs` の `accept_tests`） | 枠を持った thread が panic しても枠は戻る。上限は 8 | 枠は新しい |
| `concurrent_first_request_over_the_limit_or_without_a_thread_is_denied`（同上） | 枠が無い接続は待ちの thread を起こさず `invalid_request`。thread を起こせなかった接続も `invalid_request` で、枠は残らない | 同上 |

- 試験は固定の sleep で順序を作らない。黙った接続の待ちの thread が待ちに入ったことは `on_wait` の出来事（mpsc、保険 60 秒）で確かめてから、別の接続を送る。枠が戻るのを待つところは、拒否が lease に触れないことを使い、注入が通るまで 10 ms 間隔で繰り返す（保険 60 秒）。CPU を焼く loop は無い。
- 試験の終わりに `Accepting` の drop が黙った接続を閉じ、受付を止める接続を 1 本送り、受付の thread を join する。受付は `thread::scope` の中で待ちの thread と照合 thread の終わりを待ってから戻るので、join の後に残る thread は無い。
- 検出の確認（一時の変更。commit していない）: 待ちの thread を起こさず accept の thread で待つ形に `accept_injection` を変えると、1 本目の試験は `call_raw` の読み取り期限で落ちた（FAILED、10.29 秒）。元に戻して合格を確かめた。
- 既存の試験は消していない・弱めていない。`first_request_tests` の 2 試験と `tests/injection_ipc.rs` の既存 17 試験は変えずに合格する。`tests/injection_ipc.rs` の `inject_raw` は、送り先の socket を引数に取る `inject_raw_at` を呼ぶ形にした（既存の試験の送り先は変わらない）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

（検査の後に書く）

## 未解決

- 無し（userns・実 browser の全体試験は release gate の担当）。

## 提案

- 照合が直列なので、前の login の controller の CDP mux が sink に答えないと、後の login は sink の応答待ち（`IO_TIMEOUT`）の間止まり、launcher の 5 秒を超えれば `Rejected` になり得る（ADR の「直列の受付の影響」）。同時の login が増えたら、sink の応答待ちを照合の直列から外すかを別に決める。
