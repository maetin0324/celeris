---
title: 健全性v2 R2 unit launcher-login-fix — settings 段で落ちた launcher login 試験の原因
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
status: blocked
updated: 2026-11-11
---
# 健全性v2 R2 unit launcher-login-fix — settings 段で落ちた launcher login 試験の原因

task 01M6TYVJT0Z1CBGNK8VTNK8ZJH の WorkUnit launcher-login-fix の記録。基準は `$CELERIS_WU_BASE`。

## 結論

- 原因は本番の期限の食い違いで、負荷はそれを表に出しただけ。daemon（`launcher_credential_login`、`crates/task-worker/src/browser_launcher_run.rs`）は `authenticate` の前に credentiald の `injection.sock` へ接続し、その fd を launcher に渡す。launcher（`run_login`、`crates/task-worker/src/browser_launcher/backend.rs`）が注入の要求を書くのは、login の navigation と password 欄の待ち（login navigation の期限 15 秒）の後になる。credentiald（`serve_injection`、`crates/celeris-credentiald/src/ipc.rs`）は accept した接続に `IO_TIMEOUT`（5 秒、`injection_ipc.rs`）の read timeout を付ける。このため、接続から要求までが 5 秒を超えると要求を読む前に接続を閉じる。
- adapter-settings と browser-port の退行ではない。接続してから `authenticate` を呼ぶ順は 2026-10-09 の commit『launcher 経路の承認後 Authenticate を本番で通る形にする（protocol v4）』からある。credentiald の 5 秒は 2026-09-30 の commit『phase 4 (P4-B ipc): injection-only IPC …』からある。browser-port が `browser_launcher_run.rs` で変えたのは `sink` から `control`・`authority` への置き換えだけで、接続・`authenticate`・credentiald の箇所は変えていない。
- 直すには credentiald の要求待ちの期限を変えるか、launcher protocol を変える必要がある。前者は受け入れ条件 4 が禁じる timeout の延長にあたり、後者は protocol の設計変更になる。どちらも高 risk class `credential`（`^crates/celeris-credentiald/`）の設計判断なので、この unit ではコードを変えず plan_issue と人の決定（下の「提案」）で返す。

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

回帰試験 `launcher_login_regression_slow_password_page_still_injects_through_the_passed_stream`（mod `launcher_login`）を書き、現行のコードで 3 回流して 3 回とも落ちることを確かめた（9.6 秒・10.3 秒・7.6 秒。どれも `left: Recorded(Err("launcher_authenticate_rejected"))`）。

```
cargo nextest run -p task-worker --lib -E 'test(/launcher_login_regression_/)'
```

直す前に落ちる試験を main 向けの branch に入れると統合検査が落ちる。このため試験は commit せず、差分を run の成果物 `launcher_login_regression.patch` に残した。ignore や should_panic を付けて入れることもしない。直した後に通ることは未確認（直していない）。

試験が守る不変条件: IdP が password 欄を出すまでの時間が launcher の login navigation の期限（15 秒）の内にあれば、daemon が先に開いて渡した injection の接続で注入でき、lease はちょうど 1 回 consumed になる。

既存の試験で検出できなかった理由: `launcher_credential_login_navigates_injects_submits_and_consumes_the_granted_lease` などの fixture の IdP はすぐ応答する。単独では接続から要求まで約 1〜3 秒で、5 秒に届かない。IdP を「daemon の接続の後、注入の要求の前」に止める試験は無かった。credentiald 側の試験（`call_raw` 等）は接続してすぐ要求を書くので、渡した接続が待たされる形を通らない。

## 変えていないもの

- この unit はコードを変えていない（変更はこの記録だけ）。`LauncherLimits::default` の action 120 秒、`browser_cdp_sink.rs` の `TIMEOUT` 5 秒・`AGENT_TIMEOUT` 25 秒・`FETCH_TIMEOUT` 40 秒、期限切れの `Rejected` と teardown、credentiald の `IO_TIMEOUT` はどれもそのまま。
- 一時の計測（`run_login` と `CdpController::call` の eprintln、exp_ 試験）は `git checkout` で戻した。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-worker --lib -E 'test(/launcher_login::/)'` → 6 passed（変更前の tree。単独で本試験は 3.48 秒）
- 回帰試験（patch を当てた tree）を 3 回 → 3 回とも FAIL（上記）
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check` → 結果は run の報告に書く

## 提案（人の決定 `launcher-injection-stream-deadline`）

- a. launcher protocol を分ける（推奨）: navigation と password 欄の待ちを、fd を渡さない段（`auth_begin` の拡張か新しい要求）に移す。daemon は注入の直前に `injection.sock` へ接続して fd を渡す。credentiald の 5 秒はそのまま残り、「接続してすぐ要求を書く」という credentiald の前提に戻る。protocol の版の更新と旧 launcher への扱い（ADR 2026-10-09 launcher Authenticate 付記への付記）が要る。
- b. credentiald の要求待ちを launcher の待ちに合わせる: injection 接続の最初の要求の read 期限を、launcher の注入前の上限（login navigation 15 秒 + `Page.navigate` の 5 秒）から導く。sink 応答の 5 秒は残す。変更は小さいが timeout の延長であり、credentiald が接続を持つ時間も延びる。ADR-0109 D1 への付記と `credential` class の設計 review が要る。
- c. 変えない: IdP の表示が約 5 秒を超える login は本番でも `Rejected`（fail-closed）のまま残る。試験は高負荷の host で今後も落ちうる。

## 未解決

- 原因の修正（上の決定の後に、別の unit か子 task で行う）。修正の受け入れ条件には、この回帰試験が直す前に落ち、直した後に通ることを入れる。
