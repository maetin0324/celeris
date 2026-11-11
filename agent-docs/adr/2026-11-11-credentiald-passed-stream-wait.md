---
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
---
# ADR 2026-11-11: credentiald の渡された injection 接続の最初の要求の待ちと、待ちを並行にする受付

- 状態: 採用（実装済み）。ADR-0109 D1 の「読み取り期限 5 秒」を改める。実装は task 01M6TYVJT0Z1CBGNK8VTNK8ZJH（健全性 v2 R2）の
  unit launcher-login-fix（D1・D2・D3）と unit inject-wait（D4・D5）。
- 関係: [ADR-0109](0109-browser-p4b-injection-ipc-cdp-sink.md) D1（socket と wire）・D2（peer の役割）・D3（照合順）、
  [ADR-0080](0080-browser-phase2-policy-broker-approval.md) H3（auth section）、
  [ADR 2026-10-09 credential username / post-login](2026-10-09-browser-credential-username-and-post-login-read.md)（launcher の Authenticate 経路）、
  [ADR-0125](0125-deterministic-time-tests.md)（出来事待ち＋長い保険）、
  [ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)。
- 記録: [launcher-login-fix の進捗](../progress/2026-11-10-health-v2-r2/launcher-login-fix.md)、
  [inject-wait の進捗](../progress/2026-11-10-health-v2-r2/inject-wait.md)。
- 決定 inject-accept の回答: a

## 背景

- daemon（`launcher_credential_login`、`crates/task-worker/src/browser_launcher_run.rs`）は auth section を開いてから
  credentiald の `injection.sock` に接続し、その FD を launcher に渡す。launcher が注入の要求を書くのは、login 頁の
  password 欄が出た後になる（login navigation の期限 15 秒）。
- ADR-0109 D1 は「1 接続 = 1 要求 = 1 応答、読み取り期限 5 秒」と決めていた。credentiald は accept した接続の最初の要求を
  `IO_TIMEOUT`（5 秒）しか待たず、接続から要求までが 5 秒を超えると要求を読む前に `invalid_request` を返して閉じた。
  launcher の注入は `sink_failed` → `Unauthorized` → `Rejected` になった（settings 段の統合で落ちた launcher login 試験の原因）。
- credentiald の injection の受付は 1 thread で 1 接続ずつ処理していた。

## 決定

### D1. 最初の要求の期限: 固定 5 秒から、`IO_TIMEOUT` ごとに見直す出来事待ちへ

- 最初の要求の待ち（`InjectionService::await_first_request`、本体は `wait_first_request`、`crates/celeris-credentiald/src/injection_ipc.rs`）は
  `poll` を `IO_TIMEOUT` ごとに回す。要求の byte・peer の close・接続の異常のどれかが来たらすぐ読みに進み、中身の照合は
  `read_request` と `InjectionService::handle`（D3 の照合順）に任せる。
- 要求が着いた後の部分読み（`read_request` の `SO_RCVTIMEO`）・sink の応答（`SeqpacketSink`）の期限は `IO_TIMEOUT` のまま変えない。
  ADR-0109 D1 の「読み取り期限 5 秒」は、この 2 つと、次の D2 で待ち続けない peer の最初の要求に残る。

### D2. 待ち続けるのは開いた auth section を持つ controller の間だけ

- `IO_TIMEOUT` の間に何も来ないとき、待ち続けるのは peer が登録済み controller（`LiveRegistry` の `controller_pid` と、
  `/proc/<pid>/stat` の開始時刻 `controller_start` が一致）で、その session に開いた auth section があるときだけである。
- daemon が auth section を閉じたら、次の tick（最長 `IO_TIMEOUT` 後）で `invalid_request` にする。
- auth section を持たない peer（worker・agent・登録の無い process）は、従来どおり `IO_TIMEOUT` の 5 秒で `invalid_request` になる。

### D3. 保険 `PASSED_STREAM_BACKSTOP` = 120 秒

- auth section が開いたままでも、待ちは `PASSED_STREAM_BACKSTOP`（120 秒）で打ち切る。
- 根拠: launcher が 1 操作を打ち切る期限（`LauncherLimits::default` の action 120 秒）に揃えた。launcher がそれより長く要求を
  書かないことは無いので、通常の待ちは要求の到着・peer の close・auth section の close で終わり、120 秒には届かない。

### D4. 受付: 最初の要求の待ちだけを接続ごとに並行にし、照合は 1 本の thread で直列に行う（決定 inject-accept の回答 a）

D1〜D3 の後、auth section を開いた接続は最初の要求を最長 120 秒待つ。受付が 1 接続ずつのままだと、その間は他の session の注入が
止まる（修正前の止まりは最長 5 秒）。受付を次の形にする（`accept_injection`、`crates/celeris-credentiald/src/ipc.rs`）。

