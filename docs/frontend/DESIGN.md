# Celeris ops workbench のデザインシステム
---
tasks: [01M3XTCNKMQBCHKSZ7Y1GF6ZM4]
---

対象は `web/`。案件・タスク・実行を追い、人の判断から結果確認までを支える運用画面の規則を定める。本書は実装先の仕様であり、導入済み・表示検証済みを意味しない。dark mode は末尾の提案に留める。

根拠は [UX_AUDIT.md](UX_AUDIT.md) の「cosmetic と IA/component 層の切り分け」「4 群の割り当て」「quality gate critique」、[現行 styles.css](../../web/styles.css)、[共通 components](../../web/components/)、[ADR-0081](../../agent-docs/adr/0081-web-spa-frontend.md)。frontend-design・web-design・shadcn・ui-ux-quality-gate の指針を Celeris の判断・復旧・日本語表示に適用する。

## 原則（避けるもの・戻らないもの）

**対象名 → 現在状態と鮮度 → 判断の根拠 → 次の操作 → 結果**を同じ文脈で読めるようにする。成功はボタンを押せたことではなく、対象の変化と次の行動を確認できたこと。タスクの実行終了・成果の成立・人の受け入れ・リリースの稼働は別々に表示する。

- generic AI dashboard、巨大 hero、反復する統計カードを避ける。運用者が最初に知る必要があるのは、総数の大きさより「何が判断を待っているか」「更新が止まっていないか」である。
- card wall を作らない。受信箱の空区画は短い行、案件の概要は見出しと一覧、ボードのカードはタスクの比較に必要な情報だけにする。編集は開いた対象に限定する。
- 意味のない gradient/glass、飾りのグラフ、過剰余白を使わない。密度は枠の反復と常設フォームを減らして得る。文字と target を縮めて得ない。
- unstyled admin にも戻さない。主・副・危険操作の識別、状態の語彙、整列、読みやすい境界が無いと、認可や昇格の誤操作を招く。shadcn の既定の外観をそのまま完成形にせず、共通部品へ Celeris token を適用する。
- 主表示は「判断待ち」「実行中」「変更を確認」「再取得」などの日本語にする。ID・enum・JSON は根拠や診断の詳細へ置く。原文を読む run log と code は原文を保つ。設計意図を説明する文章は通常の画面へ持ち込まない。

基調色は作業面 `#FFFFFF`、周辺面 `#F4F7F9`、本文 `#20303C`、補助文字 `#536572`、区切り `#D3DCE2`、操作 `#245675`。特徴は装飾ではなく、実行中と判断待ちを区別し、対象に操作結果を結び付ける行の構造に置く。

配置は左揃え、比較する数値のみ右揃え。PC は比較のための幅を使い、スマホは選択した対象の判断を先に表示する。

```text
desktop: [日々の仕事 / 管理] [対象名・状態・最終更新       主操作]
                            [一覧または根拠] [選択対象の詳細・結果]
mobile:  [Celeris・接続状態] [受信箱・承認・メニュー]
         [対象名・状態・根拠]
         [主操作・結果]
         [詳細 / 絞り込みを開く]
```

cosmetic（文字・余白・色・境界）は本書の token で揃える。IA/component（情報順・一覧と詳細・危険操作の分離）は共通部品と画面構成から直す。`/`・`/login`・`/org/cos` 以外を色替えだけで完了扱いしない。文書の保守は JSON 中心の主操作を監査結果・整理案・確認へ作り直す。

| 監査の4群 | 共通規則の適用先と重点 |
|---|---|
| foundation（5 route / 4 fixture） | shell・login・home・help・人との Console。ナビを日々の仕事と管理に分け、page header・取得状態・確認・入力・一覧を共通化する |
| task・run 系（8 route） | タスク・計画作成・詳細・files・changes・run・graph。現在状態と要判断を先頭にし、破棄を取り込みから分離、ログ追記と復旧を見せる |
| inbox・project 系（12 route） | 受信箱・認可・報告・成果物・案件・文書・ボード・知識。閲覧と判断を先に、設定と編集は展開。選択後の本文への移動を保つ |
| 管理系（6 route） | 組織・daemon・providers・accounts・clusters・releases。稼働可否と鮮度を先に、設定と影響範囲を後に示す |

