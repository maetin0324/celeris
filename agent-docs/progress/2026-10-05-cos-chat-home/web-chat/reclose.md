---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-07
---
# web-chat / reclose（再検査と記録の更新）

polish 段（fix-visual・shots）を統合した HEAD `b1e61693` で全体検査を再実行し、`web-chat.md`（まとめ）と ADR 付記の D6 小節を更新した。実装は変更していない。

## final review 4 指摘の解消確認

- (1) streaming 中: `chat-streaming-1440.png` が生成途中（caret 付き本文・「考え中」・停止ボタン）を写す。e2e `content.spec.ts`「streaming は text_delta を流し、確定 message に置き換えて二重表示にしない」が撮る前に caret 1 個・本文 1 項目・「停止」可視を検証。
- (2) badge 縦積み: `bc0f30e4`（fix-visual）で badge `shrink-0 whitespace-nowrap`・題名 `truncate`。e2e `threads.spec.ts`「長い題名の旧会話でも badge は一行に収まり、題名を省略する」が badge 矩形の高さと省略を検証。1440 の 5 枚すべてで一行。
- (3) tool 状態 contrast: 成功・失敗・実行中は既存 token（`text-success-foreground` 等）。白地 7.13:1・8.31:1・7.27:1、hover 6.07:1・7.07:1・6.18:1（AA 以上）。vitest `chat_messages_*` が contrast を固定。
- (4) 狭い幅: `chat-drawer-320.png`（drawer 開き）・`chat-attachment-sent-320.png`（送信後画像 preview）を追加。白い thumbnail の原因（1px 白 PNG の fixture と fake-daemon preview）を 96×64 青黄格子に修正し、e2e `mobile.spec.ts` が `naturalWidth=96` を poll。

## 証拠コマンドと結果（HEAD b1e61693）

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: exit 0（623ms、lockfile 差分なし）。
- `bash scripts/dev/test-parallel.sh`: exit 0。nextest 141 binaries・4355 passed・0 failed・14 ignored（doc-test 10 binaries 込み）。nextest 77.2 s、doctest 8.5 s。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`・`lint`: 各 exit 0（lint は既存 `!important` warning 4 件のみ）。
- `corepack pnpm@12.6.0 -C web test`: exit 0。vitest 70 files / 490 tests、node server 57 tests passed。
- `corepack pnpm@12.6.0 -C web build`: exit 0（既存 chunk size warning のみ）。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test`: exit 0、278 passed・8 skipped・0 failed。
- `CHAT_SHOT_DIR=$PWD/agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots WEB_E2E_SCOPE=functional … playwright test e2e/chat`: exit 0、31 passed。screenshot 11 枚を本 HEAD で再生成（4 枚が build/fixture 差で像素更新）。
- `corepack pnpm@12.6.0 -C web run check:boundaries`・`check:parity`・`check:secrets`: 各 exit 0。
- `corepack pnpm@12.6.0 -C web run mobile-audit`: exit 0。32 paths × 4 widths。
- `sh scripts/dev/check-adr-numbers.sh`（149 files）・`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・`sh scripts/dev/check-doc-links.sh`: 各 exit 0。

## crates/・gui/ に差分が無いことの確認

- `git diff --name-only c40b3669 HEAD` は 101 files（web/ 79・agent-docs/ 22）。`crates/`・`gui/` は 0 files。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ gui/`: exit 0。本 leaf の WU base からの差分は再生成 screenshot 4 枚と文書（web-chat.md・ADR 付記・本ファイル）のみ。

## 未解決・提案

- 実装の未解決は web-chat.md の「未解決」節の 6 件をそのまま引き継ぐ（実装は本 leaf の範囲外）。
- 本 leaf での目視: この run のモデルは画像入力を扱えないため、screenshot の最終的な見た目の確認（4 指摘の解消）は e2e の determinism な assertion（badge 矩形・contrast token・naturalWidth・caret/停止ボタンの可視性）と、polish 段（fix-visual.md・shots.md）が記録した目視結果に依拠した。人が final review で再目視することを推奨。
