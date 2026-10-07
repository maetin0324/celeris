---
title: ブラウザ実行の owner session に信頼できるデバイスを入れ、再起動・promote 後も再承認なしで使えるようにする
tasks: [01M4ADMXWYHPSVEJBJ5JPCR6PS]
status: running
updated: 2026-10-07
---
# 信頼できるデバイスと owner session の自動復帰

ADR `agent-docs/adr/2026-10-07-browser-trusted-devices.md`（状態: 提案）。葉ごとの進捗は `2026-10-07-browser-trusted-devices/<key>.md`。

## 葉 adr（2026-10-07 完了）

- 現状をコードで確認して ADR §1.1 に書いた。
  - `web/server/browser-live.js`: owner はメモリだけ・24h・challenge は 12 hex/5 分。
  - `web/server/auth.js`: login cookie は HMAC。鍵 file が無いと再起動で無効。
  - `scripts/selfdeploy/lib.sh` の `sd_web_app_probe`: 本番 web.env を読む。
  - `crates/task-api/src/browser_live.rs`: Ed25519 で検証。
- 決定（ADR D1〜D7）:
  - 方式: 乱数 cookie → https 化後に passkey（人の決定 cookie-then-passkey）。
  - cookie は `__celeris_web_device`、path `/browser/owner-session`。値は `<ULID>.<32 byte base64url>`。
  - 保存: daemon SQLite に web 側で計算した SHA-256 だけを置く。
  - 復帰: login ＋ 端末 cookie のときだけ。
  - 期限・上限: 30 日 sliding、絶対 90 日、5 台。
  - 回転: 使うたび。旧秘密の再提示で失効。
  - events 4 種。CLI 承認は残す。probe は `CELERIS_WEB_PROBE=1`。
- 証拠: `git diff --name-only "$CELERIS_WU_BASE" -- crates/ web/ gui/ scripts/` が空（コード変更なし）。

## 未解決事項

- 人の決定 `sliding-90` は「最後の使用から 30 日・使うと延長・登録から 90 日の絶対上限」と読んだ（計画の推奨どおり）。
  「90 日 sliding」の意味なら、ADR D4 の 30 日を 90 日に替えるだけでよい（store 葉の定数）。運用セッションの確認を待つ。
- LAN の http では端末 cookie が平文で流れる（ADR D1 の緩和策）。https 化と passkey は別 task。

## 提案

- web を https で配る（reverse proxy か tailnet の証明書）。passkey（D1）の前提になる。