画面台帳と割り当ての正本は UX_AUDIT。実装の受け入れでは同じ fixture を 360/390/412/1440px で比較し、正常データ・長文・状態別・操作後も確認する。監査の before は取得失敗が多く、画像枚数だけで正常系や操作性の合格としない。本書の色の計算もブラウザでの検証の代わりにはしない。

## Typography

和文 UI はローカルの `"Yu Gothic UI", "Hiragino Kaku Gothic ProN", "Meiryo", sans-serif` を使う。依頼・承認理由・ログを長時間読むため、見出しも同じ sans とし、字間の演出や全大文字ラベルを加えない。外部フォント配信は不要。code と path のみ `ui-monospace, "SFMono-Regular", Consolas, "Liberation Mono", monospace` を使う。

| 役割 | token | サイズ / 行高 / weight |
|---|---|---|
| page title（h1） | `text-title`（desktop は `text-title-wide`） | 1.375rem / 1.35 / 600、desktop は 1.5rem |
| section title（h2 以下） | `text-section` | 1.125rem / 1.4 / 600 |
| 本文・主操作・入力 | `text-body` | 1rem / 1.5 / 400、button は 500 |
| 一覧の補助・field の説明・badge | `text-label` | 0.875rem / 1.5 / 400、badge は 500 |
| code・ログの構造化出力 | `text-code` | 0.875rem / 1.6 / 400（会話本文は `text-body`） |

本文は最大 65ch、和文の長い説明は最大 40em を目安にする。表や差分の全体幅には本文の上限を適用しない。見出し・対象名・失敗理由は折り返し、重要な文を省略しない。補助的な path の省略時は全文表示・コピーを用意する。数値と時刻の列は `tabular-nums`、桁数と単位を揃える。200% の文字拡大でも入力・操作・見出しを切らない。

## Spacing

4px を基本とする。関連情報は 4/8px、行・field の内側は 12/16px、section 間は 24px。ページ余白はスマホ 16px、desktop 24px。外側の余白は内側以上にし、入れ子ごとに 24px を足さない。

一覧の行は上下 8px・左右 12px を標準とし、操作がある行では 44px の操作領域を確保する。field の label と control は 4px、説明は 4px、field 間は 16px。ボタン群は 8px。`gap-*` を使い、同種の余白を各画面で微調整しない。入力とボタンは同じ最小高さに揃える。

## 状態色と contrast 比

通常文字は **4.5:1 以上**、大きい文字（24 CSS px 以上、または約 18.7 CSS px 以上の太字）は **3.0:1 以上**。英大文字化を大きい文字の例外にしない。意味を持つ icon・control 境界も隣接色と **3.0:1 以上**にする。本書の文字色は小さいラベルでも通常文字の基準を満たす。[WCAG contrast minimum](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html)、[non-text contrast](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html) に従う。

状態 token は `--color-<意味>` が淡い背景、`--color-<意味>-foreground` が文字・icon。文字と背景を必ず組で使い、opacity で薄めない。

| 意味 | 使用例・表示ラベル | 前景 | 背景 | contrast |
|---|---|---|---|---|
| success | 確認済みの成功・利用可。「保存済み」「検証合格」。実行終了だけから成果の成功を推測しない | `#166534` | `#F0FDF4` | 6.8:1 |
| warning | 人の判断待ち・注意・鮮度未確認。「判断待ち」「更新を確認できません」 | `#854D0E` | `#FEFCE8` | 6.6:1 |
| danger | 失敗・拒否・破壊的操作。「実行失敗」「権限がありません」 | `#991B1B` | `#FEF2F2` | 7.6:1 |
| info | 補足・案内。「再取得できます」。稼働中という意味には使わない | `#1E40AF` | `#EFF6FF` | 8.0:1 |
| neutral | 下書き・待機・中止・未確認。「下書き」「順番待ち」「未確認」 | `#475569` | `#F1F5F9` | 6.9:1 |
| running | 実行・検証・昇格の進行中。「実行中」「昇格中」。成功とは区別する | `#155E75` | `#ECFEFF` | 7.0:1 |

