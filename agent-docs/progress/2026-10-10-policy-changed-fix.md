---
title: "credential 登録後に毎回 policy_changed: 登録 wait の origin を site policy に結び付け、使えない登録で止まらない"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# policy_changed fix（2026-10-10）

- branch: `ops/policy-changed-fix`（main b865201c から）。決定は ADR 2026-10-09 credential username / post-login 付記 2026-10-10g。

## 原因（読み取りだけで確認）

- prod DB `browser_waits`: 13:42 の wait 01M4K0SATAY54M4NM96E6R5WS8 の origin が `https://manaba.tsukuba.ac.jp`。それ以前の
  成功した登録 wait は全部 `https://idp.account.tsukuba.ac.jp`。site policy `manaba-tsukuba` の `exact_origin` は IdP。
- run 01M4K0R802F3HKH878BXMAWVT1 の events: navigate → extract → `credential_request`。agent が LMS の origin を名指しした。
- vault の非秘密の欄（`/local/celeris/state/credentiald/vault/cred-01M4K0SATAY54M4NM96E6R5WS8.json`）: `exact_origin` が LMS、
  `login_url` なし（504 byte。成功した登録は 1026 byte で selector 等あり）。秘密の欄は読んでいない。
- task-api の登録は site policy を `policy_id` と `exact_origin` の一致で引くので、合わない origin では selector 無しで登録された。
  credentiald の `describe_registered` は `login_url` が無いと `denied` → daemon の `policy_changed`。
- release（p8 の修正・selfdeploy の prune）とは無関係。credentiald の再起動も原因ではない。

## 直したこと

- store: 登録 wait の origin を site policy の `exact_origin` に結び付ける（`read_origins` の名指しも可、他は拒否）。
- daemon: `policy_changed (<sub-reason>)` の固定語彙と warn log、pin した login のどの欄が変わったか、wait を開けないときの
  store の code。
- daemon: credentiald が登録を使えないと答えたら承認 wait を開かず run を続ける（task が登録済み wait で止まらない）。
- 試験: `credential_wait_is_bound_to_the_site_policy_login_origin`（task-core）、
  `registered_credential_describe_failures_carry_sub_reasons_and_unusable_one_is_skipped`、
  `trusted_login_difference_names_the_changed_field_only`（task-worker）。既存の 2 試験の期待文言を sub-reason 付きに更新。

## 運用（この task の回復）

1. この branch を含む release に daemon を更新する（launcher・sandboxd・credentiald の再 build は不要）。
2. task 01M4GYJ3XGJNWZQDF35F1MDE0H を再開する。次の run は使えない登録（wait 01M4K0SATAY…）を飛ばし、progress に
   `browser.credential: registered credential unusable (describe_denied)` が出る。
3. agent が credential を頼み直すと wait の origin は IdP になる。人がそこに登録し直し、使用を承認する。

更新しないままでは、登録済みの wait は閉じないので task は同じ失敗を繰り返す（API に登録済み wait を閉じる口は無い）。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` → exit 0、passed 5069 / failed 0 /
  ignored 14、userns true。
- `cargo clippy --workspace -- -D warnings`・`--all-targets` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決事項

- 登録 API は、site policy があるのに origin が合わない wait（この修正より前に開いたもの）を selector 無しで登録してしまう。
  新しい wait は store で結び付くので起きないが、登録 API でも拒否するかは提案に回す。
- 使えない登録の wait は `registered` のまま残る（閉じる状態遷移は足していない）。

## 提案

- 登録 API で、site policy があり origin が合わないときは 409 にする。
- 登録済み wait を人が取り消せる操作（`invalidated`）を足す。
