---
title: "launcher runtime の browser がどの site にも届かない（egress の wildcard・dual-stack・CNAME NODATA）"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-09
---

# launcher runtime の egress が全拒否

- 完了日: 2026-10-09（コード。本番の egress binary 差し替えは operator 作業）
- 症状: release 2cb9cfe4e782、task 01M4GYJ3XGJNWZQDF35F1MDE0H の run 01M4H0CJR22Z2T1KZ4M216D5V5（launcher session
  31425e060badcfe7acc96ed1ef79fa3a）で `https://manaba.tsukuba.ac.jp/`・`https://www.tsukuba.ac.jp/` が毎回
  「This site can't be reached」。
- 原因（ADR [2026-10-09-egress-wildcard-origin](../adr/2026-10-09-egress-wildcard-origin.md)）:
  1. task の許可は `https://*.tsukuba.ac.jp`（`browser_task_policies` と task requirements）。egress 許可は
     `*.tsukuba.ac.jp:443` になるが `check_egress` は完全一致だけ → `not_allowed`。
  2. dual-stack の名前は `ipv6_disabled` で全拒否。
  3. CNAME + AAAA NODATA（CloudFront）が解決失敗。
- 証拠（本番 binary を uid 995 で socketpair 越しに直接起動、policy は stdin）:
  - 旧 `/usr/local/libexec/celeris/celeris-browser-egress`、allow `*.tsukuba.ac.jp:443`、CONNECT manaba → 403、
    stderr `{"kind":"not_allowed","host":"manaba.tsukuba.ac.jp","port":443}`。
  - 旧 binary、allow `manaba.tsukuba.ac.jp:443` → 200（中継・DNS・host の外向き通信は正常）。
  - 旧 binary、`www.tsukuba.ac.jp`（完全一致許可）→ 403（解決失敗）、`example.com` → `ipv6_disabled`。
  - 新 binary（本ブランチ）: manaba・www（wildcard 許可）・example.com → 200、apex `tsukuba.ac.jp`・`evil.com` → 403
    `not_allowed`。
  - sandboxd・egress・relay の source は installed binary（2026-10-09 01:54）以降変更なし。protocol 不一致ではない。
- 試験: `cargo test -p task-core --lib egress_` → 11 passed。`cargo test -p task-worker --lib browser_egress` → 26 passed。
- 証拠: `bash scripts/dev/test-parallel.sh` → exit 0（nextest 4992 passed、0 failed、14 ignored）。
  `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 未解決: allow_ipv6=true の場合の AAAA 側も D3 で救われるが、本番は常に false。台帳の egress_negative_suite は拒否しか
  見ないので、許可側（wildcard 下位 host への到達）の正の検査が無い。
- 提案: 台帳 / launcher 統合試験に「wildcard 許可の下位 host へ CONNECT が通る」正の検査を足す。
