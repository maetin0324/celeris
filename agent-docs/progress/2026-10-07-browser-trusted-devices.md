---
title: ブラウザ実行の owner session に信頼できるデバイスを入れ、再起動・promote 後も再承認なしで使えるようにする
tasks: [01M4ADMXWYHPSVEJBJ5JPCR6PS]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 信頼できるデバイスと owner session の自動復帰

ADR `agent-docs/adr/2026-10-07-browser-trusted-devices.md`（状態: 実装済み）。葉ごとの進捗は `2026-10-07-browser-trusted-devices/<key>.md`。

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

## close-out（2026-10-07 完了）

統合後 HEAD の全体検証（葉 verify、記録: `2026-10-07-browser-trusted-devices/verify.md`）。コード・設定は未変更。全コマンド exit 0、未解決の失敗なし。

| コマンド | exit | 要点 |
|---|---|---|
| `bash scripts/dev/test-parallel.sh` | 0 | nextest 4320 passed / 0 failed（skip 13）、doctest 合格 |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告なし |
| `cargo fmt --all -- --check` | 0 | 差分なし |
| `corepack pnpm@12.6.0 -C web test` | 0 | vitest 491 passed、node:test 67 pass |
| `… -C web typecheck` | 0 | エラーなし |
| `… -C web lint` | 0 | error なし、既存 warning 4 件（styles.css の `!important`） |
| `… -C web e2e` | 0 | 275 passed / 8 skipped / 0 failed |
| `bash scripts/selfdeploy/tests/web_probe_owner_isolation.sh` | 0 | all ok |
| `verify_web_app_start.sh` / `web_follow_health_gate.sh` | 0 | ok / all ok |

成果（葉 docs）:

- 本番確認の人向け手順: `docs/ops/browser-trusted-devices.md`（実行は運用セッション）。
- ADR `agent-docs/adr/2026-10-07-browser-trusted-devices.md`: 状態「実装済み」、実装との突き合わせの付記あり。
- 葉ごとの記録: `api.md`・`store.md`・`web-server.md`・`web-ui.md`・`docs.md`・`verify.md`（同名ディレクトリ配下）。

## 未解決事項

- 人の決定 `sliding-90` は「最後の使用から 30 日・使うと延長・登録から 90 日の絶対上限」と読んだ。「90 日 sliding」の意味なら ADR D4 の 30 日を 90 日に替えるだけ（定数）。運用セッションの確認待ち。
- LAN の http では端末 cookie が平文で流れる（ADR D1 の緩和策）。https 化と passkey は別 task。
- CLI 承認の owner に `deviceId` が付かない。登録直後は「この端末」badge が出ず（次の resume から）、その端末の失効で今の owner が落ちない。
- `celerisctl browser owner-session approve` の `--socket` 既定の環境変数が `CELERIS_GUI_OWNER_SOCKET` のまま（web では `--socket` を明示）。
- daemon の `verify` の `readonly`（probe 用）は web から呼ばれていない。端末の失効を CLI から行う経路は無い（CLI 承認 → 画面で失効）。
- 不一致の秘密による拒否でも、その端末由来の owner を落とす（fail closed。owner にはなれない）。
- 自動復帰は画面の読み込みごとに 1 度だけ。失敗後は読み込み直すまで challenge 表示のまま。
- main を取り込むと migration 0055・0056 が実在になる。`RESERVED_VERSIONS` から外し、試験の版数一覧を直す。
- `gui/docs/celeris-api-v1.md`（`scripts/sync-gui-docs.sh` の写し）はこの task の前からずれている（未着手）。

## 提案

- `registerDevice` で今の owner に登録した `deviceId` を結び付ける（web/server の小変更）。登録直後から「この端末」表示と失効の即時反映が揃う。
- `celerisctl browser owner-session approve` が `CELERIS_WEB_OWNER_SOCKET` も既定として読む。
- web を https で配る（reverse proxy か tailnet の証明書）。passkey（ADR D1）の前提になる。
