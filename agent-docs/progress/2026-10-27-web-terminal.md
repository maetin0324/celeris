---
title: クラスタ画面から host と登録済みクラスタの web 端末を開く
updated: 2026-10-27
status: done
tasks: [01M5ZA2NP31AVVKQ9F4T3A52VB]
---
# クラスタ画面から host と登録済みクラスタの web 端末を開く

完了日: 2026-10-27。
基準: `git merge-base HEAD main`。設計は [ADR 2026-10-27-web-terminal](../adr/2026-10-27-web-terminal.md)、
使い方と戻し方は [docs/ops/web-terminal.md](../../docs/ops/web-terminal.md)。

## 変更
- `phase 1: クラスタ画面から host とクラスタの web 端末を開けるようにする` — ADR、PTY helper
  （`celerisctl terminal-pty`）、daemon の `POST /api/v1/terminals`・`/close`（主体 human:web だけ）と追記専用の表
  `terminal_session_events`（migration 0098。0097 は別 branch の `comment_author_kinds` が使うので予約）、web gateway の
  `/terminal/ws`・`/terminal/step-up`・`/terminal/status`、`/api/terminals` の中継の拒否、xterm.js の画面、CoS ops の除外表。
- `phase 1: web 端末の検査結果を記録する` — clippy の指摘（openpty の引数）の修正とこの進捗ファイル。
- `phase 1: web 端末の gateway 試験で並走する probe を記録しない` — 並走する他の試験の loopback probe で偽 daemon が落ちたので、`acceptFixtureRequest` で絞った。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6639 passed / 21 skipped、web vitest 689 passed、web server 105 passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 個別: `cargo nextest run -p celerisctl --test terminal_pty`（3 passed: 入出力・日本語・リサイズ・exit code、stdin の
  EOF と SIGTERM で SIGHUP を無視する別 process group の job まで消える）、`cargo nextest run -p task-api --test terminals`
  （4 passed: operator・MCP・別の人・token 無し構成は 403、未接続 409、work_dir 無し 409、記録）、
  `node --test server/terminal.test.mjs`（6 passed: 切断・終了・exit・idle・最大継続・logout・上限 429・停止、
  step-up 無し・別 Origin・Origin 無し・LAN の Host は拒否、tailscale の Host は許可、未接続クラスタは開かない）。

## 本番への移行
- 端末は既定で loopback の Host からだけ開ける。tailscale から使うには `~/.config/celeris/web.env` に
  `CELERIS_WEB_TERMINAL_HOSTS=<tailscale の名前>` を足してから昇格する（operator が利用者に告知してから適用する。
  手順は docs/ops/web-terminal.md「本番で有効にする手順」）。
- migration 0098 は昇格時に daemon が当てる（表の追加だけ）。

## 未解決
- 本物の browser・ssh・systemd-run の e2e（xterm の描画、CSP 下の `<style>`、scope の資源上限、ControlMaster 越しの
  ssh）は release gate と本番の確認に任せた。この run では未確認。
- daemon が止まっている間に閉じた端末の終了記録は残らない（ADR D2 に書いた）。

## 提案
- 端末の入出力の opt-in 記録は作っていない。必要になったら秘密の扱いを別 ADR で決める。