| 用途 | 前景（境界色） | 背景（隣接色） | contrast |
|---|---|---|---|
| 本文 | `#20303C` | `#FFFFFF` | 13.6:1 |
| 補助文字 | `#536572` | `#F4F7F9` | 5.6:1 |
| 主ボタン | `#FFFFFF` | `#245675` | 7.9:1 |
| 主ボタン hover | `#FFFFFF` | `#1D4660` | 10.0:1 |
| 主ボタン active | `#FFFFFF` | `#17384D` | 12.3:1 |
| 危険の確定ボタン | `#FFFFFF` | `#991B1B` | 8.3:1 |
| 危険ボタン hover / active | `#FFFFFF` | `#7F1D1D` | 10.0:1 |
| 入力境界 / 作業面 | `#758593` | `#FFFFFF` | 3.8:1 |
| 入力境界 / 周辺面 | `#758593` | `#F4F7F9` | 3.5:1 |
| focus / 作業面 | `#245675` | `#FFFFFF` | 7.9:1 |
| focus / 選択面 | `#245675` | `#E5EEF4` | 6.7:1 |

値は不透明な sRGB の組の計算結果で、小数第1位に丸めた。各 channel を 0〜1 にし、`c <= 0.04045` は `c/12.92`、他は `((c+0.055)/1.055)^2.4` で線形化。`L = 0.2126R + 0.7152G + 0.0722B`、比は `(L明+0.05)/(L暗+0.05)`。合否は丸める前の値で判定する。背景を混ぜる、新しい hover 色を作る等の場合は再計算する。

状態には日本語ラベルを必須とし、色だけを伝達手段にしない。API enum → ラベル → 意味色の対応は共通化し、未知値は neutral「未確認」とする。stale や disconnected はタスクの業務状態を上書きせず、取得・接続の状態として併記する。

## Surface と elevation

main の作業面は `surface`、shell や補助領域は `background`。一覧は作業面の上に並べ、項目ごとの白い箱を増やさない。`muted` は補足・readonly、`accent` は選択・hover の背景とし、選択は文字の太さと `aria-current` / `aria-selected` でも示す。

通常の section・行は影なし。`shadow-popover` は一時的なメニュー、`shadow-dialog` は確認・drawer に限定する。overlay は不透明の面と単色の幕で背景操作を止め、glass や blur を足さない。elevation は操作の階層を表すもので、重要度の演出に使わない。重なり順・背景の非活性化は共通の overlay 部品が管理し、画面が任意の z-index を足さない。

## Border と radius

区切りは 1px の `border`。これは装飾的な区切り専用で、入力可能性や状態を示す唯一の手掛かりにはしない。入力・outline button の境界には 3:1 を満たす `input`、入力エラーには `danger-foreground` と説明文を使う。

角丸は badge・code に 4px（`radius-sm`）、入力・button に 6px（`radius-md`）、独立した編集面に 8px（`radius-lg`）、浮いた dialog に 12px（`radius-xl`）。表の各行は丸めない。入れ子全体を同じ丸角の枠にせず、内側は余白・見出し・Separator で分ける。状態 badge を pill button に見せない。

## Icon

shadcn 導入時の `components.json` の `iconLibrary` に合わせ、1 系統の線画 icon に統一する（例えば lucide を選ぶなら `lucide-react`）。現行 `web/package.json` に icon 依存は無い。ライブラリ名の例示を追加認可とせず、導入工程の承認範囲で決定・固定する。別の icon set や外部配信を混ぜない。

標準 20px、行内・badge は 16px、stroke は 2、`currentColor`。実寸は共通部品側で指定し、Button 内の icon へ各画面から sizing class を付けない。`data-icon="inline-start"` / `inline-end` で位置を示し、icon component 自体を渡す。

