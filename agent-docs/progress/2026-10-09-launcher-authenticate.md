---
title: "launcher runtime の承認後 Authenticate 経路（credential login）を本番で通る形にする"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-09
---

# launcher の Authenticate 経路

- 完了日: 2026-10-09
- branch: `ops/launcher-authenticate`（main d5a39326 から）
- 決定: [ADR 2026-10-09 launcher credential release](../adr/2026-10-09-browser-launcher-credential-release.md) の
  「付記 2026-10-09: launcher の Authenticate 経路」

## 直したこと（前 task の未解決 5 件 + 読んで見つけた 1 件）

1. **lease が発行されない** → 承認消費後に daemon が `grant_h3_lease` で lease を発行する
   （`browser_launcher_run.rs::launcher_credential_login`）。
2. **session 束縛の食い違い** → `grant_h3_lease` に束縛先の稼働 session を引数で渡す。launcher 経路は launcher が採番した
   `runtime.session_id`（credentiald に登録した session、注入要求が名指す session）、daemon 経路は従来どおり wait の
   `session_id`。store 側（`BrowserRun`・`browser_auth_section`）は論理 session（wait の `session_id`）のまま。
3. **login_url 未使用** → protocol v4 の `authenticate` に `login_url`・`password_selector`・`submit_selector`・
   `credential_lease_id` を載せ、launcher は `login_url` へ navigate する（`backend.rs::run_login`）。
4. **submit なし** → daemon 経路と同じ `requestSubmit()`/`click()` と、元 document が消えるまでの待ち（≤5 秒）。
   username は daemon 経路も入れていない（下の未解決）。
5. **auth section 未記録** → `auth_begin` 成功後に store へ `browser_auth_section(true)`、worker の
   `LiveEmitter::auth_section` guard。解除は launcher session の stop 成功後だけ。記録失敗は lease revoke・stop・retryable error。
6. （新規に判明）**launcher は credentiald と話せない**: control は daemon の PID からだけ、injection は daemon UID かつ登録
   controller の PID だけ、socket dir は daemon UID の 0700、本番 `launcher.toml` に `injection_socket` 無し。→ control は
   daemon が行い、injection.sock への接続は daemon が `connect` して FD を `SCM_RIGHTS` で launcher に渡す。launcher の
   `injection_socket` 設定と `CELERIS_CREDENTIALD_INJECTION_SOCKET` は削除。偽 broker が sink で任意の CDP を流せないよう、
   `CdpController::inject` は sink frame が credentiald の固定 `INJECT_FUNCTION` 呼び出しであることを照合する（両 runtime）。

ほか: launcher protocol を v4 にし（`auth_begin`/`auth_begun`、FD 付き `authenticate`）、daemon は承認消費前に `hello` で
v4 以上を確かめる（古い launcher は明示文言で拒否、承認は未消費）。credential 使用後の harness policy を daemon 経路と同じ
`credential_harness_policy` にした。run 終了時に credentiald の live session 登録を外す。`IdentityRestore` 拒否・
`credential_use` 承認必須・二段 gate・Attested admission は変えていない。

## 証拠

- 新試験（すべて `launcher_credential_` 接頭辞、userns・外部ネットワーク・CPU 負荷なし。Chromium は既存 SSO fixture と同じ
  Playwright headless shell + loopback HTTPS）:
  - `launcher_credential_login_navigates_injects_submits_and_consumes_the_granted_lease`: 実 `LauncherServer`（FD 渡しの
    実 socket）+ 実 Chromium + 実 credentiald（manual provider、`register` した site policy、SameUidHarnessFacts admission）。
    承認済み wait の消費結果から `launcher_credential_login` を通し、SP が受け取った password = secret（root に form が無い
    fixture なので `login_url` 経由でしか成立しない）、journal の `consumed` が 1 件で launcher session id 付き、store の
    auth section は論理 session に `true` 1 回だけ（stop 前は解除しない）、controller は観測停止のまま・guard 1、2 回目の
    `auth_begin` は拒否、secret は結果・store 呼び出し・journal のどれにも無い。
  - `launcher_credential_login_forged_sink_frame_never_reaches_chrome`: 偽 broker が sink に `Network.getAllCookies` を
    書くと Chrome に届かず、sink は応答なしで閉じ、login は失敗、section は再使用不可。
  - `launcher_credential_sink_accepts_only_the_fixed_injection_frame`: sink frame 照合の 16 変異をすべて拒否。
  - `launcher_credential_authenticate_fd_rules_fail_closed`、`..._requires_bounded_trusted_login_arguments`、
    `..._is_fixed_and_status_only`（v3 形・password 欄の拒否）、`..._backend_rejection_has_status_only_response`、
    `launcher_credential_authenticate_fake_backend_returns_status_only`（hello = v4、`auth_begin`、FD 付き authenticate）、
    `launcher_credential_login_old_launcher_message_is_explicit`。
