---
title: "ログイン後の download @ref を controller が href を頁の文脈で取りに行く（人の決定）"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# download fetch href（2026-10-10）

- branch: `ops/download-fetch-href`（main e65571fe から）。決定は ADR 2026-10-09 credential username / post-login 付記 2026-10-10m（人の決定）。
- protocol は 9 のまま（launcher・controller・runner の中の変更。daemon・shim は変わらない）。

## 直したこと

- launcher（ログイン後の `download`）: runner に ref を読ませ（`phase=resolve`）、controller が agent-browser の解いた要素
  （`DOM.resolveNode` の `backendNodeId`）を控える。`fetch_href`（`browser_launcher::backend`）が controller に href を取りに行かせ、先頭 byte の
  型を確かめて session の output に書き、protocol 8 の受け渡しで届ける。href が無い・要素を控えられない → click（`phase=click`）。
- controller `fetch_captured_download`: 頁の検査（read_origins・password 欄）→ 自分の isolated world で要素か祖先の `<a href>` の絶対 URL →
  origin が read_origins → `fetch(href, {credentials:'same-origin', mode:'same-origin'})`（最初は `redirect:'manual'`、redirect なら
  `follow`。他 origin への redirect は request 前に失敗 → `redirect_denied`）→ 本文を 10 MiB まで数えて読む → 1 MiB ずつ base64 で取り出す。
  失敗は固定 token（`href_missing`・`ref_unresolved`・`origin_denied`・`redirect_denied`・`size_exceeded`・`type_denied`・
  `fetch_failed_<n>xx`・`fetch_failed_network`・`fetch_failed_timeout`・`fetch_failed_script`・`page_denied`）。
- timeout の詳細: launcher の runner 待ちの期限切れも screenshot / download なら token を agent に返す（`path=`・`link_read=`）。
- tab の回復: runner は click の download が待って諦めたら agent-browser の daemon を止める（`recovery=daemon_restarted`）。次の snapshot は
  頁を開き直さずに通る。runner の各段の期限は launcher の 50 秒より短い（link の読み取り 30 秒・click 38 秒）。

## 試験

- 実 sandbox（`browser_sandbox_artifacts.rs`、Chrome for Testing、ログイン後 mode、Live View を流しながら）: 同一 origin の PDF（通常・7 秒遅れ・
  Content-Disposition attachment・同一 origin の redirect・`<a href>` の中の button）を `fetch_href` で取得、他 origin への redirect は
  `redirect_denied`、HTML は `type_denied`、10 MiB 超は `size_exceeded`、href の無い `role=link` の要素は `href_missing` で click に落ちて
  click の download が通る。新しい tab の他 origin の download が待って諦めた後、runner が daemon を止め、同じ tab の snapshot が通る。
- 単体: `fetch_href_needs_login_and_a_captured_element`。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` を 2 回 → どちらも exit 0、
  `5111 tests run: 5111 passed (1 slow), 13 skipped`。
- `cargo test -p task-worker --test browser_sandbox_artifacts -- --test-threads=3`（gate env）を 3 回 → すべて 3 passed。
- `cargo test -p task-worker --lib` → 1066 passed。
- `cargo clippy --workspace -- -D warnings`・`--all-targets` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `launcher-admission-evidence.sh --credential` → EXIT[credential-unit] 0、EXIT[credential-broker] 0。

## 運用

launcher を再 build して差し替える（controller・runner は launcher の process / 埋め込み）。daemon も同じ commit（変更は無いが揃える）。
sandboxd は不要。

## 未解決事項

- Content-Disposition・URL の file 名は protocol に欄が無いので渡していない（shim が先頭 byte で `.pdf` などを付ける）。
- text は既存の artifact の型に無いので通さない（`type_denied`）。
- manaba の file link が実際にどの形か（`href` の有無）は本番で確かめる。journal の `path=`・`fetch=`・`link=` で分かる。