主要操作と状態は可視ラベル必須。文字に添える icon は `aria-hidden="true"`。閉じる・コピーなどの icon-only 操作も `aria-label` または sr-only の名前、必要に応じた tooltip、44px target を持つ。tooltip だけを名前にしない。削除・昇格・常設許可を icon-only にしない。

## Button

| 役割 | 見た目と利用 |
|---|---|
| primary | `primary` + `primary-foreground`。現在の作業領域に原則1つ。「回答を送る」「変更を保存」 |
| secondary / outline | `surface` + `foreground` + `input` 境界。「戻る」「再取得」「編集を開く」 |
| ghost | 周囲と同じ背景、hover / active は `accent`。「詳細を開く」などの補助操作。可視ラベルを持つ |
| destructive | 一覧では danger の文字・icon・境界と淡い背景。確認画面の確定だけ `destructive` の塗り。「変更を破棄」 |

hover / active は専用 token、focus は共通輪郭を使う。secondary の hover / active は `accent`、danger の確定は `destructive-hover`。hover で要素を移動・拡大させない。最小高さ・幅は 44px、左右 padding 16px、上下 8px、長いラベルは折り返して高さを増やす。

pending はラベルを「保存中」等に変え、`aria-busy` と重複送信防止を付ける。Spinner を使ってもラベルを残す。disabled は `neutral` の組と「稼働中のため昇格できません」等の近接した理由を示し、opacity で説明まで薄めない。受理と完了を区別し、昇格 API の受理直後に「昇格しました」と表示しない。

button は操作、link は遷移。承認・却下・常設許可を同じ強さで並べず、影響差を先に説明する。呼出側の `className` は配置に限定し、色・文字・サイズは共通 variant で扱う。

## Badge

badge は状態・件数の読み取り専用部品。14px、weight 500、左右 8px・上下 4px、角丸 4px、影なし。前景/背景は状態色の組で固定し、ラベルを切らない。押して絞り込む要素は ToggleGroup 等の control として分ける。

タスク状態、成果、人の判断、昇格状態を1つの「完了」badge に潰さない。件数未取得は「未取得」（狭い nav では `?` + accessible name）、確認済み 0 件は `0`。件数を推測して成功色にしない。running は静止 icon と文字で認識できるようにし、常時点滅させない。

## Table と list

行の共通構造は「対象名 / 状態 / 補助情報 / 操作 / 操作結果」。対象名を最も読みやすくし、ID は補助、時刻は相対時刻と確認可能な絶対時刻、数量は単位付き右揃え。読み取りと選択を一覧で行い、複雑な編集は詳細・drawer で行う。

表は caption・列見出し・`scope` を持ち、並び替えは名前のある button と `aria-sort` で伝える。移動先のあるタイトルは link。行内操作を行全体の button/link に入れ子にしない。グラフのノードもキーボードで対象詳細を開けるようにし、関係を読める代替の一覧を持つ。

スマホの通常一覧は同じ情報を「対象名 → 状態 → 要判断 → 操作 → 補助」の行に組み替える。データと操作を欠落させず、見た目と DOM の順序を合わせる。比較のための表、差分、DAG、ボードの横スクロールは名前のある枠の中だけに限定し、ページ全体を溢れさせない。ボードには現在の列名・件数と列への移動手段を置く。

フィルタは検索と適用中の条件・件数を先に、追加条件は展開する。条件を URL と同期する既存の意味を保つ。受信箱の処理結果は対象行に残し、一括操作は対象数と項目別結果を示す。データが取得できない時に空行や 0 件へ置き換えない。

## Drawer

drawer は一覧の文脈を保ちながら、1対象の短い詳細・編集・絞り込みを開くために使う。Radix 系 shadcn の Sheet を基礎とする共通部品で実現し、名前が Drawer だからという理由だけで別依存を増やさない。長い文書や run log は専用ページを優先する。

desktop は右側、幅上限 32rem。スマホは viewport 内の全幅で、タイトル・閉じる・本文・操作が縦に並ぶ。`100dvh` と safe area、ソフトキーボードを考慮し、本文をスクロールして全操作へ届くようにする。固定フッターを使う場合は本文側に同じ高さの余白を確保する。

