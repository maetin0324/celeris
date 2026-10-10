---
title: "release gate の不定 2 件（898929d9）: 他 origin の取消の競争、Unknown ref"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# download blank target 2（2026-10-10）

- branch: `ops/download-blank-target2`（main 898929d9 から）。決定は ADR 2026-10-09 credential username / post-login 付記 2026-10-10l。

## 原因と対処

- `real_sandbox_launcher_chrome_downloads_inline_pdf_after_login`（screenshot が `Target.setDiscoverTargets` で失敗）: 他 origin の小さい
  PDF が relay の取消より先に書き終わり breach（設計どおりの fail closed）。fixture の他 origin の file を header 先行・本文 10 秒後にした。
- `real_sandbox_screenshot_and_pdf_download_reach_the_output_dir`（`Unknown ref: e30`）: agent-browser の ref の振り直し。再現せず。
  controller が答えた dialog の閉じた event も agent に渡さないようにし、試験は `unknown_ref` のときだけその段を 1 回やり直す。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` を 2 回 → どちらも exit 0、
  `5102 tests run: 5102 passed (1 slow), 13 skipped`。やり直し（`stale ref, snapshot again`）は出ていない。
- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require cargo test -p task-worker --test browser_sandbox_artifacts -- --test-threads=3` を
  4 回 → すべて 3 passed（変更前の slow body の修正の後にさらに 3 回、SIGSTOP stutter 下で 2 回も 3 passed）。
- `cargo clippy --workspace -- -D warnings`・`--all-targets` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `launcher-admission-evidence.sh --credential` → EXIT[credential-unit] 0、EXIT[credential-broker] 0。

## 未解決事項

- `Unknown ref` の原因は確定していない（runner の link の事前読み取りとの関係も未確定）。製品では agent に `error_class=unknown_ref`
  が返るので、agent は snapshot を取り直せる。
