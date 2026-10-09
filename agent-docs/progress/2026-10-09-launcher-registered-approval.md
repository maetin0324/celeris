---
title: "launcher 経路で登録済み credential が承認待ちにならない不具合の修正"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-09
---

# launcher 経路の Registered → WaitingForApproval

- 完了日: 2026-10-09
- 症状: `[browser] runtime = "launcher"` で credential_request → 人が登録（wait `registered`）→ 再開 run が
  承認 wait を開かず harness を再起動し、ログイン画面で再び credential_request を出し続けた。
- 原因: `browser.rs::run_with_executable_attempt` の launcher 分岐が `Registered` の処理（承認 wait を開く段）より
  前に `launcher_run::run` に return していた。`launcher_run::run` は `Approved` の credential_use だけを見る。
- 修正:
  - `Registered` → `WaitingForApproval`（operation `credential_use`、resume key `approval:<wait_id>`、
    `describe_policy` で固定した trusted login）を `browser.rs::registered_credential_approval` に切り出し、
    daemon 経路と launcher 経路（launcher に接続する前）の両方で同じ照合・fail closed で呼ぶ。
  - 承認済み credential_use の policy 照合を `check_approved_credential` に切り出し、launcher 経路でも接続前に通す。
  - launcher 経路の承認済み credential_use の消費を `browser_operation_approval_consume`（credential_use を常に拒否）
    から `browser_approval_consume` に直した。消費失敗時は session を stop して拒否。
  - 再開 run の論理 session は承認 wait の `session_id`（daemon 経路と同じ）。
  - 承認方針（click/download は承認不要、credential_use は承認必須）は変えていない。

## 証拠

- 修正前（呼び出しを無効化）で新試験 2 件が失敗、修正後は成功。
- `cargo nextest run -p task-worker -E 'test(/launcher/)'`: 105 passed
- `bash scripts/dev/test-parallel.sh`: 4995 passed, 0 failed, 14 ignored, exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require cargo nextest run -p task-api --test browser_h3_injection`: 3 passed

## 未解決事項（読んで見つけた、launcher の承認後 Authenticate 経路の欠落。未修正）

- credentiald の H3 lease が発行されない: launcher 経路は `AuthenticateArgs.lease_id` に launcher session の lease を
  渡すが、credentiald の注入は `grant_h3_lease`（broker.bind / grant）で発行した lease を
  `consume_for_injection` で消費する。launcher 経路は grant していないので `lease_invalid` になるはず。
- lease の session 束縛: `grant_h3_lease` は wait の `session_id` に束縛するが、launcher の注入要求の session は
  launcher が採番した `runtime.session_id`（credentiald に登録した session）。lease をどちらの session に束縛するかの設計判断が要る。
- launcher backend の Authenticate は `Page.navigate` に `args.origin` を使い、trusted login の `login_url` を使わない
  （`AuthenticateArgs` に欄が無い）。ログイン form が origin の root に無いサイトでは timeout になる。
- launcher backend は `submit_selector` を扱わない（password を入れるだけで送信しない）。daemon 経路は requestSubmit する。
- launcher 経路は `browser_auth_section`（store の H3 区間の記録）を立てない。

## 提案

- 上の lease 発行・session 束縛・login_url・submit を launcher Authenticate に揃える ADR（2026-10-09 launcher credential
  release の付記）を先に書いてから実装する。本番での credential login は、それまで launcher runtime では成立しない見込み。