Title と必要な説明、focus trap、背景の非活性化、閉じた後の focus 復帰を共通部品で保証する。Esc と明示的な閉じる操作を用意し、swipe 専用にしない。未保存入力は無言で捨てず、破棄の確認または同じ認証 session 内での保持を行う。drawer の多段入れ子は作らない。

## Code と log の surface

code は `code` 背景と `code-foreground`、角丸 4px、padding 12px。ログの会話本文は白い作業面に置き、話者・時刻・実行状態を先に読む。tool の引数・stdout・生 JSON は必要な箇所で展開する。すべての event を同じカードで囲わない。

コードと差分は `pre` の改行を保持し、名前のある局所スクロール枠を使う。任意に折り返しへ切り替えられても、コピーは原文を保つ。本文や path は折り返してページの横溢れを防ぐ。追加/削除は success/danger の組に `+`/`−` や「追加」「削除」を添え、色だけで区別しない。

追記時は末尾を読んでいる人のみ追従し、過去を読む位置を奪わない。「最新のログへ」と未読追記の手掛かり、省略した行数を示す。省略を通信の stale と混同しない。secret を初期表示やコピーへ不用意に露出させない。

## 状態の表示

状態は取得領域・対象行・操作の近くに置く。shell は取得失敗で消さない。FetchFrame と ActionResult の既存の責務を保ち、共通の状態部品で次を満たす。

| 状態 | 表示と次の操作 |
|---|---|
| loading | 初回は内容に沿った静止 Skeleton と `aria-busy`。1秒で「読み込み中」、5秒で遅延と接続状況・再取得の手段を表示する。ADR-0081 の時間を維持し、進捗率を捏造しない。再取得中は既存データを残す |
| empty | 取得成功の 0 件だけ。「判断待ちはありません」。検索結果なしは「条件に合うタスクがありません」+「条件を解除」。初期作成が必要なら作成への link。区画の空は短い行にする |
| error | 何を取得/保存できなかったか、影響、次の操作を表示する。例「タスクを取得できません。再取得してください」。404 は「見つかりません」+ 一覧へ、5xx は再取得、422 は該当 field の文に結び付ける。理由を特定できない時は推測しない |
| stale | データを残し「更新を確認できません。最終取得 …」+「再取得」。取得失敗・切断・明示された更新遅延を根拠に表示する。Query の staleTime 経過だけで障害扱いしない。鮮度の時計は最終成功取得を使い、取得不能なら「未確認」 |
| disconnected | shell の全幅で「接続が切れています。再接続中」等、実際の transport 状態と最終受信を表示する。再接続成功だけで各データを最新扱いせず、取得成功まで stale を残す。ナビ・閲覧は利用可能にする |
| permission-denied | 403 を一般の error と分け「この操作を行う権限がありません」+ 対象と必要な権限の案内・戻り先。変更を送らず、理由のない再試行ループを作らない。401 は認証境界で保護データを消して login へ移る |

時刻は意味を明記する。「最終受信」（SSE）、「最終取得」（対象）、「最終 tick」（daemon）は別の時計で、相互に代用しない。取得成功時刻を対象そのものの更新時刻として表示しない。判断に必要なデータが更新できない場合、確認の確定を止めて再取得へ導く。

操作結果は成功・失敗・結果不明を区別する。timeout/切断は「結果を確認できません。状態を再取得してください」とし、自動再送しない。409 は「状態が変わりました」と最新状態を見せ、再確認する。重要な結果は行内に残し、toast だけにしない。読み込み・成功は `role="status"`、新しい失敗は `role="alert"`。tick ごとの同文の再読み上げやログ全体の live region 化を避ける。

## 破壊的操作の確認

削除・却下・中止・変更破棄・常設許可・本番への昇格・整理案の適用は、共通の確認 dialog を通す。対象名、影響範囲、元に戻せるか、実行後の追跡方法を先に示す。「よろしいですか」だけにしない。一回の許可と常設許可は明確に分け、一括操作は件数と対象を確かめられるようにする。

