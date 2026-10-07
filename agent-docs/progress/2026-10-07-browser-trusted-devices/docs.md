---
title: 葉 docs — 本番確認の手順・ADR 実装済み化と付記・architecture-map の入口
tasks: [01M4AK858F3BEPV7HHN9EW9EET]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 docs: 信頼端末の文書の close-out

## やったこと（文書だけ。crates/・web/・gui/ は差分なし）

- `docs/ops/browser-trusted-devices.md` を新設した。配送後に運用セッションが本番で行う 5 手順を、コマンドと期待する出力つきで書いた:
  1. migration 0057 の読み取り確認（`sqlite3 -readonly` で `schema_migrations`・`.schema browser_trusted_devices`）。
  2. 初回の host CLI 承認（`celerisctl browser owner-session approve <challenge> --socket <web.env の CELERIS_WEB_OWNER_SOCKET>`）→ 「この端末を信頼する」。
  3. web 再起動・promote（web-follow）の後に challenge なしで owner に戻ること（`trusted_device_used` の event、probe が端末を書かないこと）。
  4. `/browser/devices` の一覧と失効（失効後は即 challenge）。
  5. 端末を失ったときの復旧（別端末、または CLI 承認 → 画面で失効 → 再登録）。
  コマンド・cookie 名・期限・端点は ADR と実装（`web/server/browser-live.js`、`crates/celerisctl/src/commands/browser.rs`、
  migration 0057、`web/features/browser/`）で確かめた。実行はしていない（本番 host・`~/.config/celeris`・本番 DB に触れていない）。
- ADR `agent-docs/adr/2026-10-07-browser-trusted-devices.md` の状態を「実装済み（2026-10-07）」にし、末尾に「付記（close-out の実装突き合わせ）」を足した。
  D1〜D7 ごとに実装の場所（file・関数・試験名）と食い違いを書いた。
- `docs/architecture-map.md` に 3 行足した: task-core の信頼端末 store、task-api の信頼端末 API、web gateway の owner session・Live View・信頼端末。
- `scripts/dev/docs-layout.tsv`: 足さない。この表は ADR-0128 の再配置の manifest で、再配置後に足された `docs/ops/*.md`
  （`browser-web-live-check.md`・`cron-jobs.md` など 19 件）も載っていない。新しい人向け運用文書は `docs/ops/` にそのまま置けばよい。

## 証拠

- `sh scripts/dev/check-doc-links.sh`: `check-doc-links: ok`。
- `sh scripts/dev/check-adr-numbers.sh`: `ok (153 files)`。
- `sh scripts/dev/progress-index.sh --check`: ok。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: `check-doc-layout: ok`。
- `python3 scripts/dev/check-architecture-map.py`: `OK: 285 件のパスを確認した`（初回は subsystem 欄の URL を backtick で書いたためパスと誤認され NG、外して解消）。
- `test -s docs/ops/browser-trusted-devices.md && grep -q 'owner-session approve' … && grep -q '実装済み' <ADR>`: exit 0。
- 全体の `test-parallel.sh`・clippy は並行の verify 葉が統合後の HEAD で流す（この葉はコードを変えていない）。

## 未解決事項

- CLI 承認の owner に `deviceId` が付かない（登録直後に「この端末」が出ず、その端末の失効で今の owner が落ちない）。
- `celerisctl browser owner-session approve` の `--socket` 既定の環境変数が `CELERIS_GUI_OWNER_SOCKET` のまま（web では `--socket` を明示）。
- daemon の `verify` の `readonly`（probe 用）は web から呼ばれていない。
- 端末の失効を CLI から行う経路は無い（CLI 承認 → 画面で失効）。

## 提案

- `registerDevice` で今の owner に登録した `deviceId` を結び付ける（web/server の小変更）。
- `celerisctl browser owner-session approve` が `CELERIS_WEB_OWNER_SOCKET` も既定として読む。
