---
title: "credential login: IdP の同意頁を controller が固定ボタンで押す（launcher protocol 7）"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
---

# 同意頁の固定ボタン（2026-10-10）

- branch: `ops/postlogin-consent`（main 0130a45a から）。決定は ADR 2026-10-09 credential username / post-login の付記 2026-10-10b。
- 入れたもの: site policy の `consent {selector, choice_selector?}`（migration 0065、API・vault の写し・TrustedLogin・grant の照合・
  承認画面・doctor・web の編集）、controller の `press_consent`（IdP の top document・同意頁・同じ form のちょうど 1 個の submit と
  radio・1 login 1 回）、同意 form の欄名の診断（name・type・value だけ、ASCII・64 文字・12 個・512 文字）、launcher protocol 7
  （`consent`・`report_consent_controls`、応答の `consent_pressed`・`consent_controls`）。
- 試験の fixture: 他 origin の download の本文を 8 秒遅らせた（高負荷で取消より先に本文が届き、取消試験が揺れたため。
  controller の挙動は変えていない）。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` → exit 0、5024 passed / 0 failed / 14 ignored
  （その前の 1 回は load の高い時間帯に daemon の dispatch 系 e2e 33 本が 60〜120 秒の timeout で落ちた。browser と無関係で、
  load 12 での再実行は 0 failed）
- `cargo clippy --workspace -- -D warnings` exit 0（`--all-targets` も 0）、`cargo fmt --all -- --check` exit 0
- `launcher-admission-evidence.sh --credential` → `EXIT: 0`（credential-unit 36、credential-broker 5）。1 回目は load 60 で
  `launcher_credential_post_login_pair_login_then_reads_only_the_lms` の他 origin の download 待ち（10 秒）が切れて失敗 →
  待ちを 30 秒・fixture の本文の遅れを 8 秒にし、cargo test 形で 4 回・evidence 2 回 pass
- web: vitest 644 passed、server の node test 78 passed、typecheck exit 0、lint 0 errors、check:secrets OK、
  `e2e/browser/settings.spec.ts` 3 passed
- 新しい試験: `daemon_post_login_presses_the_pinned_consent_button_once`、
  `daemon_post_login_consent_not_pressed_twice_and_diagnostics_hold_no_page_data`、
  `launcher_credential_post_login_presses_the_pinned_consent_once_and_resumes`、`consent_controls_are_sanitized_and_bounded`、
  core / broker / protocol / doctor / web の consent の単体試験

## 未解決事項

- 実頁の consent の selector は、同意頁で止まった run の progress（`consent_controls`）から運用者が決める。