Title と説明、初期 focus を置く「戻る」、動詞と対象を含む確定ラベルを持つ。不可逆の操作では必要に応じて影響の確認 checkbox を加えるが、全操作に機械的な文字入力や長押しを要求しない。確認中に対象状態が変わったら更新して再確認し、以前の確認を使い回さない。

送信中は同じ操作の再送を止め、対象行に処理中を表示する。失敗は理由と再確認、結果不明は再取得へ進める。閉じても送信済み処理が取り消されると誤解させない。「変更を破棄」を取り込み方法の select と共通の「取り込む」ボタンに混在させない。

## Focus

すべての操作に `:focus-visible` の 2px の `ring` 色の outline と 2px offset を付ける。塗りボタンでは白い間隙で輪郭を分離する。輪郭を切る overflow を避け、selected と focus を別の形で表す。forced-colors ではシステム色の outline を残す。

先頭の skip link から main へ進め、遷移後の h1 focus、戻る時の位置、開閉時の trigger 復帰を保つ。overlay 内だけ focus を循環させ、背景を Tab で操作させない。削除後に trigger が消えたら次の行または一覧見出しに戻す。入力検証は最初の不正 field と説明へ誘導し、`aria-invalid` と `aria-describedby` で関連付ける。自動更新で focus を奪わない。

## Reduced motion

`prefers-reduced-motion: reduce` では drawer の移動、Skeleton の pulse、Spinner の回転、smooth scroll を止め、静止表示と状態文を残す。通常時も操作に応える短い遷移だけに限定し、hover は最大 120ms、overlay は最大 180ms、入力とキーボード操作は即時に反映する。

自動でカードを順次浮かせる演出、常時点滅する running、数字のカウントアップを使わない。縮小設定でも確認や処理状態を省略せず、見える情報と操作の順序を同じにする。追加の motion ライブラリは不要。

## Target サイズ

**操作可能な target は最低 44×44 CSS px**。button、nav、一覧の link、tab、閉じる、コピー、checkbox/radio の label を含める。`min-h-11 min-w-11` 相当を共通部品で確保し、文字拡大時はさらに広がる。icon や checkbox の絵自体は 16/20px のままでよい。

独立した操作間は 8px を標準とし、不可視の hit area を隣に重ねない。本文リンクは下線を保つ。長文内で 44px の領域を確保できない場合は、本文を非操作の参照名にし、直後の独立したリンク行へ操作を移す。検査回避のためにリンクを検査対象から除外しない。読み取り専用 badge に target 高さは不要。

## 幅ごとの規則

360/390/412 は撮影・検査幅であり、3つの別レイアウトを作る breakpoint ではない。本文幅によって自然に折り返し、shell は既存 md（48rem）を基準、一覧と詳細の併置は lg（64rem）以上で双方を読める場合に限る。

| 幅 | 配置と主操作 |
|---|---|
| 360px | 左右 16px、1列。対象名・状態・主操作を先に出す。接続状態・受信箱と承認の件数への入口をメニューの外に置く。複数操作は折り返し、追加フィルタ・編集を折りたたむ |
| 390px | 360 と同じ情報順。短い補助情報は同じ行へ収めてよいが、文字や target を縮めず長文で再び折り返す |
| 412px | 余った幅は対象名・根拠の可読性へ使う。スマホの一覧と詳細を無理に2列化しない。判断結果と復旧操作を対象の近くに保つ |
| desktop（検査は1440px） | 左 nav 224px、main 余白 24px。データ一覧は利用可能幅を使い、詳細併置時は一覧 20rem を基準に残りを詳細へ。フォームは最大40rem、長文は本文の行長上限で抑え、入力欄を画面いっぱいに伸ばさない |

md 未満では画面下に固定タブ（5 列・各 44×44 以上・ラベル 1 行・safe area 分を空ける）を出し、並びは **ホーム・受信箱・ボード・案件・その他** とする。タスクなど残りの項目は「その他」のシートに nav の順で出し、シート内の項目が現在地なら「その他」を現在地の色にする（[ADR 2026-10-06-web-bottom-tabbar-first-screen](../../agent-docs/adr/2026-10-06-web-bottom-tabbar-first-screen.md) D2・付記 2026-10-08）。md 以上の側面 nav は変えない。

