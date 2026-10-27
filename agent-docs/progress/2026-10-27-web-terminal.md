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
  `terminal_session_events`（migration 0099。0097・0098 は main の `comment_author_kinds`・`decision_log_intervention_actors`）、web gateway の
  `/terminal/ws`・`/terminal/step-up`・`/terminal/status`、`/api/terminals` の中継の拒否、xterm.js の画面、CoS ops の除外表。
- `phase 1: web 端末の検査結果を記録する` — clippy の指摘（openpty の引数）の修正とこの進捗ファイル。
- `phase 1: web 端末の gateway 試験で並走する probe を記録しない` — 並走する他の試験の loopback probe で偽 daemon が落ちたので、`acceptFixtureRequest` で絞った。

- `phase 1: terminal-pty で session を抜けて tty を持つ process を回収する`（review の差し戻し、WU pty-orphan-fix）—
  helper は session の SIGKILL の後に出力の読み切り（join）を待ち、その後で `cgroup.kill` を書いていたので、`setsid` で
  session を抜けて slave を持つ process があると join が返らず、SIGTERM でも止まらなかった（reviewer の再現:
  `setsid sleep 48` で helper が 48 秒残った）。直した順: session の SIGHUP/SIGKILL → slave の pts を
  `/proc/<pid>/fd` で開いている自 uid の process に SIGKILL（上限 2 秒で消えるまで待つ）→ scope に自分以外が残って
  いれば `cgroup.kill` → 読み切りは最大 2 秒の出来事待ち。ADR D1 付記 A1、試験 2 件を追加。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6672 passed / 21 skipped、web vitest 689 passed、web server 試験成功。
  WU の run では `env -u CELERIS_WU_BASE` で基準を main にして実行）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 個別: `cargo nextest run -p celerisctl --test terminal_pty`（3 passed: 入出力・日本語・リサイズ・exit code、stdin の
  EOF と SIGTERM で SIGHUP を無視する別 process group の job まで消える）、`cargo nextest run -p task-api --test terminals`
  （4 passed: operator・MCP・別の人・token 無し構成は 403、未接続 409、work_dir 無し 409、記録）、
  `node --test server/terminal.test.mjs`（6 passed: 切断・終了・exit・idle・最大継続・logout・上限 429・停止、
  step-up 無し・別 Origin・Origin 無し・LAN の Host は拒否、tailscale の Host は許可、未接続クラスタは開かない）。
- 差し戻しの修正（WU pty-orphan-fix）: `cargo test -p celerisctl --test terminal_pty` → 5 passed（既存 3 件 + 新しい
  `stdin_eof_ends_a_job_that_left_the_session_holding_the_tty`・`sigterm_ends_a_job_that_left_the_session_holding_the_tty`）、
  4 回続けて全件成功し、毎回の後に `pgrep` で helper と `sleep 300` の残りが無いことを確かめた。修正前の helper では
  新しい 2 件が「helper did not exit within 15s」で落ちる（修正前のソースに戻して確認）。reviewer の再現
  `( sleep 1 ) | timeout 20 celerisctl terminal-pty --cwd . --cols 80 --rows 24 -- /bin/sh -c 'setsid sleep 48 & echo started; sleep 100'`
  は修正後 1 秒で終わり（exit=129、SIGHUP で終わった shell の code）、`sleep 48` は残らない。
- WU pty-orphan-fix の 2 回目: 前の run で入れた `scripts/dev/test-affected.sh` の修正は WU の範囲外（範囲の check が
  落ちた）なので `phase 1: test-affected の変更をこの WU から外す` で取り消した。提出時の HEAD で
  `cargo test -p celerisctl --test terminal_pty` → 5 passed、clippy・fmt → exit 0、範囲の check → 範囲外 0 件。

## 本番への移行
- 端末は既定で loopback の Host からだけ開ける。tailscale から使うには `~/.config/celeris/web.env` に
  `CELERIS_WEB_TERMINAL_HOSTS=<tailscale の名前>` を足してから昇格する（operator が利用者に告知してから適用する。
  手順は docs/ops/web-terminal.md「本番で有効にする手順」）。
- migration 0099 は昇格時に daemon が当てる（表の追加だけ）。

## 未解決
- 本物の browser・ssh・systemd-run の e2e（xterm の描画、CSP 下の `<style>`、scope の資源上限、ControlMaster 越しの
  ssh）は release gate と本番の確認に任せた。この run では未確認。
- daemon が止まっている間に閉じた端末の終了記録は残らない（ADR D2 に書いた）。
- scope の外（`inline`）では、tty を閉じて `setsid` で抜けた process と他の uid の process は消せない（ADR D1 付記 A1）。
  本番は `systemd-run` の scope で起こすので `cgroup.kill` で消える。scope での `cgroup.kill` の実機確認は未確認
  （この run の sandbox では systemd-run の scope を作らない）。
- この WU の前の run で残った古い helper 3 個（2 thread、子なし、`drop(command)` 修正前の build と推定）を見つけ、
  SIGKILL で消した。

- `CELERIS_WU_BASE` を基準にした test-affected は、変更 crate が bin だけの celerisctl になるので
  `cargo test --doc -p celerisctl` が「no library targets found」で落ちる（script の欠陥。この WU の範囲外）。
  修正は followups の task に回した（`cargo metadata` で lib target のある crate だけを doc test に渡す）。

## 提案
- 端末の入出力の opt-in 記録は作っていない。必要になったら秘密の扱いを別 ADR で決める。

## main 同期（integration-repair-2）
- main が 0098 を `decision_log_intervention_actors` に使ったので、`terminal_session_events` を 0099 へ移し、
  `SCHEMA_VERSION` を 99 にした（版数を固定する試験の assert も 99）。
- 未解決: `scripts/dev/jev_source_reader.py` の `KNOWN_SCHEMA_VERSIONS` は 98 のままで、版 99 の DB に
  `unknown_schema` の診断を出す。この WU の範囲外なので followup に回した。
