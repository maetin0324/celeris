---
title: "credential login v5: 本番の post_login_unconfirmed の対処（IdP の中継を通す待ち・理由の固定 code・protocol 6）"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
---

# post-login の待ちと理由（2026-10-10）

- branch: `ops/v5-postlogin-fix`（main 93cb3a36 から）。決定は ADR 2026-10-09 credential username / post-login の付記 2026-10-10。
- 原因の見立て: 本番 run 01M4HR6PY6NJXSPWTW9X5AZ81Y は session 開始から login の記録まで 16 秒で、login と 15 秒の待ちの両方には
  足りない。待ちは一時的な CDP 失敗（`post_login_ready` の `Err`）を即打ち切っていたので、期限前に終わったか、IdP の中継
  （localStorage の interstitial・同意頁・SAML の自動 POST）で期限を迎えたかのどちらか。どの条件かの記録が無かった。
- 直したこと: 状態の分類（`PostLoginProbe`）、最長 60 秒の待ち（中間状態と一時失敗は待ち続け、同意頁と login form の再表示は
  3 秒続けば終える）、理由の固定 code と top origin の分類を progress へ、launcher protocol 6（`held_reason`、
  `report_held_reason` で旧 daemon と併存）、Shibboleth 形の fixture（前後の interstitial と SAML の自動 POST、同意頁）。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` → exit 0、5020 passed / 0 failed / 14 ignored
- `cargo clippy --workspace -- -D warnings` exit 0（`--all-targets` も 0）、`cargo fmt --all -- --check` exit 0
- `sh crates/task-worker/scripts/launcher-admission-evidence.sh --credential` → `EXIT: 0`（credential-unit 35、credential-broker 5）
- 新しい試験: `daemon_post_login_waits_through_slow_shibboleth_hops`（ログイン後の hop 2 つ各 9 秒、15 秒超えでも区間が閉じる）、
  `daemon_post_login_unconfirmed_keeps_observation_stopped_and_names_the_reason`（同意頁・login form 再表示は早く終わる、
  password 欄・他 origin は期限で、それぞれの理由）、`launcher_credential_post_login_consent_page_is_held_with_its_reason`
  （実 credentiald・実 launcher protocol で `consent_required`）、`post_login_waiter_waits_through_hops_and_names_the_failed_condition`、
  `post_login_read_with_the_production_task_policy_shape_enables_reading`。

## 未解決事項

- 本番で同意頁が毎回出る場合（同意の記録が browser 側の IdP）、ADR の範囲では自動では閉じない（`post_login_consent_required`）。
  同意を controller に押させる（site policy に管理者の同意 selector を pin する）かは人の判断。

## 提案

- 同意頁を人の承認の下で controller が固定 selector で押す「管理者 pin の同意」の ADR。
