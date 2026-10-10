---
title: "新しい tab で開く file link の download を opener で取る・download 失敗の固定診断を agent に返す"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# download blank target（2026-10-10）

- branch: `ops/download-blank-target`（main edccce6b から）。決定は ADR 2026-10-09 credential username / post-login 付記 2026-10-10j。
- protocol は 9 のまま。controller が href を取りに行く代替は作っていない（人の決定）。

## 調べたこと

- 本番 18:05:31Z: `runner_reason=exec_timeout error_class=none gate=none`（relay・gate の失敗なし、agent-browser が download を見ないまま待った）。
- fixture: `target="_blank"` の link は新しい tab で download され、agent-browser も controller もその download を見ない（controller に届くのは
  `Page.windowOpen` と `Target.targetCreated`）。file は `/session/output/<guid>` に残る。同じ tab の遅い応答・中継頁は通る。
- manaba の link の形は未確認（次の失敗の `link=` token で分かる）。

## 直したこと

- controller: `download` の見張りの間、agent の頁が開いた新しい tab を閉じ、read_origins の URL なら opener を script でそこへ移す。
- runner: agent-browser が待って諦めたときだけ、完了した `<guid>` file を 1 つ採用。launcher が controller の記録で検証（違えば消して失敗）。
- 診断: link の静的な形の token、action 中に Chrome がしたことの token、launcher の失敗行、agent への `detail`。
- 同じ dialog を複数の session から受けたら一度だけ答える。

## 試験

- 単体: `download_watch_follows_a_new_tab_in_the_opener_on_read_origins_only`、
  `adopted_downloads_must_have_completed_through_the_post_login_rules`、`launcher_download_diagnostics_keep_fixed_shapes`、
  `launcher_shim_download_failure_carries_the_fixed_tokens_only`。
- 実 sandbox（`browser_sandbox_artifacts.rs`、Live View を流しながら）: target=_blank の PDF（速い・7 秒遅れ）と中継頁の download が
  agent-browser の保存で通り、tab が使えること。ログイン後は他 origin の target=_blank が追われず（`window_open_origin_denied`）、link の token
  （`target_blank`・`href_other_origin`・`path_pdf`・`onclick`）が出て、採用されても検証で拒否されること。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` → exit 0。nextest の Summary は
  `5102 tests run: 5102 passed (1 slow), 13 skipped`（summary 行の `passed` は `(1 slow)` の表記で 3 と誤って読まれている。script の
  集計の不具合で、試験の失敗ではない）。
- `cargo test -p task-worker --test browser_sandbox_artifacts`（gate env）→ 3 passed（2 回）。
- `cargo clippy --workspace -- -D warnings`・`--all-targets` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `launcher-admission-evidence.sh --credential` → EXIT[credential-unit] 0、EXIT[credential-broker] 0。

## 運用

launcher を再 build して差し替える。daemon も同じ commit。sandboxd は不要。

## 未解決事項

- 新しい tab の download が read_origins の URL から他 origin へ redirect する場合、opener で行うので取消は従来どおり効くが、最初の
  `Page.windowOpen` の URL が read_origins でなければ追わない（その link は失敗する）。
- agent-browser の CLI を runner が link の読み取りで複数回呼ぶため、download 1 回あたり 0.5 秒ほど遅くなる。

## 提案

- 失敗が続く場合は、次の journal の `link=` と `after=` を見て原因を確定する。
