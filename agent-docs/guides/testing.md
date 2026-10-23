# 試験の指針: CPU を焼く負荷をかけず、時間依存の不具合を決定的に再現する

---
tasks: [01M3Y4AV7801NSXB6FD698QZHW]
---

## 規則

試験・検査・検証（WU の check・受け入れ条件を含む）で CPU を焼く負荷をかけない。busy loop
（`while :; do :; done`）、`stress-ng`、cargo を並走させる負荷台本はどれも使わない。時間依存の不具合
（競合・飢餓・timeout）は、下の 4 つの方法のどれかで決定的に再現する。

## 禁止の理由

2026-10-02、flaky 修正の task が `scripts/dev/stress-e2e-phase3.sh`（`nproc` 本の CPU 焼きを最大 1800 秒、
並走 cargo test）を受け入れ検査として daemon 上で走らせた。本番と他 task が共用する host が load 65 になり、
他 task の統合検査（browser_injection_wire、controller_kill など）を巻き込んで落とした。人が process を止めた。

- 共用 host では負荷が自分の試験だけでなく本番 daemon と他 task の検査を壊す。
- 負荷による再現は確率的で、通っても直った証拠にならない（例: `docs/progress/phase-101-150.md` の
  codex login の競合は CPU 負荷だけでは 0/100 で再現せず、SIGSTOP stutter で 6/10 再現した）。

台本は 2026-10-02 に削除した。過去の実行記録は `docs/progress/phase-browser.md` に残っている。

## 方法 1: 時計の差し替え（tokio::time::pause・注入した時計）

時刻を引数で受け取る関数にし、試験は任意の時刻を渡す。実時間を待たない。

- 例（注入した時計）: `crates/task-dispatch/src/accounts.rs` の `AccountBook::set_cooldown(.., now: i64)` /
  `clear_expired(now: i64)`。試験 `crates/task-dispatch/src/accounts/tests.rs` の
  `book_clear_expired_removes_only_past_cooldowns` は `until: 100` と `until: 300` の cooldown を置き、
  `now` を渡すだけで期限切れを判定する。
- `tokio::time::pause` / `#[tokio::test(start_paused = true)]` はこのリポジトリには例が無い。最小の擬似例:

```rust
#[tokio::test(start_paused = true)]
async fn timeout_fires_after_deadline() {
    let fut = tokio::time::timeout(Duration::from_secs(30), pending::<()>());
    // 仮想時計なので 30 秒は即座に進む。sleep に頼る実時間の待ちは入らない。
    assert!(fut.await.is_err());
}
```

## 方法 2: 出来事待ち（固定 sleep ではなく条件が立つまで待つ）

「何 ms 待てば終わっているはず」と書かず、観測できる出来事（状態・ファイル・件数）を上限付きで待つ。

- 例: `crates/task-api/tests/common/mod.rs` の `eventually(within, cond)`。
  `crates/task-api/tests/stream.rs` の `the_seventeenth_stream_is_rejected_with_503` は
  `eventually(TWO_SECONDS, || state.active_streams() == 15)` で stream の確立を待つ。
- 例: `crates/task-dispatch/src/dispatcher/tests/mod.rs` の `liveness_round` は probe が `probe_inflight` を
  外すまで呼び続け、結果を読み切ってから判定する。ほかに `crates/scratch-cache/src/tests.rs` の `wait_until`、
  `crates/task-dispatch/tests/unified_kill.rs` の `wait_until_gone`。
- 例（web e2e、「何も起きないこと」の確認）: `web/e2e/realtime/refetch-scope.spec.ts` は page の時計を
  Playwright の clock に差し替えて止め、`web/e2e/support/realtime-probe.ts`（試験側の EventSource・fetch の
  観測点）で「app が SSE を処理し終えた件数」を待ってから `clock.runFor` で束ね窓と poll を進める。最後に
  取り直しを起こす目印の event を流し、観測が空振りしていないことも確かめる。

## 方法 3: 対象 process への SIGSTOP/SIGCONT（stutter）

スレッド飢餓や「子の終了と出力読み取り」の順序競合は、CPU を焼かずに対象 process だけを
SIGSTOP / SIGCONT で細かく止めて再現する。負荷は host 全体に広がらない。

- 例: `crates/task-worker/src/claude_account/tests.rs` の
  `start_login_finds_the_url_even_when_the_process_exits_immediately` と
  `crates/task-worker/src/codex_account/tests.rs` の
  `start_login_codex_finds_url_and_code_even_when_the_process_exits_immediately`。この競合は試験 process に
  SIGSTOP 300 ms / SIGCONT 3 ms を繰り返して旧ロジック 6/10 失敗を再現し、読み切り（`drain_readers`）→判定に
  直した（`docs/progress/phase-101-150.md`）。stutter の台本自体はリポジトリに残していない。最小の擬似例:

```sh
# 対象 test process の pid に対して、短時間だけ止めては動かす（CPU は使わない）
while kill -0 "$pid" 2>/dev/null; do
  kill -STOP "$pid"; sleep 0.3; kill -CONT "$pid"; sleep 0.003
done
```

## 方法 4: 試験専用の遅延フック（gate で止める）

競合の窓を広げたいときは、試験用の adapter・fake に「ここで止まる」フックを入れ、試験側から開ける。

- 例: `crates/task-dispatch/src/dispatcher/tests/mod.rs` の `ParallelWuAdapter` は `with_delay(key, ..)`
  （key ごとの遅延）と `holding(key)`（`tokio::sync::Notify` の `gate` が開くまで run を終わらせない）を持つ。
  `crates/task-dispatch/src/dispatcher/tests/work_units.rs` の `parallel_units_survive_dispatcher_restart` は
  `.holding("a").holding("b")` で 2 つの WU を走らせたまま dispatcher を作り直す。

## WU の check・受け入れ条件

- 差分の試験は `bash scripts/dev/test-affected.sh`。変更 crate と逆依存 crate の nextest・doc test に加え、
  web の型に届きうる変更（`web/`・`docs/api/`・workspace 共通 file・`task-api` とその通常依存の crate。
  Rust の event kind や API の型を変えたとき）では web の型検査（typecheck = `tsc -b`）と unit 試験（vitest＋node --test）を先に走らせる。
  `--list` は対象 crate と `web: typecheck+unit (理由: …)` の行を出す。worktree に `web/node_modules` が無ければ
  `pnpm install --offline --frozen-lockfile` を先に行う（ADR 2026-10-23-test-affected-web-typecheck）。
  e2e・本物の browser・workspace 全体の試験は release gate だけ（ADR 2026-10-20-full-tests-at-release-only）。
- check に重い負荷の台本（CPU 焼き、`stress-ng`、並走 cargo）を置かない。
- 高負荷の host で落ちた試験は、単独で再実行して結果を記録し、必要なら上の方法で決定的な回帰試験を足す。
- web の e2e は機能（`pnpm -C web e2e`）と非機能（`pnpm -C web e2e:nfr`: 全画面の axe・latency・
  refetch-scope の掃引）に分かれている（`web/playwright.config.ts` の project）。WU の check は対象画面の
  functional spec を指定して流し（例: `pnpm -C web build && pnpm -C web e2e parity/inbox.spec.ts`）、
  nfr と `e2e:all` は visual-qa・最終の受け入れ・release gate の段で流す。webServer は dist/ が無いか
  古いときだけ build するので、check で先に build しても二重には build しない。

## loopback の fake server は期待する要求だけを数える

`127.0.0.1:0` は試験専用の接続元を保証しない。host の別 process が空接続や `GET /`
（例: Go-http-client）を送る場合がある。accept 回数を request 数にせず、期待する method・path と
完全な HTTP header・Content-Length 分の本文を確認した後で記録・通知・応答台本の消費を行う。
空接続、header/body の途中 EOF、別 path は記録せず、書ける接続には 404 を返す。
一回応答する stub も期待する要求が来るまで accept を続ける。認証や本文の assertion は残す。

手書き HTTP fixture は `tests/support/http_async.rs` または `http_sync.rs` を `#[path]` で参照する。
両者の framing は `http_request.rs` に集約した（HTTP/1.x・Content-Length、上限 1 MiB、chunked は非対応）。
Node の fake daemon は `web/server/test-support/http-fixture.mjs` で API 外要求を除き、本文の記録は
`end` / `req.complete` を確認する。upload 中断を検査するための bytes・abort 観測は、完了要求数とは区別する。
読み取りは有界とし、無言の接続が正規 client を妨げないよう接続ごとに処理する。
負の応答や timeout を試す fixture は、その意図を保ったまま期待する要求を識別する。
TCP の DNS・echo 等も protocol の完全な要求や既知の peer を確認する。接続自体の拒否・無応答を
測る fixture、ネットワーク namespace 内だけの fixture は、その性質を一覧に明記して区別する。

回帰試験では正規 client より先に空接続・別 path の GET・途中切断を注入し、404 の受信完了を
同期点にする。その後の正規応答と記録数、台本が消費されていないことを確かめる。無言の接続も
開いたまま正規要求を通す。確率的な反復や CPU 負荷で再現させない。
