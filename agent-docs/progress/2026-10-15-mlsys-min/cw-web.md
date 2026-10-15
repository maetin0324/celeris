---
task: 01M4Z2B2HA2FEK1ZRWKS8MXSRR
unit: cw-web
status: done
completed: 2026-10-15
---
# cw-web: WebUI（受信箱 item の登録リンク・待機状態表示）

ADR 2026-10-15-mlsys-min-decision-assisted D4 の名前（`waiting_for_auth`・`secret_stored`・集約キー `credential:<secret_id>`）をそのまま使い、新しい名前は足していない。

## 実装（web/ のみ。Rust は触らない）
- 生成型: cw-enter の schema 差分（`InboxKind` に `credential_wait`、`WorkUnitBlockedReason` に `waiting_for_auth`）を `docs/api/v1/api-v1.schema.json` へ同一内容で適用し、`pnpm install --offline` → `pnpm gen:types`。
- 受信箱: `credential_wait` を種類・表示名・専用画面扱い（answer は native）に追加。行き先は `/accounts?secret=<secret_id>`（secret id は `answer.native.path` の末尾。値ではない）。誤キー（item が「更新してください」）は「再登録待ち」バッジ。
- `/accounts?secret=<id>`: secrets section の該当 secret にフォーカス（scrollIntoView・枠・置き換え欄を開いて入力欄へ focus）。Jev 用の `jev_api_key` は cw-resume が一覧に常に出すので登録項目の追加は不要。
- secrets section 先頭の「登録待ちの work unit」（`credential-waits.tsx`、判定は `credential-wait-model.ts`）: 待っている task（リンク）・unit 数・状態（登録待ち / 検証中 / 再登録待ち）。登録直後は検証中、item が消えたら「続行しました」。登録は連携の有効化で予算の承認ではない旨を表示。LLM を使わない。item の再取得は 5 秒間隔の API 読み取り。
- task の待ち理由: `units_blocked` が `waiting_for_auth` のとき「API キーの登録待ち（登録で自動再開）」。
- `features/ops/clusters-screen.tsx`: 既存の biome format 違反を整形しただけ（lint を通すため）。

## 証拠（web/ で `corepack pnpm@12.6.0`）
- `typecheck`: exit 0。`lint`: exit 0（warning 4 件は既存の styles.css）。`test`: vitest 94 files / 660 tests pass、node test 93 pass。
- 新規試験: credential-wait-model.test.ts（secret id・focus 先・3 状態・猶予・行）、inbox-model・wait-reason の追加分。
- 範囲 check（`sh $CELERIS_WU_SCOPE_PATHS`）: web/・docs schema・この進捗のみ。

## 未解決・提案
- cw-enter のブランチは本 WU の base に入っていない（schema 差分のみ同一内容で適用。merge で同一 hunk になる）。
- 検証中か再登録待ちかを API が持たないため、「保存から 60 秒は検証中」とする近似（`VERIFY_GRACE_MS`）。wait の state（waiting/verifying/rejected）を inbox item に出す欄の追加を提案。
- Playwright e2e（fake daemon に credential_wait item を足す）は未追加。
- 実ブラウザ・実 daemon での目視は未実施。
