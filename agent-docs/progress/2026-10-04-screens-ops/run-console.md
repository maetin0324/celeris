---
tasks: [01M436BZBX67PM4ZWR53ETRX9M]
work_unit: console
status: done
completed: 2026-10-04
base: 171c8e02
---
# run ログと Console の code/log 面（run-log・console 葉）

/tasks/$id/runs/$runId の run ログ画面（run-log 葉、commit 6ddd8573・d225624c）と、`/`・`/org/$id` の Console
（console 葉、commit 136dfac2 ほか）を web/components/ui の primitive（Badge・StatusBadge・Button・DataList・CodeBlock・
LogSurface・Icon）と token だけで作り直した記録。

## 変更点

run ログ（web/features/runs/**、run-log 葉）:
- header に run の状態（StatusBadge）・harness（adapter と model）・開始・所要時間（実行中は「経過」）を DataList で出す。
- event ごとに種類の文字 label（agent / user / tool / コマンド / 思考 / エラー …）を Badge で出す。tool・コマンドの出力と
  未分類の行は CodeBlock、原文表示は LogSurface。「長い行を折り返す」（aria-pressed）で折り返しを切り替える。
- 本文は高さ上限のある面の内側で scroll し、末尾にいるときだけ追記に合わせて送る。離れていれば「最新へ」。

Console（web/features/console/**、console 葉）:
- block ごとに種類の文字 label（あなた / 返事 / 返事（生成中）/ run の作業 / 質問 / 質問（回答済み）/ 承認待ち / 途中目標 /
  報告 / 知識）を Badge で出し、送り手・宛先と時刻を同じ行に置く。以前は bg-blue-50 等の色だけで返事と発言を分けていた。
- 返事・発言の本文中の ``` の囲みは CodeBlock、progress の先頭・末尾の行（間の省略件数つき）と「すべて見る」の全行は
  LogSurface（等幅・折り返し）。返事の手順（steps）と思考は開閉（summary は 44px）。
- 追記時の追従: ページ（window）が末尾にあるときだけ追記に合わせて末尾へ送る。離れていれば送信欄の上に「最新へ」を出す。
  送信後は返事を追う。
- progress・返事・質問から run ログ画面への link「run ログを開く（R…）」を足した。
- 送信欄は素の border（@layer base の --color-input）・bg-background・md:left-nav。IME 変換中の Enter 無視・
  accessible name「Console への入力」・下書きの保持・下端固定はそのまま。
- 空・読み込み中・取得失敗に状態と次の行動の文を足した。生の色（neutral-*・red-800・blue-50・white）を消した。
- 純粋関数は console-labels.ts（+ console-labels.test.ts の 5 件）。
- e2e（console.spec.ts）に 2 件足した: 360px で 2400 文字の 1 行（発言・返事の code・progress の行・全行）が
  `scrollWidth <= 360` を保つこと、追従と「最新へ」。

## before/after

screenshot は run の成果物ディレクトリ `artifacts/shots/`（before-*.png / after-*.png、360/390/412/1280px）。
Console の撮影は artifacts/console-shots.mjs（偽 daemon に長い path を含む発言・progress・返事・質問を返す）。

- before（Console）: 全 block が同じ枠で、返事は薄い青の背景だけで区別。progress の行は地の文で長い path が十数行に
  伸びる。「すべて見る」は全幅の button。時刻・送り手が無い。360px でもページは広がらない（break-words）が読みにくい。
- after（Console）: 1 枚の面に区切り線で block を並べ、各行頭に種類の label と時刻。progress の行は等幅の LogSurface
  （高さ上限で内側 scroll）、返事の手順は折り畳み。run ログへの link。360/390/412/1280 すべて scrollWidth = 幅。
- run ログの before/after は run-log 葉の commit d225624c の本文のとおり（状態・harness・所要時間が無く太字の語だけ →
  header と種類 label と code 面）。

## 要望

範囲外（web/components/・styles.css・e2e/support・API 型）のため作らず記録する。

run-log 葉から移したもの:
- LogSurface・CodeBlock が ref を受けない（Omit<HTMLAttributes> の型）。追従 scroll のため wrapper の capture で拾っている。
  ref（または追従 scroll と「最新へ」）を primitive 側で持てると画面ごとの実装が要らない。
- 会話一覧のような「行ではなく要素を並べるログ面」の primitive が無い（いまは token で枠を組んでいる）。
- RunSummary に harness 名そのもの（claude-code の CLI 版等）と所要時間の数値は無い。adapter・model・started_at/finished_at から出している。

console 葉から:
- 追従 scroll の hook（末尾判定・「最新へ」）が run ログ（面の内側）と Console（window）で 2 つある。
  `useFollowScroll(target)` のような共通の部品が web/components にあるとよい。
- 開閉（details/summary に chevron と 44px の高さ）を run ログと Console が別々に組んでいる。Disclosure primitive が欲しい。
- ConsoleProgress に run が終わったかの欄が無い。progress block の label を「作業中」と言えず「run の作業」にしている。
- 本文の Markdown（``` 以外の見出し・list）は解釈していない。Markdown 表示の primitive を決めるなら Console と報告で共有したい。
- e2e/support の fixture-gateway に Console の block が無く、screenshots.mjs / mobile-audit の `/` は空の Console しか見ない。
  block を返す fixture があると mobile-audit が会話の tap 領域も検査できる。

## 検査結果

すべて worktree の HEAD（console 葉の最終 commit）で実行。

| コマンド | exit | 要点 |
|---|---|---|
| `corepack pnpm@12.6.0 -C web install --frozen-lockfile && … typecheck && … lint && … test` | 0 | vitest 44 files / 278 tests passed。biome は既存の warning 4 件（skills-screen・styles.css）のみ |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/console.spec.ts e2e/parity/runs-files.spec.ts` | 0 | 11 passed（console 8・runs-files 3） |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/console.spec.ts --repeat-each=4` | 0 | 32 passed（追従 test の安定を確認） |
| `corepack pnpm@12.6.0 -C web mobile-audit` | 0 | 30 path(s) x 4 widths ok |
| 差分範囲 check（task の受け入れ条件 2）と `git diff --quiet 171c8e02 -- crates/ web/components/ web/styles.css web/e2e/support/` | 0 | 範囲外の差分なし |
| 生の色・任意値の grep（web/features/console） | 1（該当なし） | `neutral-*`・`red-*`・`bg-white`・`-[..]` なし |

cargo の検査（test-parallel.sh・clippy）は crates/ に差分が無いため実行していない。