48〜64rem では nav の幅を引いた実効幅を見て1列を維持する。狭い幅では一覧から詳細へ選択した時に詳細見出しへ移動し、明示的な「一覧へ戻る」で選択・検索・スクロールを復元する。詳細の前に全一覧や作成フォームを積まない。ボードは列移動を提供し、編集をカード内に常設しない。

flex/grid 子に `min-width: 0`、長い対象名と path は折り返しを許す。ページ全体に横スクロールを作らない。Console の入力はソフトキーボード・safe area・IME 変換を考慮し、本文と送信を隠さない。DOM 順序と focus 順序を視覚順に合わせる。画面幅に関係なく失敗理由や接続状態を隠さない。

## @theme の token 名一覧

実装時は [web/styles.css](../../web/styles.css) の Tailwind v4 `@theme` に集約する。以下は導入する token の名前と light の値。生の色・寸法を feature ごとに持たず、部品の variant から semantic utility を使う。参照 alias は `@theme inline` で公開し、shadcn の変数へ対応させる場合も色の正本を二重化しない。[Tailwind theme variables](https://tailwindcss.com/docs/theme) の名前空間に従う。

| token 名（複数の値は記載順） | 値 | 用途 |
|---|---|---|
| `--color-background` / `--color-surface` | `#F4F7F9` / `#FFFFFF` | 周辺面 / 作業面 |
| `--color-foreground` / `--color-muted-foreground` | `#20303C` / `#536572` | 本文 / 補助文 |
| `--color-muted` | `#F4F7F9` | 補足・readonly |
| `--color-card` / `--color-card-foreground` | surface / foreground の alias | 独立した内容の面 |
| `--color-popover` / `--color-popover-foreground` | surface / foreground の alias | overlay の面 |
| `--color-primary` / `--color-primary-foreground` | `#245675` / `#FFFFFF` | 主操作 |
| `--color-primary-hover` / `--color-primary-active` | `#1D4660` / `#17384D` | 主操作の状態 |
| `--color-secondary` / `--color-secondary-foreground` | surface / foreground の alias | 副操作 |
| `--color-accent` / `--color-accent-foreground` | `#E5EEF4` / `#20303C` | 選択・副操作 hover / active |
| `--color-border` / `--color-input` / `--color-ring` | `#D3DCE2` / `#758593` / `#245675` | 区切り / 入力境界 / focus |
| `--color-success` / `--color-success-foreground` | `#F0FDF4` / `#166534` | 確認済みの成功 |
| `--color-warning` / `--color-warning-foreground` | `#FEFCE8` / `#854D0E` | 要判断・注意 |
| `--color-danger` / `--color-danger-foreground` | `#FEF2F2` / `#991B1B` | 失敗・危険の案内 |
| `--color-info` / `--color-info-foreground` | `#EFF6FF` / `#1E40AF` | 補足 |
| `--color-neutral` / `--color-neutral-foreground` | `#F1F5F9` / `#475569` | 未確認・待機・disabled |
| `--color-running` / `--color-running-foreground` | `#ECFEFF` / `#155E75` | 進行中 |
| `--color-destructive` / `--color-destructive-foreground` | `#991B1B` / `#FFFFFF` | 危険操作の確定 |
| `--color-destructive-hover` | `#7F1D1D` | 危険の確定 hover / active |
| `--color-code` / `--color-code-foreground` | `#F1F5F9` / `#20303C` | code / 生ログ |
| `--color-overlay` | `rgb(32 48 60 / 0.4)` | 背景操作を覆う幕（文字を載せない） |
| `--font-sans` | `"Yu Gothic UI", "Hiragino Kaku Gothic ProN", "Meiryo", sans-serif` | 日本語 UI・本文・見出し |
| `--font-mono` | `ui-monospace, "SFMono-Regular", Consolas, "Liberation Mono", monospace` | code・path |
| `--text-title` / `--text-title-wide` / `--text-section` | `1.375rem` / `1.5rem` / `1.125rem` | h1 / desktop h1 / 節 |
| `--text-body` / `--text-label` / `--text-code` | `1rem` / `0.875rem` / `0.875rem` | 本文 / 補助 / code |
| `--text-title--line-height` / `--text-title-wide--line-height` / `--text-section--line-height` | `1.35` / `1.35` / `1.4` | 見出し行高 |
| `--text-body--line-height` / `--text-label--line-height` / `--text-code--line-height` | `1.5` / `1.5` / `1.6` | 本文等の行高 |
| `--font-weight-normal` / `--font-weight-medium` / `--font-weight-semibold` | `400` / `500` / `600` | 本文 / 操作 / 見出し |
| `--spacing` | `0.25rem` | 基本4px、既定の数値 utility の単位 |
| `--spacing-1` / `--spacing-2` / `--spacing-3` / `--spacing-4` / `--spacing-6` / `--spacing-8` | `0.25rem` / `0.5rem` / `0.75rem` / `1rem` / `1.5rem` / `2rem` | 4/8/12/16/24/32px の余白 |
| `--spacing-11` / `--spacing-target` | `2.75rem` / `2.75rem` | 44px target（CSS px の下限も共通部品で保証） |
| `--spacing-icon-sm` / `--spacing-icon` | `1rem` / `1.25rem` | 16 / 20px icon |
| `--spacing-nav` / `--spacing-list` / `--spacing-drawer` / `--spacing-form` | `14rem` / `20rem` / `32rem` / `40rem` | nav / 一覧の基準幅 / drawer 上限 / form 上限 |
| `--spacing-tabbar` | `4rem` | md 未満の画面下の固定タブ（shell の MobileTabBar）の高さ。本文の列の下余白と Console の送信欄の bottom は `--shell-bottom-inset`（= これ + `env(safe-area-inset-bottom)`、md 以上は 0）で空ける |
| `--container-prose` / `--container-prose-ja` | `65ch` / `40em` | 本文の行長上限 |
| `--radius-sm` / `--radius-md` / `--radius-lg` / `--radius-xl` | `0.25rem` / `0.375rem` / `0.5rem` / `0.75rem` | 4/6/8/12px の角丸 |
| `--shadow-flat` | `none` | 通常面 |
| `--shadow-popover` | `0 4px 12px rgb(32 48 60 / 0.12)` | メニュー |
| `--shadow-dialog` | `0 8px 24px rgb(32 48 60 / 0.18)` | 確認・drawer |
| `--breakpoint-md` / `--breakpoint-lg` | `48rem` / `64rem` | shell / 詳細併置の基準 |

`surface` 等の alias 表記は同表の `--color-surface` 等への参照を指す。表の `danger` は淡い状態面、shadcn の `destructive` は確定操作の塗りであり、同じ用途に交換しない。token に無い値を必要とする場合は共通設計へ理由と利用箇所を加え、画面の任意値で迂回しない。runtime の viewport 補正や safe area はデザイン値の追加とは分ける。

motion の `--duration-fast: 120ms`、`--duration-overlay: 180ms`、境界の `--border-width: 1px`、focus の `--focus-width: 2px`・`--focus-offset: 2px` は同じ CSS の通常の custom property として管理する。Tailwind の utility を自動生成する名前空間と混同しない。overlay の z-index は採用部品の管理に従う。

## Dark mode（提案）

dark mode は**提案のみ**。この仕様では light を基準とし、自動切替・toggle・dark 用依存を追加しない。必要性を確認した後、同じ semantic token の値を切り替える方式を検討する。各 feature に `dark:` の生色を散らさない。

採用時は単純な反転をせず、surface の明度差、状態色6組、diff、focus、disabled、overlay 上の文字を再設計・再計算する。4幅・全状態・キーボードの gate を light と同様に実施してから有効にする。OS 設定への追従と手動選択はその時に決める。本書の light の contrast 比を dark の保証に流用しない。
