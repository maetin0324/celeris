---
title: 健全性v2 R2 unit launcher-login-fix — settings 段で落ちた launcher login 試験の原因と修正
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R2 unit launcher-login-fix — settings 段で落ちた launcher login 試験の原因と修正

task 01M6TYVJT0Z1CBGNK8VTNK8ZJH の WorkUnit launcher-login-fix の記録。基準は `$CELERIS_WU_BASE`。

## 結論

- 原因は credentiald の固定時間の待ちで、負荷はそれを表に出しただけ（adapter-settings・browser-port の退行ではない）。daemon（`launcher_credential_login`、`crates/task-worker/src/browser_launcher_run.rs`）は auth section を開いてから credentiald の `injection.sock` へ接続し、その fd を `authenticate` で launcher に渡す。launcher（`run_login`、`crates/task-worker/src/browser_launcher/backend.rs`）が注入の要求を書くのは、login の navigation と password 欄の待ち（login navigation の期限 15 秒）の後になる。credentiald（`serve_injection`、`crates/celeris-credentiald/src/ipc.rs`）は accept した接続の最初の要求を `IO_TIMEOUT`（5 秒）しか待たなかった。このため接続から要求までが 5 秒を超えると、要求を読む前に拒否の応答を書いて閉じ、launcher の注入は `sink_failed` → `Unauthorized` → `Rejected` になった。
- 接続の順は 2026-10-09 の commit『launcher 経路の承認後 Authenticate を本番で通る形にする（protocol v4）』から、credentiald の 5 秒は 2026-09-30 の commit『phase 4 (P4-B ipc): injection-only IPC …』からある。browser-port が `browser_launcher_run.rs` で変えたのは `sink` から `control`・`authority` への置き換えだけで、接続・`authenticate`・credentiald の箇所は変えていない。
- 直し方（原因の owner の credentiald）: 最初の要求の待ちを「出来事待ち＋長い保険」に変えた（`InjectionService::await_first_request`、本体は `wait_first_request`、`crates/celeris-credentiald/src/injection_ipc.rs`）。`poll` で `IO_TIMEOUT` ごとに見直し、要求の byte・peer の close が来たらすぐ読みに進む。何も来ないとき待ち続けるのは、peer が開いた auth section を持つ登録済み controller（pid と開始時刻が一致）の間だけで、auth section が閉じれば次の tick で拒否する。保険は `PASSED_STREAM_BACKSTOP`（120 秒。launcher が 1 操作を打ち切る `LauncherLimits::default` の action 120 秒に揃えた）。auth section を持たない peer（worker・agent・登録の無い process）は従来どおり `IO_TIMEOUT` の 5 秒で `invalid_request` になる。

## Rejected を返した分岐（試験側の観測）

`handle_authenticate`（`server.rs`）の分岐ごとの観測:

| 分岐 | 観測 | 判定 |
| --- | --- | --- |
| `authorize` | `ChromeSession::authenticate` が呼ばれた（broker stream の数 = 1） | 通過 |
| `passed_broker_stream` | 同上 | 通過 |
| backend の navigation（`Page.navigate`） | 一時計測で成功 | 通過 |
| page・selector の待ち（`login_password_document`） | password 欄を検出（redirect chain は同じ origin の 3 件） | 通過 |
| credentiald の lease・admission | journal に `use`（consumed）も拒否もない。最後の記録は supervisor の `revoke` | credentiald は要求を受け取っていない |
| 注入（`CdpController::inject`） | `sink_failed`。CDP の呼び出しは失敗していない | ここで `Err` → `Unauthorized` → `Rejected` |

統合検査の失敗も同じ形である。log は `integration-checks/integrate-settings/` の nextest 出力で、5867 件中 4903 番目、7.663 秒、`Recorded(Err("launcher_authenticate_rejected"))`。同じ時間帯には soak・dagbench の試験が並走し、host は高負荷だった。単独で流すとこの試験は 3.48 秒で通る。