- accept の thread は、接続ごとに待ちの thread を 1 本起こす。待ちの thread は受け付けた直後に `SO_PEERCRED` で peer の uid・pid を取り、
  `await_first_request` と `read_request` を行い、読めた要求（peer・body・sink FD・接続）を組にして照合 thread へ渡す。
- 照合 thread は 1 本だけで、sink FD の検査・`InjectionService::handle`（D3 の照合順・lease の消費・注入）・応答の書き込みを
  受け取った順に直列に行う。D3 の照合順と lease の消費は変えない。
- 要求を読めなかった接続（期限切れ・peer の取得の失敗・形の誤り・照合 thread が無い）は、その接続の待ちの thread が固定の拒否
  （`invalid_request` などの D3 の語彙）を書いて閉じる。この拒否は `handle` を通らず、lease にも provider にも触れない。
- 要求の body は `Zeroizing` に入れたまま照合 thread へ move する（写しを作らない）。照合を終えた時、照合 thread が無くて
  渡せなかった時、channel に残ったまま捨てられた時のどれでも、drop で zeroize する。

### D5. 待てる接続の上限 `MAX_WAITING_STREAMS` = 8

- 同時に受け付けておける接続は 8 までとする。数えるのは accept してから、その接続の応答を書き終えるまで（照合 thread の
  順番待ちを含む）なので、待ちの thread の数と照合 thread の順番待ちの数も 8 を超えない。
- 枠は drop guard（`WaitSlot`）で数える。期限切れ・`invalid_request`・peer の取得の失敗・thread の起動の失敗・panic のどの抜け道でも、
  guard の drop で枠が戻る。
- 9 本目の接続と、待ちの thread を起こせなかった接続は、accept の thread が要求を読まずに `invalid_request` を書いて閉じる。
  lease にも provider にも触れない。
- 8 の根拠: 同時に走る credential login は 1 本か 2 本で、8 はその数倍の余裕である。`injection.sock` は 0600 で、繋げるのは
  broker と同じ UID の process だけなので、上限は誤動作する同 UID の process に対する防御として置く。auth section を持たない
  peer は 1 接続あたり最長 5 秒しか枠を持たない。

### 直列の受付の影響（D4・D5 の後に残るもの）

- launcher の `start_on_stream`（`crates/task-worker/src/browser_cdp_credential_ipc.rs`）は credentiald の応答を `TIMEOUT` 5 秒しか待たない。
- D4 の前は、同時の 2 つの login の後の方は、前の方の最初の要求を待つ間（最長 120 秒）止まり、5 秒を過ぎると `sink_failed` から
  `Rejected` になり得た。修正前（D1 より前）も最大 5 秒止まっていた。D4 でこの止まりは無くなった。
- 照合が直列なので、後の方は前の方の `handle` が終わるまで待つ。`handle` で長いのは sink の応答待ち（送信・受信とも `IO_TIMEOUT`）で、
  通常は数 ms で終わる。前の方の controller の CDP mux が sink に答えないときは、後の方が 5 秒を超えて `Rejected` になり得る。
  credential は漏れない（応答は receipt か固定の拒否コードだけ）。

## 採らなかった案

- launcher protocol を分け、daemon が注入の直前に `injection.sock` へ接続する: credentiald の「接続してすぐ書く」前提に戻せるが、
  protocol 版の更新と daemon・launcher の両方の変更が要る。最初の要求の待ちは credentiald の受付（owner）で直せるので採らなかった。
- `IO_TIMEOUT` を伸ばす: auth section を持たない peer の黙った接続も長く待つようになり、fail-closed の期限が弱まる。
  「timeout を伸ばすだけ」の修正に当たるので採らなかった。
- 受付を 1 接続ずつのままにして上限と見直しの条件だけを書く（決定 inject-accept の選択肢 b）: 他の session の止まりが 5 秒から
  120 秒に伸びる退行を R2 が入れたままになるので採らなかった。
- 照合も接続ごとに並行にする: D3 の照合順・lease の消費・注入の直列を崩すので採らなかった（決定 inject-accept の条件）。

## 守る不変条件と試験

- auth section を持たない peer の黙った接続は `IO_TIMEOUT` で拒否される。auth section が閉じた後は、渡された接続も待ち続けない
  （`injection_ipc.rs` の `first_request_tests` の 2 試験）。
- auth section を開いた peer の黙った接続が待っている間も、別の接続の要求に `IO_TIMEOUT` の内に応答が返り、lease はちょうど 1 回
  消費される（`tests/injection_ipc.rs` の `concurrent_first_request_` の試験）。
- 待つ接続が 8 本あるとき、9 本目は要求を読まれずにすぐ `invalid_request` になり、lease は消費されない。待ちが終われば枠は戻る
  （同上）。
- 枠は thread の起動の失敗・panic でも戻る（`ipc.rs` の `concurrent_first_request_` の単体試験）。
