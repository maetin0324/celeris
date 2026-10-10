---
title: "download の click が sink_failed（dialog ではない）: 応答の遅い navigation の間 controller が 5 秒で諦めていた"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# download click fix 2（2026-10-10）

- branch: `ops/download-click-fix2`（main f42a181b から）。決定は ADR 2026-10-09 credential username / post-login 付記 2026-10-10i。
- protocol は 9 のまま。

## 原因

- 本番（15:48:02Z）: `gate=Input.dispatchMouseEvent!sink_failed`、dialog の記録なし、Live View は流れていた。
- fixture で確認: click で始まった navigation の応答が遅いと（7 秒）、Chrome はその頁への command に応答が届くまで答えない。
  controller の 5 秒の期限で、click 本体かログイン後の gate の検査が失敗する（修正前の試験は
  `CDP error (Input.dispatchMouseEvent): observation_origin_denied`、`gate=Controller.reply!cdp_reply_timeout,…document_response_pending` で落ちる）。
- 試験の controller は 60 秒の試験用期限（`response_timeout_for_test`）で動いていたので、これまで見えなかった。試験からこの上書きを外した。
- Live View の screencast は原因ではない（流しても流さなくても同じで、修正後は流したまま通る）。

## 直したこと

- agent の command（と gate の検査）は最大 25 秒待つ。期限切れは `cdp_reply_timeout` と、待つ間に見えたものの固定 token を記録する。
- 試験: `agent_reply_timeout_records_pending_navigation_as_fixed_tokens`（単体）。`browser_sandbox_artifacts.rs` は launcher と同じく
  `LiveTap` と `ScreencastFeed` で Live View を流しながら、7 秒遅れの PDF の download（直接の link と mousedown で navigation する link）を
  ログイン後 mode・Chrome for Testing と headless-shell で確かめる。

## 運用

launcher を再 build して差し替える。daemon も同じ commit。sandboxd は不要。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` → exit 0、passed 5101 / failed 0 /
  ignored 14、userns true。
- `cargo test -p task-worker --test browser_sandbox_artifacts`（gate env）→ 3 passed。`AGENT_TIMEOUT` を 5 秒に戻すと
  `download Slow PDF: … observation_origin_denied`・`gate [("Controller.reply", "cdp_reply_timeout"), …]` で落ちる。
- `cargo clippy --workspace -- -D warnings`・`--all-targets` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `launcher-admission-evidence.sh --credential` → EXIT[credential-unit] 0、EXIT[credential-broker] 0。

## 未解決事項

- 25 秒を越える応答の download は失敗する（journal の token で分かる）。
- 本番の manaba の応答時間は測っていない。

## 提案

- 応答が遅い site のために、controller が read_origins 内の `<a href>` を頁の文脈で取りに行き artifact の経路で渡す方式
  （cookie を agent に出さない）を、人の決定を経て検討する。
