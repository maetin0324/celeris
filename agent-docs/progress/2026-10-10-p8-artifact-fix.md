---
title: "launcher protocol 8 の screenshot・download 失敗: Chrome for Testing の PDF viewer と、失敗の固定診断"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# p8 artifact fix（2026-10-10）

- branch: `ops/p8-artifact-fix`（main fe20a5a7 から）。決定は ADR 2026-10-09 credential username / post-login 付記 2026-10-10f。
- protocol の版は 8 のまま（wire の型は変えていない）。protocol 9 は Live View（01M4JAK3MY）に予約。

## 原因（確かめたこと / 見立て）

- 確かめた: 本番の launcher の Chrome は Chrome for Testing 153（`/opt/celeris-browser/chrome/chrome`）で、PDF viewer を持つ。
  daemon 経路と試験は chrome-headless-shell（viewer なし）。inline 配信の PDF の link の download は、Chrome for Testing では
  tab が viewer に移って download が始まらず、agent-browser が 30 秒で timeout する。実 sandbox・ログイン後 mode で再現
  （修正を外すと `download Inline PDF: {"error_class":"timeout","runner_reason":"agent_browser_exit","status":1,…}`）。
- 確かめた: 実 launcher（本番 socket、uid 995・subuid）で、ログイン前の about:blank の screenshot と `fetch_artifact` は通る
  （一時的な診断用 test、commit には入れていない）。session dir の権限・UMask・0644 化は原因ではない。
- 見立て（未確定）: 本番の 5 件のうち速い失敗（0.16 秒差の連続・screenshot）は、tab が viewer か read_origins 外（PDF の配信が
  別 host へ redirect）に残りログイン後の gate が拒否したもの。fixture の同一 origin の viewer では screenshot は通ったので
  断定できない。launcher の journal に action の失敗が何も残らない作りだったため、本番の記録からは分からない。

## 直したこと

- sandboxd: Chrome 起動前に新しい profile の `Default/Preferences` に `always_open_pdf_externally: true`（PDF は常に download）。
- 診断: action runner の `runner_reason` / `error_class`、controller の relay 拒否の記録（最後の 8 件、`download_origin_denied`・
  `download_breach` を含む）、launcher の失敗 1 行（`code= launcher_reason=|status= runner_reason= error_class= gate=`）、
  session policy の拒否と action 期限切れの 1 行、daemon の理由の分割（`browser_artifact_action_refused` / `_timeout` /
  `_isolation_failed` / `_launcher_unavailable`、`count_limit`、それ以外は従来の `browser_artifact_action_failed`）。
- 試験: `tests/browser_sandbox_artifacts.rs`（実 bwrap + sandboxd + runner + 実 agent-browser 0.38.1 + HTTPS fixture、private netns）
  - `real_sandbox_launcher_chrome_downloads_inline_pdf_after_login`: Chrome for Testing・ログイン後 mode。screenshot、attachment /
    inline / 同一 origin redirect の PDF の download、download 後の screenshot、別 origin への redirect の download が取消され
    `Browser.downloadWillBegin!download_origin_denied` が記録され tab が使えること、失敗の `runner_reason` / `error_class`。
  - `real_sandbox_screenshot_and_pdf_download_reach_the_output_dir`: headless-shell・通常 mode。
  - 単体: `agent_denials_are_bounded_fixed_and_include_cancelled_downloads`、
    `launcher_action_failure_detail_is_fixed_tokens_only`、
    `launcher_artifact_action_failures_keep_the_launcher_code_as_a_fixed_reason`。
- 運用: `docs/ops/browser-launcher-host-setup.md`「protocol 8 の修正」— launcher と sandboxd の両方を再 build・差し替え。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` → exit 0、passed 5046 / failed 0 /
  ignored 14、userns true。
- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require cargo test -p task-worker --test browser_sandbox_artifacts` → 3 passed（SKIP なし）。
  sandboxd の Preferences を外すと `real_sandbox_launcher_chrome_downloads_inline_pdf_after_login` が timeout で落ちることを確認。
- `cargo clippy --workspace -- -D warnings`・`--all-targets` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh crates/task-worker/scripts/launcher-admission-evidence.sh --credential <log>` → EXIT[credential-unit] 0、EXIT[credential-broker] 0。

## 未解決事項

- `target="_blank"` の link からの download は agent-browser が拾えず 30 秒 timeout（headless-shell でも同じ）。PDF の URL の `open` も
  download になり file は渡らない。manaba の link がこの形かは未確認。
- 本番の速い失敗の原因は、差し替え後の journal の `gate=` / `error_class=` で確定させる（launcher の journal は root でしか読めない）。

## 提案

- launcher の Chrome と daemon 経路の Chrome の種類を揃えるか、doctor に Chrome の種類（viewer の有無）を出す。
- agent への prompt に「PDF の link が download できなければ URL を open」の指針を足すかは、本番の診断を見てから決める。