## 再現の手順と結果（CPU 負荷を使わない）

方法 4（試験専用の遅延フック）を使った。`ChromeFixture::start_with` に渡す fixture の script に gate を足す。`gate-<name>` が fixture の directory にある間、IdP は `POST /<name>` への応答を保留し、要求が着いた時に `held-<name>` を書く。試験は `held-password` が現れるのを出来事として待ち（保険 60 秒）、`IO_TIMEOUT` + 1 秒の後に gate を外す。保留は login navigation の期限 15 秒より十分短い。

一時の観測（commit していない）:

| 保留した要求 | 保留 | 結果 |
| --- | --- | --- |
| `POST /password`（relay の自動送信） | 3 秒 | `Ok(HELD)`、SP が password を受信 |
| `POST /password` | 8 秒 | `Rejected`（経過 8.3 秒、lease は revoke、注入なし） |
| `POST /submit`（注入の後） | 8 秒 | `Ok(HELD)`、SP が受信 |

`POST /password` を保留している間、relay 文書への `Page.createIsolatedWorld` は 8.03 秒止まり、応答が着いてから成功した（fixture の CDP 応答待ちは 30 秒）。その後の注入が `sink_failed` になった。注入より後の待ち（`/submit`）は 8 秒保留しても通るので、5 秒を超えて困るのは「接続から要求まで」の区間だけである。

回帰試験 `launcher_login_regression_slow_password_page_still_injects_through_the_passed_stream`（mod `launcher_login`、`crates/task-worker/src/browser_launcher_run_tests.rs`）は上の gate を使い、`POST /password` を `IO_TIMEOUT` + 1 秒保留する。gate の解除は `held-password` の出来事待ち（保険 60 秒）の後にだけ起きるので、CPU 負荷にも sleep の順序にも依らず「接続から要求までが `IO_TIMEOUT` を超える」形を毎回作る。

| 時点 | コマンド | 結果 |
| --- | --- | --- |
| 直す前（前の run、3 回） | `cargo nextest run -p task-worker --lib -E 'test(/launcher_login_regression_/)'` | 3 回とも FAIL（9.6・10.3・7.6 秒、`left: Recorded(Err("launcher_authenticate_rejected"))`） |
| 直す前（この run） | `cargo test -p task-worker --lib launcher_login_regression_` | FAIL（exit 101、6.76 秒。journal は request・grant・supervisor の revoke だけで consumed なし） |
| 直した後 | 同上 | PASS（1 passed、6.82 秒・6.67 秒） |

credentiald 側の試験（`crates/celeris-credentiald/src/injection_ipc.rs` の `mod first_request_tests`）:

- `silent_stream_without_an_open_auth_section_is_denied_after_io_timeout`: auth section の無い peer の黙った接続は `IO_TIMEOUT` 以上待った後に `InvalidRequest` になる（fail-closed の維持）。
- `passed_stream_waits_while_the_auth_section_is_open_and_stops_when_it_closes`: auth section が開いている間は `IO_TIMEOUT` + 1 秒後に着いた byte を読みに進み、auth section を閉じると次の tick で `InvalidRequest` になる。

## 足した試験が守る不変条件

- IdP が password 欄を出すまでの時間が launcher の login navigation の期限（15 秒）の内にあれば、daemon が先に開いて渡した injection の接続で注入でき、lease はちょうど 1 回 consumed になり、SP は password を受け取る（回帰試験）。
- auth section を持たない peer の黙った接続は `IO_TIMEOUT` で拒否される。auth section が閉じた後は、渡された接続も待ち続けない（credentiald の 2 試験）。

## 既存の試験で検出できなかった理由

- `launcher_credential_login_navigates_injects_submits_and_consumes_the_granted_lease` などの fixture の IdP はすぐ応答する。単独では接続から要求まで約 1〜3 秒で、5 秒に届かない。IdP を「daemon の接続の後、注入の要求の前」に止める試験は無かった。
- credentiald 側の試験（`tests/injection_ipc.rs` の `call_raw` 等）は接続してすぐ要求を書くので、渡した接続が待たされる形を通らない。