- 変異確認（e2e 試験が元の欠落を検出するか、一時的に戻して実行し、元に戻した）:
  login_url → origin へ navigate: FAIL（15.3 秒 timeout）／submit 無し: FAIL（SP 未着）／lease を wait の session に束縛:
  FAIL（login が `Recorded(Err)`、credentiald は session 不一致で拒否）／store の auth section 記録なし: FAIL。
- `cargo nextest run -p task-worker --lib -E 'test(/launcher_login|fixed_injection/)'`: 4 passed
- `bash scripts/dev/test-parallel.sh`: exit 0、passed 4996 / failed 0 / ignored 14（175 binaries）。
  （1 回目は実行途中で TMPDIR（`/tmp/celeris-test-parallel.*/tmp`）が消え、無関係の 2586 件が 0.005 秒で NotFound 失敗。
  同時に `launcher_credential_login_forged_sink_frame_never_reaches_chrome` が競合で落ちた — sink frame を拒否したとき
  broker の終了を待たずに返していたため。拒否時も sink を閉じて `pending.finish()` を待つよう直し、3 回連続 pass、
  2 回目の全体実行で 0 failed）
- `cargo clippy --workspace -- -D warnings`: exit 0（`--all-targets` も 0）
- `cargo fmt --all -- --check`: exit 0
- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require cargo nextest run -p task-api --test browser_h3_injection`: 3 passed
- `sh crates/task-worker/scripts/launcher-admission-evidence.sh --credential <out>`: `EXIT: 0`
  （credential-unit: task-worker `launcher_credential_` 30 passed、credential-broker: prod_admission 5 passed）

## 実 launcher（root service）でしか確かめられないこと

- 実 launcher（UID 995、別 userns）の `auth_begin`/`authenticate` と、daemon が渡した injection 接続での credentiald の
  Attested admission（launcher 証明の再検証）→ lease 消費。同一 process の偽 launcher では responder UID = daemon UID で
  admission が成立しないため、`run()` の admission → 版確認 → 承認消費 → login の結線は host 実証で確かめる。
- launcher の bwrap 内 Chromium から `idp.account.tsukuba.ac.jp` への egress（task policy の許可 domain に入っていること）。

## 未解決事項

- **username 未注入**: daemon 経路も username を入れない（ADR-0110 未解決、1 section = 1 lease = 1 欄、site policy に
  `username_selector` 無し）。manaba の IdP form は `j_username`（type=text）と `j_password` が同じ頁にあるため、この変更
  だけでは manaba のログインは完了しない（password だけ入って送信され、IdP がエラーを返す見込み）。
- **ログイン後の観測停止**: ADR-0080 H3 により、注入した session では snapshot / extract / screenshot / download が session の
  終わりまで使えない。ログイン後の頁を読む用途（課題監視）はこの経路だけでは成立しない。

## 提案

- username 注入の ADR: site policy に `username_selector`（管理者 pin、ADR-0110 の文法）を足し、1 承認 = 1 auth section で
  username → password の 2 欄を lease 1 本（欄ごとに 1 回、順序固定）で消費する形を提案する。credentiald の lease 消費と
  `trusted_selector` 照合、wait の `TrustedLogin`、API・GUI の site policy 編集、migration が要る。
- ログイン後の観測: 課題監視には「認証後の頁の観測を許す条件」（redisplay guard 前提で H3 を注入完了で閉じる、など）の
  人の判断が要る。ADR-0080 H3 / 0099 / 0100 の変更になる。
