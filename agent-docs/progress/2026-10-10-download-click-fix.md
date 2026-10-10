---
title: "ログイン後の download の click が sink_failed: JavaScript dialog を controller が答える"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# download click fix（2026-10-10）

- branch: `ops/download-click-fix`（main 342ec107 から）。決定は ADR 2026-10-09 credential username / post-login 付記 2026-10-10h。
- protocol は 9 のまま。

## 原因

- 本番の launcher journal（coordinator から）: `action download failed: … error_class=cdp_command_failed
  gate=Input.dispatchMouseEvent!sink_failed`。`sink_failed` は controller が `Input.dispatchMouseEvent` の返事を 5 秒待って諦めた。
- fixture で再現: click で alert / confirm / beforeunload の dialog が開くと Chrome は dialog が閉じるまで click に答えない。relay は
  agent の command を 1 つずつ通すので agent-browser は dialog を閉じられず、修正前は `CDP command timed out:
  Input.dispatchMouseEvent`・`gate=Input.dispatchMouseEvent!sink_failed` になる（修正を外すと新しい試験がこれで落ちる）。
- dialog の無い link（`?view=full` の相対 URL、7 秒遅れの応答、3 MiB、`download` 属性、同一 origin の redirect）は修正前でも通る。
  target=_blank は 30 秒 timeout（既知、`sink_failed` ではない）。
- manaba の link のどの dialog かは未確認（本番の頁は見ていない）。

## 直したこと

- `CdpController::resolve_agent_dialog`: agent の頁の dialog に controller が答える（alert・beforeunload は受け入れ、confirm・prompt は
  退ける）。種類だけを `gate=` に記録し、開いた event は agent に渡さない。private session は触らない。
- 試験: `agent_page_dialogs_are_answered_with_fixed_defaults_and_private_ones_left_alone`（単体）、
  `browser_sandbox_artifacts.rs` に alert・beforeunload の link の download（通る）と confirm の link の click（退けて tab が止まらない）を
  ログイン後 mode・Chrome for Testing と headless-shell の両方で。

## 運用

launcher を再 build して差し替える（controller は launcher の process の中）。daemon も同じ commit にする。sandboxd は不要。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` → exit 0、passed 5100 / failed 0 /
  ignored 14、userns true。
- `cargo test -p task-worker --test browser_sandbox_artifacts`（gate env）→ 3 passed。controller の修正を外すと
  `download Alert PDF: … CDP command timed out: Input.dispatchMouseEvent` で落ちる。
- `cargo clippy --workspace -- -D warnings`・`--all-targets` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `launcher-admission-evidence.sh --credential` → EXIT[credential-unit] 0、EXIT[credential-broker] 0。

## 未解決事項

- confirm で守られた download は通らない。manaba が confirm なら `gate=Page.javascriptDialogOpening!dialog_dismissed_confirm` が出る。

## 提案

- confirm を受け入れてよいのは `download` verb の間だけ、などの人の決定（launcher が action の verb を controller に渡せば
  protocol を変えずにできる）。