## 変えていないもの

- `LauncherLimits::default` の action 120 秒、`browser_cdp_sink.rs` の `TIMEOUT` 5 秒・`AGENT_TIMEOUT` 25 秒・`FETCH_TIMEOUT` 40 秒、期限切れの `Rejected` と teardown（`handle_authenticate` の `None` 分岐）。
- credentiald の `IO_TIMEOUT`（5 秒）の値と、それを使う部分読み（`read_request` の `SO_RCVTIMEO`）・sink 応答（`SeqpacketSink`）の期限。要求が着いた後の D3 の照合（peer の役割・session・auth section・lease・selector）はすべてそのまま通る。
- launcher protocol（版 9）・daemon の接続の順・`handle_authenticate` の固定の `Rejected`。credential は従来どおり credentiald から sink FD（controller の CDP mux）にだけ書かれ、daemon と launcher の応答には receipt か固定の拒否コードしか出ない。
- timeout の延長だけ・retry・ignore・assert の緩和はしていない。既存の試験は削除・変更していない（追加は 3 件）。

## 実装前 brief（credential class の変更）

1. 根本原因: 渡された injection 接続の最初の要求を credentiald が固定の 5 秒しか待たない。要求が来る時刻は launcher の login 頁待ち（出来事）で決まる。
2. 責務 owner: credentiald の `serve_injection`（接続ごとの待ち）。
3. 既存の再利用先: `LiveRegistry` の auth section（daemon が control.sock で開閉し、injection の受付とは別 thread）、`process_start` による controller の照合、`read_request`・`IO_TIMEOUT`。
4. 選択肢: (a) launcher protocol を分け、注入の直前に daemon が接続する（protocol 版の更新・daemon と launcher の両方の変更・ADR の付記が要り、この unit の write set の ADR 外）。(b) `IO_TIMEOUT` を伸ばす（禁止の「timeout を伸ばすだけ」）。(c) 採用: 待ちを auth section と peer の close に結び付けた出来事待ちにし、保険を launcher の action 期限に揃える。auth section を持たない peer の扱いは変わらない。
5. 新しい状態・分岐・fallback・port: 状態は無し（既存の auth section を読むだけ）。分岐は「`IO_TIMEOUT` の間に何も来ず、peer が開いた auth section を持つなら待ち続ける」の 1 つ。定数 `PASSED_STREAM_BACKSTOP` を 1 つ足した。
6. 撤去対象: 無し（置き換えた旧い経路は無い。`serve_injection` の固定 5 秒の待ちは `await_first_request` に置き換わった）。
7. 不変条件: 上の「足した試験が守る不変条件」。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo test -p task-worker --lib launcher_login_regression_` → 1 passed（plan の check と同じ grep で exit 0）
- `cargo test -p task-worker --lib launcher_run::tests::launcher_login::` を 3 回 → 3 回とも 7 passed; 0 failed
- `cargo test -p celeris-credentiald --lib first_request_tests` → 2 passed（16.0 秒）
- `cargo fmt --all -- --check` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0
- `bash scripts/dev/test-affected.sh` → TEST_AFFECTED_RESULT
- `sh scripts/dev/progress-index.sh --check` → PROGRESS_CHECK_RESULT

## 提案

- 同時に 2 つの credential login が走ると、credentiald の injection の受付は 1 接続ずつなので、後の login の要求は前の login の password 欄が出るまで待つ（修正前も最大 5 秒待っていた）。同時の login が要るようになったら、injection の受付を接続ごとの thread にするかを別に決める。
- launcher protocol を分けて注入の直前に接続する案（brief の (a)）は、credentiald の「接続してすぐ書く」前提に戻せる。今回は採らなかった。

## 未解決

- 無し（userns・実 browser の全体試験は release gate の担当）。
