---
title: ホームの CoS チャットで見出し・帯を消し、会話の縦の長さを最大にする
tasks: [01M61CVREBXVQZDDP8YGM04SQ7]
status: done
updated: 2026-10-28
---
# ホームの CoS チャットで見出し・帯を消し、会話の縦の長さを最大にする

完了日: 2026-10-28

## 変更
- `web: ホームの会話から見出しと帯を消し、縦の長さを会話に使う`
  - `ScreenFrame` に `titleHidden` を足し、ホームの「ホーム」の h1 を sr-only にした（遷移後の focus 先・読み上げは残る）。
  - 統括 CoS の題名の帯を sr-only の h2 にし、読み込み中の「統括 CoS を開いています」も sr-only にした。
  - 会話の枠を負の margin で main の余白（p-4 / md:p-6）まで広げ、枠線と角丸を外した。高さの計算は張り出した分と
    main の後ろの Console の置き場の枠線（1px）を引き、ページを伸ばさない。
  - 案件スレッドの帯は 1 行に詰めた（題名は省略表示、電話幅は「読む用」だけ見せ全文は読み上げに残す、「統括 CoS に戻る」は 44px の button のまま）。
  - 判断待ちへの入口は shell のナビ・画面下のタブの「受信箱」（件数 badge）と会話中の札で、変えていない。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 会話の表示部分（`[data-slot="chat-messages"]`）の高さ（fixture gateway、playwright で測定）:

  | viewport | thread | 変更前 | 変更後 |
  |---|---|---|---|
  | 1280x800 | 統括（chat-main） | 569px | 708px |
  | 1280x800 | 案件（chat-history） | 549px | 659px |
  | 390x844 | 統括（chat-main） | 523px | 643px |
  | 390x844 | 案件（chat-history） | 442px | 594px |

  入力欄は 4 条件とも viewport 内（下端 1280: 799px、390: 779px ＝ 下部タブの上）、document の高さは viewport と同じ。
- 追加した e2e `e2e/shell/chat-home.spec.ts`「1280x800 / 390x844: 見出しと帯を出さず…」→ pass（会話の高さ 680px / 620px 以上・入力欄が全部見える・見出しと帯が sr-only）。
- `WEB_E2E_SCOPE=functional npx playwright test e2e/shell e2e/chat e2e/parity/shell.spec.ts e2e/states` → 90 passed / 8 failed。
  失敗 8 件のうち 7 件（chat/queue 5 件・shell/chat-home「人の会話が無ければ…」・shell/home-layout「Console 1440…」）は変更前の main でも同じく失敗。
  shell/mount「遷移後に見出しへ focus…」は `/tasks`・`/projects` だけを開く試験で、変更前の main でも 6 回中 5 回失敗する不安定な試験。
- `bash scripts/dev/test-affected.sh` → exit 0（web tsc -b 通過、vitest 690 passed、server 105 passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- main で失敗している e2e 7 件と shell/mount の不安定な試験は、この task の範囲外で直していない。
- 狭い幅の上部 header（Celeris・接続状態・メニュー、約 52px）は shell 全体の構成なので変えていない。

## 提案
- 電話幅の上部 header は画面下のタブと役割が重なる。ホームでは header を畳む案を別 task で検討する。
