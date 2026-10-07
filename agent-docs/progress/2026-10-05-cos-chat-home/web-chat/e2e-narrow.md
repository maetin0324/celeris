---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-07
---
# web-chat / e2e-narrow（狭い幅・keyboard・画面記録）

ADR 2026-10-05-cos-chat-home D6 の狭い幅の行を `web/e2e/chat/mobile.spec.ts` の 8 試験で検証した。旧 e2e-mobile-wip の mobile.spec.ts を参考にし、描画完了の出来事待ち、320/390 px 両方の drawer と keyboard、実際の送信・停止、本文と composer の重なり検査を加えた。tracked.patch は使っていない。

## 検証内容

- 320/390 px・高さ 700 px で、カードの回答ボタンが読み込まれた後の横溢れは 0。DOM に表示される操作要素（本文・カード・composer・タブバー）は幅・高さとも 44 px 以上。
- 両幅で drawer を開き、閉じるボタン・Escape の後に trigger へ focus が戻る。会話の選択で閉じ、URL に選択を保存。drawer 内にも横溢れ・小さな操作領域がない。
- 320 px でタブバーと composer の矩形が重ならず、Enter 送信がキューに入る。
- 両幅で visualViewport を 700→480 px に縮める。入力・送信・停止の矩形が視界内にあり、中央の hit target が該当要素内であることを expect.poll で確認。送信ボタンでキューへ追加し、停止ボタンでキューを停止。送信待ち追加後も操作が隠れず、700 px に戻した後もタブバーと重ならない。
- 320 px の添付・カードと desktop のカードの操作領域を測定。画像 preview の decode・upload 成功を待って画面を記録した。

## 見つかった不具合と修正

320 px の keyboard 相当の視界で送信待ちが増えると、composer が flex で縮み、停止ボタンが枠の overflow に切られた。また、ホームの高さ計算と composer の sticky bottom の双方で下部タブ・keyboard の退避量を適用し、本文に重なって下に余白を残していた。

`features/chat/home/chat-home.tsx` は keyboard が覆わずに残るタブバーの余白だけを引く。`composer/chat-composer.tsx` は shrink-0 とし、独自の sticky/visualViewport 補正を削除してホームの枠の末尾に置く。高さの管理をホームに集約した。mobile.spec の矩形・hit target・本文と composer の隣接検査が回帰を検出する。既存 composer 単体試験から旧 sticky の CSS 文字列検査を除き、safe area と操作の検査は維持した。

## 証拠コマンドと結果

pnpm はすべて `corepack pnpm@12.6.0 -C web`。初回 install --offline は vitest の tarball キャッシュ不足で失敗。install --frozen-lockfile で補い、最終の計画 check の install --offline --frozen-lockfile は成功した。

- `build`: exit 0。既存の bundle size 警告あり。
- `WEB_E2E_SCOPE=functional WEB_E2E_WORKERS=2 … exec playwright test e2e/chat/mobile.spec.ts`: exit 0、8 passed、fixme なし。
- `install --offline --frozen-lockfile && build && WEB_E2E_SCOPE=functional … exec playwright test e2e/chat`: exit 0、30 passed（mobile 8 + 既存 22）。
- `install --offline --frozen-lockfile && build && … mobile-audit`: exit 0、32 paths × 4 widths（360/390/412/1440 px）。`/` と `/console` を含む。320 px は mobile.spec で別途検証。
- `install --offline --frozen-lockfile && typecheck && lint && test`: exit 0。lint は既存 styles.css の !important の警告 4 件。Vitest は 70 files / 486 passed、server は 57 passed。
- `… exec biome format --write e2e features`: 成功。
- 計画の scope check（CELERIS_WU_BASE 基準）と `git diff --check`: exit 0。gui/・crates/ の変更なし。
- 計画の PNG 件数・容量 check: exit 0、計 8 枚、最大 121,637 bytes。

実行ログはこの WU の artifacts の `check-0.log`（chat 全体）、`check-1.log`（audit）、`check-2.log`（型・lint・単体）、`mobile-final.log` に置いた。

## スクリーンショット

既存 6 枚に次の 2 枚を追加した。目視で composer と本文・タブバーの重なりがないことも確認した。

- [320 px: 添付と CoS 代答カード](screenshots/chat-mobile-320.png): 41,796 bytes。
- [1440 px: 会話一覧と task・決定・質問カード](screenshots/chat-cards-1440.png): 121,637 bytes。

再生成は build 後、`CHAT_SHOT_DIR="$PWD/agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots" WEB_E2E_SCOPE=functional … exec playwright test e2e/chat/mobile.spec.ts` を repo root から実行する。

## 未解決・提案

この葉の受け入れ条件に未解決なし。keyboard は visualViewport の fake と resize イベントによる決定的な検査で、実端末 OS の keyboard・IME の実機確認は含まない。最終の人による UX 確認で添付・カードの 2 枚を既存 6 枚と合わせて確認する。
